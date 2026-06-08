// Wasmtime custom platform pour seL4 AArch64
//
// Implémente les 13 fonctions C requises par :
//   wasmtime-25.0.3/src/runtime/vm/sys/custom/capi.rs
//
// Memory model : pool de 64 pages RWX pré-mappées à POOL_VA_BASE.
// wasmtime_mmap_new → bump allocator dans ce pool.
// mprotect/munmap/mmap_remap → no-ops (pages déjà RWX, pas de libération).
//
// Justification exécutabilité : seL4_ARM_Default_VMAttributes n'active pas
// EXECUTE_NEVER, donc les pages mappées avec VmAttributes::default() sont RWX.

use sel4::{CapRights, ObjectBlueprint, ObjectBlueprintArch, VmAttributes, init_thread};

const POOL_VA_BASE: usize = 0x1000_0000;
const POOL_PAGES: usize = 64;
const PAGE_SIZE: usize = 4096;

static mut POOL_BUMP: usize = 0;
static mut TLS_VALUE: *mut u8 = core::ptr::null_mut();

/// Alloue 64 SmallPages dans le plus grand Untyped non-device et les mappe
/// à POOL_VA_BASE avec droits R/W (VmAttributes::default() = pas EXECUTE_NEVER).
/// Les PTs intermédiaires manquants sont alloués à la volée depuis le même Untyped.
pub fn init_pool(bootinfo: &sel4::BootInfoPtr) {
    let cnode = init_thread::slot::CNODE.cap();
    let page_bp = ObjectBlueprint::Arch(ObjectBlueprintArch::SmallPage);
    let pt_bp = ObjectBlueprint::Arch(ObjectBlueprintArch::PT);

    let mut next_slot = bootinfo.empty().start();

    // Plus grand Untyped non-device (typiquement plusieurs centaines de MB sur QEMU 1G)
    let (ut_ix, _) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.is_device())
        .max_by_key(|(_, d)| d.size_bits())
        .expect("aucun Untyped non-device");

    let ut = bootinfo.untyped().index(ut_ix).cap();

    // Retype 64 SmallPages d'un coup (watermark Untyped avance de 64 × 4 KB)
    let pages_base = next_slot;
    ut.untyped_retype(&page_bp, &cnode.absolute_cptr_for_self(), pages_base, POOL_PAGES)
        .expect("retype 64 SmallPages échoué");
    next_slot += POOL_PAGES;

    // Mappe chaque page à POOL_VA_BASE + i × 4KB
    // frame_map renvoie Err si un niveau de PT intermédiaire est absent →
    // on retype un PT supplémentaire et on réessaie (au plus 3 PTs pour VA 0x10000000)
    for i in 0..POOL_PAGES {
        let va = POOL_VA_BASE + i * PAGE_SIZE;
        loop {
            let page =
                init_thread::Slot::<sel4::cap_type::SmallPage>::from_index(pages_base + i).cap();
            match page.frame_map(
                init_thread::slot::VSPACE.cap(),
                va,
                CapRights::read_write(),
                VmAttributes::default(),
            ) {
                Ok(()) => break,
                Err(_) => {
                    // PT manquant à ce niveau de traduction — on en alloue un
                    let pt_slot = next_slot;
                    next_slot += 1;
                    ut.untyped_retype(
                        &pt_bp,
                        &cnode.absolute_cptr_for_self(),
                        pt_slot,
                        1,
                    )
                    .expect("retype PT échoué");
                    let pt =
                        init_thread::Slot::<sel4::cap_type::PT>::from_index(pt_slot).cap();
                    pt.pt_map(
                        init_thread::slot::VSPACE.cap(),
                        va,
                        VmAttributes::default(),
                    )
                    .expect("pt_map échoué");
                    // Retry frame_map
                }
            }
        }
    }
}

// ── Wasmtime custom platform functions ───────────────────────────────────────
// Signatures : wasmtime-25.0.3/src/runtime/vm/sys/custom/capi.rs

/// Alloue `size` octets depuis le pool RWX (bump allocator).
/// `prot_flags` ignoré : toutes les pages du pool sont déjà RWX.
#[no_mangle]
pub unsafe extern "C" fn wasmtime_mmap_new(
    size: usize,
    _prot_flags: u32,
    ret: *mut *mut u8,
) -> i32 {
    let bump = POOL_BUMP;
    let end = bump.wrapping_add(size);
    if end > POOL_PAGES * PAGE_SIZE {
        // Ne devrait pas arriver pour un module trivial sans mémoire linéaire
        sel4::debug_println!(
            "wasmtime_mmap_new: pool épuisé (bump={} + size={} > {})",
            bump,
            size,
            POOL_PAGES * PAGE_SIZE
        );
        return 1;
    }
    *ret = (POOL_VA_BASE + bump) as *mut u8;
    POOL_BUMP = end;
    0
}

/// Remplace un mapping existant — no-op : les pages du pool restent RWX.
#[no_mangle]
pub unsafe extern "C" fn wasmtime_mmap_remap(
    _addr: *mut u8,
    _size: usize,
    _prot_flags: u32,
) -> i32 {
    0
}

/// Libère un mapping — no-op : pas de récupération de pool (one-shot).
#[no_mangle]
pub unsafe extern "C" fn wasmtime_munmap(_ptr: *mut u8, _size: usize) -> i32 {
    0
}

/// Change les protections d'un mapping — no-op : pages déjà RWX.
#[no_mangle]
pub unsafe extern "C" fn wasmtime_mprotect(
    _ptr: *mut u8,
    _size: usize,
    _prot_flags: u32,
) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn wasmtime_page_size() -> usize {
    PAGE_SIZE
}

/// Pseudo-setjmp : appelle le callback directement, renvoie 1 (callback OK).
/// Pas de vraie gestion de trap (module trivial ne tombe pas en trap).
#[no_mangle]
pub unsafe extern "C" fn wasmtime_setjmp(
    jmp_buf: *mut *const u8,
    callback: extern "C" fn(*mut u8, *mut u8),
    payload: *mut u8,
    callee: *mut u8,
) -> i32 {
    static DUMMY: u8 = 0;
    *jmp_buf = &DUMMY as *const u8;
    callback(payload, callee);
    1 // callback returned normally (no trap)
}

/// Pseudo-longjmp : ne devrait pas être appelé pour notre module.
#[no_mangle]
pub unsafe extern "C" fn wasmtime_longjmp(_jmp_buf: *const u8) -> ! {
    panic!("wasmtime trap (longjmp) — module WASM en erreur");
}

/// Enregistre le handler de trap seL4 — no-op (pas de signal hardware).
#[no_mangle]
pub unsafe extern "C" fn wasmtime_init_traps(
    _handler: unsafe extern "C" fn(usize, usize, bool, usize),
) -> i32 {
    0
}

/// Pas de support CoW memory image — retourne null (acceptable selon capi.rs).
#[no_mangle]
pub unsafe extern "C" fn wasmtime_memory_image_new(
    _ptr: *const u8,
    _len: usize,
    ret: *mut *mut u8,
) -> i32 {
    *ret = core::ptr::null_mut();
    0
}

/// Jamais appelé car wasmtime_memory_image_new retourne null.
#[no_mangle]
pub unsafe extern "C" fn wasmtime_memory_image_map_at(
    _image: *mut u8,
    _addr: *mut u8,
    _len: usize,
) -> i32 {
    0
}

/// Jamais appelé car wasmtime_memory_image_new retourne null.
#[no_mangle]
pub unsafe extern "C" fn wasmtime_memory_image_free(_image: *mut u8) {}

#[no_mangle]
pub unsafe extern "C" fn wasmtime_tls_get() -> *mut u8 {
    TLS_VALUE
}

#[no_mangle]
pub unsafe extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    TLS_VALUE = ptr;
}
