// echo.rs — agent WASM minimal (B4 semaine 1).
// Appelle agent_introspect (A1) puis émet le résultat dans le log causal.
// Critère de sortie S1 : un module WASM compilé depuis Rust, chargé depuis
// disque, exerce A1 et termine proprement.
//
// Build : cargo build --target wasm32-unknown-unknown -p agent-sdk --example echo
// Output : target/wasm32-unknown-unknown/debug/examples/echo.wasm
//
// Sur wasm32 : pas de main requis (no_main), process() est le seul point d'entrée.
// Sur native : main() vide requis par Cargo (cargo check sans --target).
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::*;

/// Point d'entrée appelé par run_loop pour chaque Message::Data.
/// `ptr`/`len` : payload du message (ignoré par echo).
#[no_mangle]
pub unsafe extern "C" fn process(_ptr: i32, _len: i32) {
    // A1 — lire l'état courant
    let mut buf = [0u8; INTROSPECT_LEN];
    introspect(&mut buf);

    // S4 — barrière avant tout emit
    barrier();

    // Émet le payload introspect (EmitType::Introspect = 6)
    emit_raw(6, &buf);
}

#[allow(dead_code)]
fn main() {}
