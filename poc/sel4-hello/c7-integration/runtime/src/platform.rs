// Wasmtime custom platform pour seL4 AArch64 — C.6 runtime process
//
// Pattern identique à C.3 mais avec un pool BSS statique.
// Les pages BSS sont RWX grâce à VmAttributes::default() sur AArch64
// (seL4_ARM_Default_VMAttributes ne met pas EXECUTE_NEVER).
//
// POOL_PAGES = 128 → 512 KB pour le code Wasmtime + mémoire linéaire WASM.
//
// Edition 2024 : les corps de fonctions `unsafe extern "C"` ne sont plus
// automatiquement unsafe — il faut entourer les accès unsafe de blocs unsafe.

const POOL_PAGES: usize = 128;
const PAGE_SIZE: usize = 4096;

#[repr(align(4096))]
struct PagePool([u8; POOL_PAGES * PAGE_SIZE]);

static mut PAGE_POOL: PagePool = PagePool([0; POOL_PAGES * PAGE_SIZE]);
static mut POOL_BUMP: usize = 0;
static mut TLS_VALUE: *mut u8 = core::ptr::null_mut();

// ── Wasmtime custom platform functions ───────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_new(
    size: usize,
    _prot_flags: u32,
    ret: *mut *mut u8,
) -> i32 {
    unsafe {
        let bump = POOL_BUMP;
        let aligned_bump = (bump + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let end = aligned_bump.wrapping_add(size);
        if end > POOL_PAGES * PAGE_SIZE {
            sel4::debug_println!(
                "[C6] wasmtime_mmap_new: pool épuisé (bump={} + size={} > {})",
                aligned_bump,
                size,
                POOL_PAGES * PAGE_SIZE
            );
            return 1;
        }
        *ret = (core::ptr::addr_of!(PAGE_POOL.0) as usize + aligned_bump) as *mut u8;
        POOL_BUMP = end;
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_remap(
    _addr: *mut u8,
    _size: usize,
    _prot_flags: u32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_munmap(_ptr: *mut u8, _size: usize) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mprotect(
    _ptr: *mut u8,
    _size: usize,
    _prot_flags: u32,
) -> i32 {
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
    panic!("wasmtime trap (longjmp) dans le runtime C.6");
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
    unsafe {
        *ret = core::ptr::null_mut();
    }
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
    unsafe {
        TLS_VALUE = ptr;
    }
}
