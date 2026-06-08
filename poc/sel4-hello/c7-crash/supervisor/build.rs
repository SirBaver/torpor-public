// Build script du superviseur C.7-crash
//
// SERVER_ELF, RUNTIME_A_ELF (instrumenté KP), RUNTIME_B_ELF (nominal) fournis via env vars.
// KILL_POINT est aussi transmis pour que le superviseur puisse asserter les bons invariants.

fn main() {
    let server_elf = std::env::var("SERVER_ELF")
        .unwrap_or_else(|_| "build/server.elf".to_string());
    let runtime_a_elf = std::env::var("RUNTIME_A_ELF")
        .unwrap_or_else(|_| "build/runtime_a.elf".to_string());
    let runtime_b_elf = std::env::var("RUNTIME_B_ELF")
        .unwrap_or_else(|_| "build/runtime_b.elf".to_string());
    let kill_point = std::env::var("KILL_POINT").unwrap_or_else(|_| "1".to_string());

    println!("cargo:rustc-env=SERVER_ELF={server_elf}");
    println!("cargo:rustc-env=RUNTIME_A_ELF={runtime_a_elf}");
    println!("cargo:rustc-env=RUNTIME_B_ELF={runtime_b_elf}");
    println!("cargo:rustc-env=KILL_POINT={kill_point}");
    println!("cargo:rerun-if-env-changed=SERVER_ELF");
    println!("cargo:rerun-if-env-changed=RUNTIME_A_ELF");
    println!("cargo:rerun-if-env-changed=RUNTIME_B_ELF");
    println!("cargo:rerun-if-env-changed=KILL_POINT");
    println!("cargo:rerun-if-changed=build.rs");
}
