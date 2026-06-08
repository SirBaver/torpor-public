// Jalon C.3 — Pré-compilation AOT du module WASM add(i32,i32)->i32
//
// Tourne sur le HOST (x86_64 Linux, std disponible) avec wasmtime cranelift.
// Génère OUT_DIR/add.cwasm pour aarch64-unknown-unknown.
// La root task embarque add.cwasm via include_bytes! et l'exécute via
// Module::deserialize (runtime-only, pas de cranelift sur la cible seL4).

fn main() {
    let wat = r#"(module
  (func (export "add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add))"#;

    let mut config = wasmtime::Config::new();
    // aarch64-unknown-unknown : os=Unknown, correspond au triple parsé par la
    // cible aarch64-sel4 (vendor=Custom("sel4"), os=Unknown) → check_triple OK
    config
        .target("aarch64-unknown-unknown")
        .expect("set target aarch64-unknown-unknown");

    let engine = wasmtime::Engine::new(&config).expect("create host engine");
    let cwasm = engine
        .precompile_module(wat.as_bytes())
        .expect("precompile WAT → cwasm");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(format!("{out_dir}/add.cwasm"), &cwasm).expect("write add.cwasm");

    println!("cargo:rerun-if-changed=build.rs");
}
