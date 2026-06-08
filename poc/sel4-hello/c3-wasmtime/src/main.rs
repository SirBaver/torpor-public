// Jalon C.3 — Root task seL4 + Wasmtime min-platform AArch64 (ADR-0037/0039)
//
// Critères C.3 :
//   (a) Module WASM pré-compilé AOT (build.rs) embarqué via include_bytes!
//   (b) Engine::default() + Module::deserialize sans cranelift (runtime-only)
//   (c) Instance + TypedFunc::call → add(21, 21) == 42
//   Signal de succès : "C3_PASS"
#![no_std]
#![no_main]

extern crate alloc;

use sel4_root_task::{root_task, Never};

mod platform;

// Module WASM compilé pour aarch64-unknown-unknown par build.rs
static ADD_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/add.cwasm"));

#[root_task(heap_size = 4 * 1024 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.3 ===");
    sel4::debug_println!("    Wasmtime min-platform + AOT AArch64 (ADR-0037/0039)");

    // Critère C.3a : init pool 64 pages RWX pour le code Wasmtime
    platform::init_pool(bootinfo);

    // Critère C.3b : deserialize module pré-compilé (pas de cranelift à l'exécution)
    let engine = wasmtime::Engine::default();
    let module = unsafe {
        wasmtime::Module::deserialize(&engine, ADD_CWASM)
            .expect("Module::deserialize échoué")
    };

    let mut store = wasmtime::Store::new(&engine, ());

    // Critère C.3c : instanciation + appel
    let instance = wasmtime::Instance::new(&mut store, &module, &[])
        .expect("Instance::new échoué");

    let add = instance
        .get_typed_func::<(i32, i32), i32>(&mut store, "add")
        .expect("get_typed_func 'add' échoué");

    let result = add.call(&mut store, (21, 21)).expect("call add(21,21) échoué");

    sel4::debug_println!("add(21, 21) = {}", result);

    if result == 42 {
        sel4::debug_println!("C3_PASS");
    } else {
        sel4::debug_println!("C3_FAIL: attendu 42, obtenu {}", result);
    }

    sel4::init_thread::suspend_self()
}
