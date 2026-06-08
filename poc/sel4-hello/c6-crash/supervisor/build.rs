// Build script du superviseur C.6-crash
//
// Les chemins vers server.elf et runtime.elf sont fournis via
// les variables d'environnement SERVER_ELF et RUNTIME_ELF.
// Ce script les expose au code Rust via cargo:rustc-env.
// KILL_POINT est tracké pour invalider le cache si la constante change.

fn main() {
    let server_elf = std::env::var("SERVER_ELF")
        .unwrap_or_else(|_| "build/server.elf".to_string());
    let runtime_elf = std::env::var("RUNTIME_ELF")
        .unwrap_or_else(|_| "build/runtime.elf".to_string());

    println!("cargo:rustc-env=SERVER_ELF={server_elf}");
    println!("cargo:rustc-env=RUNTIME_ELF={runtime_elf}");
    println!("cargo:rerun-if-env-changed=SERVER_ELF");
    println!("cargo:rerun-if-env-changed=RUNTIME_ELF");
    println!("cargo:rerun-if-env-changed=KILL_POINT");
    println!("cargo:rerun-if-changed=build.rs");
}
