fn main() {
    let server_elf = std::env::var("SERVER_ELF")
        .unwrap_or_else(|_| "build/server.elf".to_string());
    let runtime_elf = std::env::var("RUNTIME_ELF")
        .unwrap_or_else(|_| "build/runtime.elf".to_string());
    let phase = std::env::var("PHASE").unwrap_or_else(|_| "0".to_string());

    println!("cargo:rustc-env=SERVER_ELF={server_elf}");
    println!("cargo:rustc-env=RUNTIME_ELF={runtime_elf}");
    println!("cargo:rustc-env=PHASE={phase}");
    println!("cargo:rerun-if-env-changed=SERVER_ELF");
    println!("cargo:rerun-if-env-changed=RUNTIME_ELF");
    println!("cargo:rerun-if-env-changed=PHASE");
    println!("cargo:rerun-if-changed=build.rs");
}
