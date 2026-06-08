// build.rs superviseur C.11-prov
// Compile agent-prov.wat → OUT_DIR/agent-prov.cwasm (AOT AArch64)

fn main() {
    let mut config = wasmtime::Config::new();
    config
        .target("aarch64-unknown-unknown")
        .expect("set target aarch64-unknown-unknown");

    let engine = wasmtime::Engine::new(&config).expect("create host engine");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    let wat = include_str!("src/agent-prov.wat");
    let cwasm = engine
        .precompile_module(wat.as_bytes())
        .expect("precompile agent-prov.wat");
    std::fs::write(format!("{out_dir}/agent-prov.cwasm"), &cwasm)
        .expect("write agent-prov.cwasm");

    println!("cargo:rerun-if-changed=src/agent-prov.wat");
    println!("cargo:rerun-if-changed=build.rs");
}
