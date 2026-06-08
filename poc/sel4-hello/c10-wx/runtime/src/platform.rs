// Wasmtime custom platform W^X pour seL4 AArch64 — C.10
//
// Le pool JIT est sorti du .bss : 128 frames dédiées pré-mappées RW+EXECUTE_NEVER
// par le superviseur à JIT_POOL_VA_BASE (ADR-0047 §D4/§D5).
//
// W^X invariant : jamais simultanément WRITE et EXECUTE sur une même page.
//   - État W (écriture Cranelift) : RW + EXECUTE_NEVER = prot 0x3
//   - État X (exécution JIT)      : R  + EXECUTABLE    = prot 0x5
//   Transition W→X via wasmtime_mprotect(PROT_READ|PROT_EXEC) :
//     frame_unmap() + frame_map(vspace, va, read_only(), DEFAULT)
//   Transition X→W (si Cranelift ré-émet) via wasmtime_mprotect(PROT_READ|PROT_WRITE) :
//     frame_unmap() + frame_map(vspace, va, read_write(), DEFAULT|EXECUTE_NEVER)
//
// Valeurs seL4 AArch64 (bindings rev 7a2321f2) :
//   seL4_ARM_Default_VMAttributes = 3 (cacheable + parity, PAS EXECUTE_NEVER)
//   seL4_ARM_ExecuteNever = 4
//
// Layout CNode runtime (size_bits=8) :
//   Slot 4 = VSPACE  (cap VSpace du runtime)
//   Slot 5..132 = caps frames JIT (RUNTIME_JIT_FRAME_BASE = 5)

pub(crate) const JIT_POOL_VA_BASE: usize = 0x4000_0000;
const JIT_POOL_PAGES: usize = 128;
const PAGE_SIZE: usize = 4096;

// Slots CNode runtime (doivent correspondre à supervisor/src/main.rs)
const JIT_POOL_VSPACE_SLOT: u64 = 4;
const JIT_POOL_FRAME_BASE:  u64 = 5;

// Flags Wasmtime mprotect (POSIX mprotect bits)
#[allow(dead_code)]
const PROT_READ:  u32 = 0x1;
const PROT_WRITE: u32 = 0x2;
const PROT_EXEC:  u32 = 0x4;

static mut POOL_BUMP: usize = 0;
static mut TLS_VALUE: *mut u8 = core::ptr::null_mut();

// Adresse de la première page passée en état RX (pour le test négatif)
pub(crate) static mut FIRST_RX_PAGE_VA: usize = 0;

// ── Wasmtime custom platform functions ───────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_new(
    size: usize,
    _prot_flags: u32,
    ret: *mut *mut u8,
) -> i32 {
    // Bump allocator sur le pool JIT (frames pré-mappées RW+XN par superviseur)
    unsafe {
        let bump = POOL_BUMP;
        let aligned = (bump + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let end = aligned.wrapping_add(size);
        if end > JIT_POOL_PAGES * PAGE_SIZE {
            sel4::debug_println!(
                "[C10] wasmtime_mmap_new: pool épuisé (aligned={} + size={} > {})",
                aligned, size, JIT_POOL_PAGES * PAGE_SIZE
            );
            return 1;
        }
        *ret = (JIT_POOL_VA_BASE + aligned) as *mut u8;
        POOL_BUMP = end;
        0
    }
}

/// Transition de protection W^X via unmap + remap (ADR-0047 §D3/§D4).
///
/// prot_flags (Wasmtime POSIX bits) :
///   PROT_READ|PROT_WRITE (0x3) → état W : RW + EXECUTE_NEVER
///   PROT_READ|PROT_EXEC  (0x5) → état X : R  + DEFAULT (executable)
///
/// Seules les pages dans [JIT_POOL_VA_BASE, JIT_POOL_VA_BASE + pool_size) sont traitées.
/// Les autres (mémoire linéaire WASM, guards) passent en no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mprotect(
    ptr: *mut u8,
    size: usize,
    prot_flags: u32,
) -> i32 {
    unsafe {
        let va_start = ptr as usize;

        // Hors pool JIT → no-op (mémoire linéaire WASM ou guard pages)
        if va_start < JIT_POOL_VA_BASE
            || va_start >= JIT_POOL_VA_BASE + JIT_POOL_PAGES * PAGE_SIZE
        {
            return 0;
        }

        let page_idx_start = (va_start - JIT_POOL_VA_BASE) / PAGE_SIZE;
        let n_pages = size.next_multiple_of(PAGE_SIZE) / PAGE_SIZE;
        let vspace = sel4::cap::VSpace::from_bits(JIT_POOL_VSPACE_SLOT);

        let want_exec  = prot_flags & PROT_EXEC  != 0;
        let want_write = prot_flags & PROT_WRITE != 0;

        for i in 0..n_pages {
            let page_idx = page_idx_start + i;
            if page_idx >= JIT_POOL_PAGES {
                return 1;
            }

            let page_va = JIT_POOL_VA_BASE + page_idx * PAGE_SIZE;
            let frame_cap = sel4::cap::Granule::from_bits(
                JIT_POOL_FRAME_BASE + page_idx as u64
            );

            // Unmap avant remap (nécessaire pour changer les droits sous seL4)
            frame_cap.frame_unmap().unwrap();

            if want_exec {
                // État X : RX — read + executable, PAS writable (W^X)
                // VmAttributes::DEFAULT (= 3) = pas EXECUTE_NEVER → page exécutable
                frame_cap.frame_map(
                    vspace,
                    page_va,
                    sel4::CapRights::read_only(),
                    sel4::VmAttributes::default(), // DEFAULT = pas EXECUTE_NEVER → executable
                ).unwrap();

                // Enregistrer la première page RX pour le test négatif
                if FIRST_RX_PAGE_VA == 0 {
                    FIRST_RX_PAGE_VA = page_va;
                }
            } else {
                // État W : RW + EXECUTE_NEVER → W^X protégé
                frame_cap.frame_map(
                    vspace,
                    page_va,
                    sel4::CapRights::read_write(),
                    sel4::VmAttributes::default() | sel4::VmAttributes::EXECUTE_NEVER,
                ).unwrap();
            }
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_remap(
    _addr: *mut u8,
    _size: usize,
    _prot_flags: u32,
) -> i32 {
    // No-op : wasmtime_memory_image non utilisé (WAT sans mémoire linéaire)
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_munmap(_ptr: *mut u8, _size: usize) -> i32 {
    // No-op : bump allocator, pas de libération individuelle
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_page_size() -> usize {
    PAGE_SIZE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_setjmp(
    jmp_buf: *mut *const u8,
    callback: extern "C" fn(*mut u8, *mut u8),
    payload: *mut u8,
    callee: *mut u8,
) -> i32 {
    unsafe {
        static DUMMY: u8 = 0;
        *jmp_buf = &DUMMY as *const u8;
        callback(payload, callee);
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_longjmp(_jmp_buf: *const u8) -> ! {
    panic!("wasmtime trap (longjmp) dans le runtime C.10");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_init_traps(
    _handler: unsafe extern "C" fn(usize, usize, bool, usize),
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_memory_image_new(
    _ptr: *const u8,
    _len: usize,
    ret: *mut *mut u8,
) -> i32 {
    unsafe { *ret = core::ptr::null_mut(); }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_memory_image_map_at(
    _image: *mut u8,
    _addr: *mut u8,
    _len: usize,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_memory_image_free(_image: *mut u8) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_tls_get() -> *mut u8 {
    unsafe { TLS_VALUE }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_tls_set(ptr: *mut u8) {
    unsafe { TLS_VALUE = ptr; }
}
