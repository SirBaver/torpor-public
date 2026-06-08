// Jalon C.11 — Pré-compilation AOT des deux modules WASM
//
// agent-oob.wat  : module OOB (P-α)
// agent-loop.wat : module boucle infinie (P-β)
//
// Tourne sur le HOST avec wasmtime cranelift.
// Génère OUT_DIR/agent-oob.cwasm et OUT_DIR/agent-loop.cwasm pour aarch64-unknown-unknown.

fn main() {
    let mut config = wasmtime::Config::new();
    config
        .target("aarch64-unknown-unknown")
        .expect("set target aarch64-unknown-unknown");

    let engine = wasmtime::Engine::new(&config).expect("create host engine");

    let out_dir = std::env::var("OUT_DIR").unwrap();

    // Compiler agent-oob.wat
    let wat_oob = include_str!("src/agent-oob.wat");
    let cwasm_oob = engine
        .precompile_module(wat_oob.as_bytes())
        .expect("precompile agent-oob.wat → cwasm");
    std::fs::write(format!("{out_dir}/agent-oob.cwasm"), &cwasm_oob)
        .expect("write agent-oob.cwasm");

    // Compiler agent-loop.wat
    let wat_loop = include_str!("src/agent-loop.wat");
    let cwasm_loop = engine
        .precompile_module(wat_loop.as_bytes())
        .expect("precompile agent-loop.wat → cwasm");
    std::fs::write(format!("{out_dir}/agent-loop.cwasm"), &cwasm_loop)
        .expect("write agent-loop.cwasm");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/agent-oob.wat");
    println!("cargo:rerun-if-changed=src/agent-loop.wat");
}
