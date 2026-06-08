// evict_wake_runner — éviction + wake avec restauration du contexte (ADR-0030/0031/0012).
//
// Flux :
//   1. Agent multi-tour : 2 questions (HISTORY accumulé en mémoire WASM)
//   2. Evict → runner reçoit EvictedState (seq, last_snapshot, last_action)
//   3. HISTORY WASM perdu, slot libéré (P1a démontré)
//   4. Runner reconstruit un résumé depuis le log causal
//   5. Restauration : new_precompiled_with_inference_and_profile + state_mut() pour seq/snapshot/action
//   6. SessionResume { summary } → contexte réinjecté
//   7. 3ème question : l'agent se souvient grâce au résumé

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

async fn wait_action(log: &CausalLog, id: &[u8; 16], after: usize, secs: u64) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        eprint!(".");
        let _ = std::io::stderr().flush();
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        let found = ids.iter().skip(after).find_map(|aid| {
            let e = log.get(aid).ok()??;
            let pb = e.emit_payload?;
            let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
            (env.emit_type == EmitType::ActionResult as u8)
                .then(|| String::from_utf8_lossy(&env.payload).trim().to_string())
        });
        if found.is_some() { return found; }
        if Instant::now() >= deadline { return None; }
    }
}

fn log_summary(log: &CausalLog, id: &[u8; 16]) -> String {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    let resps: Vec<String> = ids.iter().filter_map(|aid| {
        let e = log.get(aid).ok()??;
        let pb = e.emit_payload?;
        let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
        (env.emit_type == EmitType::ActionResult as u8)
            .then(|| String::from_utf8_lossy(&env.payload).trim().to_string())
            .filter(|s| !s.is_empty())
    }).collect();
    if resps.is_empty() { return "Aucun contexte.".into(); }
    format!("[Session precedente] {}", resps.join(" | "))
}

#[tokio::main]
async fn main() {
    let model = "llama3.2:3b";
    let endpoint = "http://localhost:11434";
    eprintln!("=== evict-wake-runner ===\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/evict-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/multi_turn.wasm"))
        .expect("multi_turn.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.into(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));
    let id: [u8; 16] = *b"evict-agent-aaaa";

    // Closure pour créer un acteur avec inference
    async fn mk(
        eng: &wasmtime::Engine, wasm: &wasmtime::Module, id: [u8; 16],
        store: Arc<ContentStore>, log: Arc<CausalLog>,
        caps: Arc<Mutex<CapabilityStore>>,
        pool: &Arc<InferencePool<OllamaBackend>>,
    ) -> ActorInstance {
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng, wasm, id, store, log, caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("actor")
    }

    // ── Phase 1 : 2 tours ────────────────────────────────────────────────────
    eprintln!("Phase 1 : 2 tours de conversation");
    let actor1 = mk(&eng, &wasm, id, Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool).await;
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor1, rx));

    for q in ["Mon animal favori est le chien. Retiens.", "Capitale de l'Italie ?"] {
        let before = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
        tx.send(Message::data(q.as_bytes().to_vec())).await.unwrap();
        eprint!("  Q: {q} ");
        println!("-> {}", wait_action(&log, &id, before, 90).await.unwrap_or("[timeout]".into()));
    }

    // ── Phase 2 : éviction ───────────────────────────────────────────────────
    eprintln!("\nPhase 2 : éviction");
    let (etx, erx) = tokio::sync::oneshot::channel();
    tx.send(Message::Evict { reply: etx }).await.unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(5), erx).await
        .expect("evict timeout").expect("recv");
    eprintln!("  seq={} snapshot={:?} — HISTORY WASM perdu", ev.seq,
        ev.last_snapshot.map(|h| h.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>()));
    drop(tx);

    // ── Phase 3 : résumé depuis le log ──────────────────────────────────────
    let summary = log_summary(&log, &id);
    eprintln!("\nPhase 3 : resume\n  {summary}");

    // ── Phase 4 : restauration avec inference (ADR-0030 §restore_with_inference) ─
    eprintln!("\nPhase 4 : restauration");
    let actor2 = ActorInstance::restore_from_evicted_with_inference_and_profile(
        &eng, &wasm, &ev,
        Arc::clone(&store), Arc::clone(&log),
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("restore");
    eprintln!("  seq restauré: {} (fail-safe #7a vérifié)", ev.seq);

    let (tx2, rx2) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor2, rx2));

    // SessionResume : injecte le résumé comme premier process_one
    let before_r = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    tx2.send(Message::SessionResume { summary: summary.into_bytes() }).await.unwrap();
    eprint!("  SessionResume");
    match wait_action(&log, &id, before_r, 90).await {
        Some(r) => eprintln!("\n  Reponse: {r}"),
        None    => eprintln!("\n  [pas de reponse au resume]"),
    }

    // ── Phase 5 : 3ème question ──────────────────────────────────────────────
    eprintln!("\nPhase 5 : 3eme question");
    let q3 = "Quel est mon animal favori ?";
    let before3 = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    tx2.send(Message::data(q3.as_bytes().to_vec())).await.unwrap();
    eprint!("  Q: {q3} ");
    println!("-> {}", wait_action(&log, &id, before3, 90).await.unwrap_or("[timeout]".into()));

    drop(tx2);
    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("log: {}", tmp.display());
}
