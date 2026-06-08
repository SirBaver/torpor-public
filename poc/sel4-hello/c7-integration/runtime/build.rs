// Jalon C.6 — Pré-compilation AOT du module WASM agent.wat
//
// Tourne sur le HOST avec wasmtime cranelift.
// Génère OUT_DIR/agent.cwasm pour aarch64-unknown-unknown.

fn main() {
    let wat = include_str!("src/agent.wat");

    let mut config = wasmtime::Config::new();
    config
        .target("aarch64-unknown-unknown")
        .expect("set target aarch64-unknown-unknown");

    let engine = wasmtime::Engine::new(&config).expect("create host engine");
    let cwasm = engine
        .precompile_module(wat.as_bytes())
        .expect("precompile WAT → cwasm");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(format!("{out_dir}/agent.cwasm"), &cwasm).expect("write agent.cwasm");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/agent.wat");
}
