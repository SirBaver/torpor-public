// decision_correction_runner — P2 en action réelle : mauvaise décision → rollback → correction.
//
// Cas d'usage : un architecte IA conseille sur le choix d'une BDD pour un projet healthcare.
// Phase 1 : brief incomplet → recommandation inadaptée (committée dans le log).
// Nouvelles contraintes arrivent (HIPAA, ACID, SQL) → rollback à l'état post-briefing.
// Phase 2 : contraintes complètes → recommandation corrigée, committée dans le log.
//
// Propriété démontrée : P2 (rollback d'état agent)
//   - L'état WASM (historique conversation) est restauré au snapshot cible.
//   - L'agent "oublie" la mauvaise recommandation mais garde le contexte initial.
//   - Le log trace l'événement de rollback — audit complet, pas de suppression.
//
// Contraste avec une API LLM stateless :
//   - API stateless : l'app doit gérer un historique externe et retransmettre
//     sélectivement — hors garanties du runtime, propice aux incohérences.
//   - OS-pour-IA : rollback atomique, tracé dans le log, garanti par le runtime.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

fn entry_count(log: &CausalLog, id: &[u8; 16]) -> usize {
    log.query_by_agent_range(id, None, None).unwrap_or_default().len()
}

async fn wait_result(
    log: &CausalLog,
    id: &[u8; 16],
    after: usize,
    secs: u64,
) -> Option<(String, [u8; 32])> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        let r = ids.iter().skip(after).find_map(|aid| {
            let e = log.get(aid).ok()??;
            let pb = e.emit_payload?;
            let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
            (env.emit_type == EmitType::ActionResult as u8)
                .then(|| (String::from_utf8_lossy(&env.payload).trim().to_string(), *aid))
        });
        if r.is_some() { return r; }
        if Instant::now() >= deadline { return None; }
    }
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== decision-correction-runner — P2 : rollback de décision IA ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/decision-correction-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/multi_turn.wasm"))
        .expect("multi_turn.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 4, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let id: [u8; 16] = *b"decision-arch000";

    let actor = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm, id,
        Arc::clone(&store), Arc::clone(&log),
        Arc::new(Mutex::new(CapabilityStore::new())), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("actor");

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx));

    // ── Phase 1 : brief incomplet ────────────────────────────────────────────
    eprintln!("╔══ PHASE 1 — Brief incomplet ══════════════════════════════════════");

    // Tour 1 : contexte initial (brief minimal — volontairement incomplet)
    let brief = "You are a senior software architect advising a healthcare startup. \
Initial brief: startup stage, ~10,000 patients, primary need is fast patient record \
lookups by name and ID. Confirm receipt in one sentence.";

    let before_t1 = entry_count(&log, &id);
    tx.send(Message::data(brief.as_bytes().to_vec())).await.unwrap();
    eprint!("  [Tour 1 — briefing]");
    let (t1_resp, _) = wait_result(&log, &id, before_t1, 120).await
        .expect("Tour 1 timeout");
    let seq_after_t1 = entry_count(&log, &id);
    eprintln!("\n  Architecte : {}", t1_resp);
    eprintln!("  Log : {} entrées (seq=1)\n", seq_after_t1);

    // Tour 2 : recommandation basée sur brief incomplet
    let query_incomplete = "Based on the brief, recommend a database technology for this project. \
Start your response with exactly 'RECOMMENDATION:' on the first line, \
then 2-3 sentences explaining your choice.";

    let before_t2 = entry_count(&log, &id);
    tx.send(Message::data(query_incomplete.as_bytes().to_vec())).await.unwrap();
    eprint!("  [Tour 2 — recommandation (brief incomplet)]");
    let (rec_initial, rec_initial_id) = wait_result(&log, &id, before_t2, 180).await
        .expect("Tour 2 timeout");
    let seq_after_t2 = entry_count(&log, &id);
    eprintln!();

    eprintln!("\n╠══ RECOMMANDATION INITIALE (brief incomplet) ══════════════════════");
    eprintln!("║  action_id : {}", hex8(&rec_initial_id));
    for line in rec_initial.lines().take(6) { eprintln!("║  {line}"); }
    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║  Log : {} entrées (seq=2)", seq_after_t2);
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    // ── Rollback : nouvelles contraintes reçues ──────────────────────────────
    eprintln!("┌───────────────────────────────────────────────────────────────────");
    eprintln!("│  >>> NOUVELLES CONTRAINTES REÇUES — ROLLBACK <<<");
    eprintln!("│  Contraintes manquantes découvertes après review légale/technique :");
    eprintln!("│    • HIPAA compliance (chiffrement, audit logs obligatoires)");
    eprintln!("│    • ACID transactions (billing, prescriptions)");
    eprintln!("│    • Requêtes SQL complexes (joins multi-tables, reporting)");
    eprintln!("│    • Modèle relationnel structuré");
    eprintln!("│");
    eprintln!("│  La recommandation initiale est inadaptée → rollback target_seq=1");
    eprintln!("│  (restauration à l'état post-briefing, avant la recommandation)");
    eprintln!("└───────────────────────────────────────────────────────────────────\n");

    tx.send(Message::Rollback { target_seq: 1 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;

    let seq_after_rollback = entry_count(&log, &id);
    eprintln!("  Log après rollback : {} entrées (entrée de rollback tracée)", seq_after_rollback);
    eprintln!("  État agent restauré à seq=1 — historique WASM tronqué\n");

    // ── Phase 2 : recommandation avec contraintes complètes ──────────────────
    eprintln!("╔══ PHASE 2 — Contraintes complètes ════════════════════════════════");

    let query_complete = "MANDATORY REQUIREMENTS UPDATE — the following constraints are non-negotiable:\n\
1. HIPAA compliance: all patient data encrypted at rest and in transit, full audit logs required\n\
2. ACID transactions: billing and prescription records require strict consistency guarantees\n\
3. Complex queries: multi-table joins for patient history, appointments, billing, and regulatory reports\n\
4. Structured relational schema across: patients, appointments, billing, prescriptions, audit_log\n\n\
With these complete requirements, revise your database recommendation. \
Start your response with exactly 'RECOMMENDATION:' on the first line, \
then 2-3 sentences explaining why your choice satisfies all constraints.";

    let before_t3 = entry_count(&log, &id);
    tx.send(Message::data(query_complete.as_bytes().to_vec())).await.unwrap();
    eprint!("  [Tour 3 — recommandation (contraintes complètes)]");
    let (rec_corrected, rec_corrected_id) = match wait_result(&log, &id, before_t3, 240).await {
        Some(x) => x,
        None => { eprintln!("\n[timeout]"); return; }
    };
    let seq_after_t3 = entry_count(&log, &id);
    eprintln!();

    eprintln!("\n╠══ RECOMMANDATION CORRIGÉE (contraintes complètes) ════════════════");
    eprintln!("║  action_id : {}", hex8(&rec_corrected_id));
    for line in rec_corrected.lines().take(6) { eprintln!("║  {line}"); }
    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║  Log : {} entrées (seq=3)", seq_after_t3);
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    // ── Bilan ─────────────────────────────────────────────────────────────────
    let rec_initial_short  = rec_initial.lines().next().unwrap_or("").chars().take(60).collect::<String>();
    let rec_corrected_short = rec_corrected.lines().next().unwrap_or("").chars().take(60).collect::<String>();

    eprintln!("╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  P2 — ROLLBACK DE DÉCISION IA                                  ║");
    eprintln!("╠══════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Avant rollback  : {:<50} ║", rec_initial_short);
    eprintln!("║  Après rollback  : {:<50} ║", rec_corrected_short);
    eprintln!("╠══════════════════════════════════════════════════════════════════╣");
    eprintln!("║  État agent (WASM HISTORY) restauré à seq=1                    ║");
    eprintln!("║  Mauvaise recommandation effacée de la mémoire agent            ║");
    eprintln!("║  Log : rollback tracé (append-only — audit complet)            ║");
    eprintln!("║  Garantie P2 : pas de reconstruction manuelle d'historique      ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());

    drop(tx);
    tokio::time::sleep(Duration::from_millis(200)).await;
}
