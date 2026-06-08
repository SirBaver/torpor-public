// rollback_runner — démonstration de P2 (rollback d'état agent).
//
// Flux :
//   Tour 1 : "Mon prénom est Joey."           → seq=1, hash H1
//   Tour 2 : "Mon projet s'appelle NovOS."    → seq=2, hash H2
//   Tour 3 : "Répète tout ce que tu sais."    → seq=3, hash H3
//   Rollback → target_seq=1                   → état restauré à H1, seq=1
//   Tour 4 : "Répète tout ce que tu sais."    → réponse basée sur H1 uniquement

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

async fn wait_result(log: &CausalLog, id: &[u8; 16], after: usize, secs: u64) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        let r = ids.iter().skip(after).find_map(|aid| {
            let e = log.get(aid).ok()??;
            let pb = e.emit_payload?;
            let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
            (env.emit_type == EmitType::ActionResult as u8)
                .then(|| String::from_utf8_lossy(&env.payload).trim().to_string())
        });
        if r.is_some() { return r; }
        if Instant::now() >= deadline { return None; }
    }
}

fn current_seq(log: &CausalLog, id: &[u8; 16]) -> u64 {
    // Lit le seq courant depuis les entrées SchedulerRollback ou ActionResult
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    ids.len() as u64  // approximation pour l'affichage
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(2).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== rollback-runner — démonstration P2 ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/rollback-{ts}"));
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
    let id: [u8; 16] = *b"rollback-agent00";

    let actor = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm, id,
        Arc::clone(&store), Arc::clone(&log),
        Arc::new(Mutex::new(CapabilityStore::new())), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn(Arc::clone(&pool)),
        AgentProfile::Batch,
    ).await.expect("actor");

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx));

    // ── Tours 1–3 ────────────────────────────────────────────────────────────
    let questions = [
        "Mon prénom est Joey.",
        "Mon projet s'appelle NovOS.",
        "Répète tout ce que tu sais sur moi.",
    ];

    let mut after = 0;
    for (i, q) in questions.iter().enumerate() {
        let before = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
        tx.send(Message::data(q.as_bytes().to_vec())).await.unwrap();
        eprint!("Tour {} : {q} ", i + 1);
        match wait_result(&log, &id, before, 180).await {
            Some(r) => {
                eprintln!("\nAgent : {}\n", r);
                if i == 0 { after = before; } // mémoriser position après tour 1
            }
            None => eprintln!("\n[timeout]\n"),
        }
    }

    let entries_before_rollback = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("Log avant rollback : {} entrées\n", entries_before_rollback);

    // ── Rollback → seq=1 (état après le tour 1) ──────────────────────────────
    eprintln!(">>> ROLLBACK target_seq=1 (retour à l'état après tour 1) <<<\n");
    tx.send(Message::Rollback { target_seq: 1 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── Tour 4 : même question qu'avant rollback ──────────────────────────────
    let q4 = "Répète tout ce que tu sais sur moi.";
    let before4 = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    tx.send(Message::data(q4.as_bytes().to_vec())).await.unwrap();
    eprint!("Tour 4 (post-rollback) : {q4} ");
    match wait_result(&log, &id, before4, 180).await {
        Some(r) => eprintln!("\nAgent : {}\n", r),
        None    => eprintln!("\n[timeout]\n"),
    }

    let entries_after = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("Log après rollback+tour4 : {} entrées", entries_after);
    eprintln!("log: {}", tmp.display());

    drop(tx);
    tokio::time::sleep(Duration::from_millis(200)).await;
}
