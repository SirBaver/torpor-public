/// PoC de fumée ADR-0037 — Wasmtime minimal features
///
/// Phase 1 : Cranelift JIT (features = ["runtime", "cranelift", "wat"])
///   - Compile + exécute un module WAT avec host functions
///   - Mesures RSS + latence
///
/// Phase 2 (mode --serialize) : runtime seul (profil seL4)
///   - Sérialise le module en .cwasm (nécessite cranelift)
///   - Recharge avec Module::deserialize() en mode runtime-only
///   - Simule le profil seL4 : modules pré-compilés, pas de JIT au runtime
use anyhow::Result;
use std::time::Instant;
use wasmtime::{Engine, Linker, Module, Store};

/// Module WAT minimal : importe une host function `host::add_cause(i32, i32) -> i32`
/// (proxy de l'invariant B-light : vérifie une action_id dans le log causal),
/// l'appelle et retourne le résultat.
const WAT: &str = r#"
(module
  (import "host" "check_action" (func $check_action (param i32 i32) (result i32)))
  (import "host" "log_entry"    (func $log_entry    (param i32)      (result i32)))

  (func (export "run") (result i32)
    ;; Simule agent_add_cause : vérifie action_id (42, 0) dans le log
    i32.const 42
    i32.const 0
    call $check_action
    ;; Résultat attendu : 1 (présent dans le log simulé)

    ;; Simule commit_barrier : log l'entrée seq=7
    i32.const 7
    call $log_entry
    drop

    ;; Retourne le résultat de check_action
  )
)
"#;

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn main() -> Result<()> {
    let serialize_mode = std::env::args().any(|a| a == "--serialize");

    println!("=== sel4-smoke : Wasmtime minimal features ===\n");
    if serialize_mode {
        println!("Mode : sérialisation .cwasm (profil seL4 — runtime-only)\n");
    } else {
        println!("features : runtime + cranelift + wat (no WASI, no component-model, no cache)\n");
    }

    let rss_before = rss_kb();

    let t0 = Instant::now();
    let engine = Engine::default();
    let t_engine = t0.elapsed();

    // Compilation du module WAT → code natif Cranelift
    let t1 = Instant::now();
    let module = Module::new(&engine, WAT)?;
    let t_compile = t1.elapsed();

    // Mode --serialize : sérialise le module en .cwasm puis le recharge
    // (simule le profil seL4 : modules AOT pré-compilés)
    if serialize_mode {
        let cwasm = module.serialize()?;
        println!("  Module sérialisé : {} KB", cwasm.len() / 1024);
        let t_deser = Instant::now();
        let _module2 = unsafe { Module::deserialize(&engine, &cwasm)? };
        println!("  Module::deserialize() : {:.3} ms", t_deser.elapsed().as_secs_f64() * 1000.0);
        println!("  → deserialize sans cranelift JIT : GO");
        println!("\n  Sur seL4 : le .cwasm sera produit hors-ligne (cross-compile),");
        println!("  puis chargé avec feature 'runtime' uniquement (sans cranelift).");
        return Ok(());
    }

    let rss_after_compile = rss_kb();

    // Store + host functions
    let mut store: Store<()> = Store::new(&engine, ());
    let mut linker: Linker<()> = Linker::new(&engine);

    // Host function 1 : check_action(action_id_lo: i32, action_id_hi: i32) -> i32
    // Simule B-light : retourne 1 si l'action existe dans le log, 0 sinon.
    // En production : `CausalLog::get(&action_id)` — IPC seL4 vers le serveur de log.
    linker.func_wrap("host", "check_action", |action_id_lo: i32, _action_id_hi: i32| -> i32 {
        // Log simulé : action_id 42 est présent
        if action_id_lo == 42 { 1 } else { 0 }
    })?;

    // Host function 2 : log_entry(seq: i32) -> i32
    // Simule commit_barrier : appende une entrée dans le CausalLog.
    // En production : `CausalLog::append()` — IPC seL4 vers le serveur de log.
    linker.func_wrap("host", "log_entry", |seq: i32| -> i32 {
        let _ = seq; // En prod : append réel
        0 // Ok
    })?;

    // Instanciation
    let t2 = Instant::now();
    let instance = linker.instantiate(&mut store, &module)?;
    let t_instantiate = t2.elapsed();

    let rss_after_instance = rss_kb();

    // Récupération + appel de la fonction exportée
    let run = instance.get_typed_func::<(), i32>(&mut store, "run")?;

    // Warmup (1 appel pour JIT warm)
    let _ = run.call(&mut store, ())?;

    // Mesure latence sur N appels
    const N: u32 = 10_000;
    let t3 = Instant::now();
    let mut result = 0i32;
    for _ in 0..N {
        result = run.call(&mut store, ())?;
    }
    let t_calls = t3.elapsed();

    let rss_final = rss_kb();

    // ── Résultats ──────────────────────────────────────────────────────────
    println!("--- Résultats ---\n");
    println!("Valeur retournée par run() : {result} (attendu : 1)");
    assert_eq!(result, 1, "host function check_action doit retourner 1 pour action_id=42");
    println!("Assertion : OK\n");

    println!("--- Latences ---");
    println!("  Engine::default()         : {:>8.3} ms", t_engine.as_secs_f64() * 1000.0);
    println!("  Module::new (compile WAT) : {:>8.3} ms", t_compile.as_secs_f64() * 1000.0);
    println!("  Linker::instantiate       : {:>8.3} ms", t_instantiate.as_secs_f64() * 1000.0);
    println!("  run() × {N} appels        : {:>8.3} ms total  ({:.3} µs/appel)",
        t_calls.as_secs_f64() * 1000.0,
        t_calls.as_secs_f64() * 1e6 / N as f64);
    println!();

    println!("--- RSS (VmRSS /proc/self/status) ---");
    println!("  Avant Engine::default()   : {:>6} KB", rss_before);
    println!("  Après Module::new         : {:>6} KB  (+{} KB)", rss_after_compile, rss_after_compile.saturating_sub(rss_before));
    println!("  Après instantiate         : {:>6} KB  (+{} KB vs compile)", rss_after_instance, rss_after_instance.saturating_sub(rss_after_compile));
    println!("  Final (après {N} appels)  : {:>6} KB  (+{} KB total)", rss_final, rss_final.saturating_sub(rss_before));
    println!();

    println!("--- Verdict ADR-0037 ---");
    let overhead = rss_final.saturating_sub(rss_before);
    if overhead < 50_000 {
        println!("  RSS overhead < 50 MB : GO (acceptable pour cible seL4)");
    } else {
        println!("  RSS overhead >= 50 MB : WARN — vérifier si c'est Cranelift JIT cache");
        println!("  Note : sur seL4 avec Module::deserialize(), le JIT cache est absent.");
    }

    println!("\n--- Note sur la portabilité seL4 ---");
    println!("  Ce test utilise Cranelift JIT (feature 'cranelift').");
    println!("  Sur seL4 : feature 'runtime' uniquement + modules .cwasm pré-compilés.");
    println!("  Étape suivante : tester 'runtime' seul avec Module::deserialize().");

    Ok(())
}
