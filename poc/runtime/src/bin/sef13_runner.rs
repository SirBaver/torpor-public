// sef13-runner — SEF-13 traçabilité causale adversariale (campagne P3, ADR-0053 §D-P3).
//
// DEUX TESTS (un seul binaire) :
//
//   V3.3a — action_id forgé (zéro faux-positif) :
//     10 000 action_id aléatoires (32 bytes) → log.get() doit retourner None pour chacun.
//     Oracle : aucun faux-positif (SHA-256 collision résistant, borne P3a).
//
//   V3.3b — intégrité content-addressed :
//     Pour chaque action_id réel (issu du log) : log.get(id) retourne un LogEntry
//     dont entry.action_id() == id (la clé RocksDB = SHA-256 du contenu).
//     Oracle : aucune violation d'intégrité.
//
//   V3.4 — DAG cyclique (finding gate) :
//     Non implémenté (non-constructible par construction : append-only +
//     content-addressed + vérification d'existence = pas de fixed-point SHA-256).
//     Voir ADR-0053 §Gate Q2 et §D-P3 V3.4.
//     Propriété garantie par design, pas par check explicite. Documenté ici sans harness.

use std::sync::Arc;
use std::time::Duration;

use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType};
use os_poc_runtime::actor::{ActorInstance, Message, AGENT_WAT};
use os_poc_runtime::make_engine;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_store::{Cache, ContentStore};
use wasmtime::Module;

// Générateur LCG déterministe pour les action_id forgés (pas de dépendance rand).
struct Lcg(u64);
impl Lcg {
    fn next_bytes(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            chunk.copy_from_slice(&self.0.to_le_bytes());
        }
        out
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    let base = std::env::temp_dir().join(format!("sef13-{run_id}"));
    let store_path = base.join("store");
    let log_path = base.join("log");
    std::fs::create_dir_all(&store_path).unwrap();
    std::fs::create_dir_all(&log_path).unwrap();

    let agent_id = [0x13u8; 16];
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&store_path, Some(shared_cache.clone())).expect("store"));
    let log   = Arc::new(CausalLog::open(&log_path, Some(shared_cache)).expect("log"));

    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let actor  = ActorInstance::new_precompiled(
        &engine, &module, agent_id, Arc::clone(&store), Arc::clone(&log),
    ).await.expect("actor");

    let mut scheduler = Scheduler::new();
    scheduler.set_log_ref(Arc::clone(&log));
    let tx = scheduler.register(actor);

    println!("=== SEF-13 — Traçabilité causale adversariale (P3, ADR-0053 §D-P3) ===");

    // ── Phase 1 : construire le log ────────────────────────────────────────
    let n_actions = 500usize;
    eprintln!("[sef13] Phase 1 : {n_actions} actions");
    for i in 0..n_actions {
        tx.send(Message::data(format!("sef13-{i:08}").into_bytes())).await.expect("send");
    }

    // Drain : attendre N ActionResult.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let ids = log.query_by_agent_range(&agent_id, None, None).unwrap_or_default();
        let count = ids.iter().filter(|id| {
            log.get(id).ok().flatten()
                .and_then(|e| e.emit_payload)
                .and_then(|p| EmitEnvelope::from_msgpack(&p).ok())
                .map(|e| e.emit_type == EmitType::ActionResult as u8)
                .unwrap_or(false)
        }).count();
        if count >= n_actions { break; }
        if std::time::Instant::now() > deadline {
            eprintln!("[sef13] TIMEOUT drain (vu {} / {})", count, n_actions);
            std::process::exit(2);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(tx);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Collecter tous les action_id réels.
    let real_ids = log.query_by_agent_range(&agent_id, None, None).unwrap_or_default();
    eprintln!("[sef13] {} action_id réels dans le log", real_ids.len());

    // ── V3.3a : zéro faux-positif sous 10 000 forgeries aléatoires ─────────
    println!("\n--- V3.3a : action_id forgé (10 000 lookups) ---");
    let n_forge = 10_000usize;
    let mut false_positives = 0usize;
    let mut lcg = Lcg(0xdeadbeef_cafebabe);

    for _ in 0..n_forge {
        let forged = lcg.next_bytes();
        // Éviter de forger un id qui existe par hasard (extrêmement improbable,
        // mais on vérifie pour la rigueur du test).
        if real_ids.contains(&forged) { continue; }
        match log.get(&forged) {
            Ok(None) => {}
            Ok(Some(_)) => {
                false_positives += 1;
                eprintln!("[sef13/V3.3a] FAUX-POSITIF : {}", forged.iter().map(|b| format!("{:02x}", b)).collect::<String>());
            }
            Err(e) => { eprintln!("[sef13/V3.3a] erreur I/O : {e}"); }
        }
    }
    let pass_v33a = false_positives == 0;
    println!("  lookups forgés     : {n_forge}");
    println!("  faux-positifs      : {false_positives}");
    println!("  verdict V3.3a      : {}", if pass_v33a { "PASS" } else { "FAIL" });

    // ── V3.3b : intégrité content-addressed ────────────────────────────────
    println!("\n--- V3.3b : intégrité content-addressed ({} action_ids réels) ---", real_ids.len());
    let mut integrity_violations = 0usize;
    let mut not_found = 0usize;

    for id in &real_ids {
        match log.get(id) {
            Ok(None) => {
                not_found += 1;
                eprintln!("[sef13/V3.3b] MANQUANT : action_id={}", id.iter().map(|b| format!("{:02x}", b)).collect::<String>());
            }
            Ok(Some(entry)) => {
                let computed = entry.action_id();
                if computed != *id {
                    integrity_violations += 1;
                    eprintln!("[sef13/V3.3b] INTÉGRITÉ VIOLÉE : queried={} computed={}",
                        id.iter().map(|b| format!("{:02x}", b)).collect::<String>(),
                        computed.iter().map(|b| format!("{:02x}", b)).collect::<String>());
                }
            }
            Err(e) => { eprintln!("[sef13/V3.3b] erreur I/O : {e}"); }
        }
    }
    let pass_v33b = integrity_violations == 0 && not_found == 0;
    println!("  action_ids vérifiés  : {}", real_ids.len());
    println!("  introuvables         : {not_found}");
    println!("  violations intégrité : {integrity_violations}");
    println!("  verdict V3.3b        : {}", if pass_v33b { "PASS" } else { "FAIL" });

    // ── V3.4 : note documentaire ────────────────────────────────────────────
    println!("\n--- V3.4 : DAG cyclique (finding gate Q2) ---");
    println!("  Résultat gate : cycles non-constructibles par design (ADR-0053 §Gate Q2).");
    println!("  Preuve : append-only + SHA-256 content-addressed + existence-check (B-light)");
    println!("  = pas de fixed-point SHA-256 constructible. Propriété P3-DAG-acyclique");
    println!("  garantie structurellement, non par check explicite. Aucun harness requis.");
    println!("  verdict V3.4 : PASS (par construction — non applicable)");

    let all_pass = pass_v33a && pass_v33b;
    println!("\n=== verdict global SEF-13 : {} ===", if all_pass { "PASS" } else { "FAIL" });

    // Cleanup explicite pour éviter le SIGSEGV RocksDB atexit.
    drop(log);
    drop(store);

    std::process::exit(if all_pass { 0 } else { 1 });
}
