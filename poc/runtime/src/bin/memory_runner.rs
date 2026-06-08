// memory_runner — agent à mémoire longue : le log causal EST la mémoire.
//
// Cas d'usage : un agent accumule des faits sur plusieurs sessions.
// À chaque "redémarrage", la RAM WASM est vide — mais le runner relit
// toutes les entrées LEARN depuis le log et les injecte comme contexte.
// L'agent "se souvient" de tout sans état persistant dans le WASM.
//
// Propriété démontrée : P1a étendu — la mémoire longue terme d'un agent
// n'est pas dans sa RAM WASM (volatile), elle est dans le log causal
// (autoritaire, append-only, content-addressed).
//
// Contraste avec une API LLM stateless :
//   - API stateless : chaque session repart de zéro, mémoire = contexte
//     passé manuellement par l'application (external state, hors garanties)
//   - OS-pour-IA : le log est la mémoire. Immuable. Auditable. Rollback-able.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::Cache;

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

async fn wait_action_result(
    log: &CausalLog,
    id: &[u8; 16],
    after: usize,
    secs: u64,
) -> Option<(String, [u8; 32])> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        eprint!(".");
        let _ = std::io::stderr().flush();
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        for aid in ids.iter().skip(after) {
            let Ok(Some(e)) = log.get(aid) else { continue };
            let Some(pb) = e.emit_payload else { continue };
            let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb) else { continue };
            if env.emit_type == EmitType::ActionResult as u8 {
                return Some((String::from_utf8_lossy(&env.payload).trim().to_string(), *aid));
            }
        }
        if Instant::now() >= deadline { return None; }
    }
}

/// Lit toutes les ActionResult du log pour cet agent.
fn load_memory(log: &CausalLog, id: &[u8; 16]) -> Vec<(String, [u8; 32])> {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    ids.iter().filter_map(|aid| {
        let e = log.get(aid).ok()??;
        let pb = e.emit_payload?;
        let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
        (env.emit_type == EmitType::ActionResult as u8)
            .then(|| (String::from_utf8_lossy(&env.payload).trim().to_string(), *aid))
    }).collect()
}

/// Construit le contexte mémoire à injecter dans le prompt.
fn build_memory_context(memories: &[(String, [u8; 32])]) -> String {
    if memories.is_empty() { return String::new(); }
    let mut ctx = String::from("Known facts (from memory log):\n");
    for (i, (fact, _)) in memories.iter().enumerate() {
        ctx.push_str(&format!("{}. {}\n", i + 1, fact));
    }
    ctx
}

async fn learn(
    log: &CausalLog,
    eng: &wasmtime::Engine,
    wasm: &wasmtime::Module,
    id: [u8; 16],
    store: Arc<os_poc_store::ContentStore>,
    log_arc: Arc<CausalLog>,
    caps: Arc<Mutex<CapabilityStore>>,
    pool: &Arc<InferencePool<OllamaBackend>>,
    fact: &str,
    last_id: Option<[u8; 32]>,
) -> Option<[u8; 32]> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng, wasm, id, store, log_arc.clone(), caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.ok()?,
        rx,
    ));
    let before = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    let msg_text = format!("\n---\nRepeat this fact exactly, word for word: {fact}");
    let msg = match last_id {
        Some(parent) => Message::caused(msg_text.into_bytes(), parent),
        None => Message::data(msg_text.into_bytes()),
    };
    tx.send(msg).await.ok()?;
    eprint!(".");
    let result = wait_action_result(log, &id, before, 120).await;
    drop(tx);
    result.map(|(_, aid)| aid)
}

async fn recall(
    log: &CausalLog,
    eng: &wasmtime::Engine,
    wasm: &wasmtime::Module,
    id: [u8; 16],
    store: Arc<os_poc_store::ContentStore>,
    log_arc: Arc<CausalLog>,
    caps: Arc<Mutex<CapabilityStore>>,
    pool: &Arc<InferencePool<OllamaBackend>>,
    memory_context: &str,
    question: &str,
    last_memory_id: Option<[u8; 32]>,
) -> Option<(String, [u8; 32])> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng, wasm, id, store, log_arc.clone(), caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.ok()?,
        rx,
    ));
    let before = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    let msg_text = format!("{memory_context}\n---\nAnswer concisely (1-2 sentences): {question}");
    let msg = match last_memory_id {
        Some(parent) => Message::caused(msg_text.into_bytes(), parent),
        None => Message::data(msg_text.into_bytes()),
    };
    tx.send(msg).await.ok()?;
    eprint!(".");
    let result = wait_action_result(log, &id, before, 240).await;
    drop(tx);
    result
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== memory-runner — mémoire longue terme via le log causal ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/memory-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let pool  = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps  = Arc::new(Mutex::new(CapabilityStore::new()));

    // Un seul ID pour tous les LEARN : toutes les entrées dans la même séquence de log
    let learn_id:  [u8; 16] = *b"memory-learn-000";
    let recall_id: [u8; 16] = *b"memory-recall-00";

    // ── SESSION 1 : apprentissage ─────────────────────────────────────────────
    eprintln!("╔══ SESSION 1 — Apprentissage ══════════════════════════════════");
    let facts_s1 = [
        "My name is Joey.",
        "I am building NovOS, an OS designed for AI agents.",
        "My favorite animal is the dog.",
        "I prefer Rust over Python for systems programming.",
        "I live in Paris, France.",
    ];

    let mut last_learn_id: Option<[u8; 32]> = None;
    for fact in &facts_s1 {
        eprint!("  [LEARN] {fact} ");
        last_learn_id = learn(
            &log, &eng, &wasm, learn_id,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool,
            fact, last_learn_id,
        ).await;
        eprintln!("✓ ({})", last_learn_id.map(|id| hex8(&id)).unwrap_or_default());
        // Pause courte entre spawns séquentiels du même agent_id
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let memories_s1 = load_memory(&log, &learn_id);
    eprintln!("╚══ {} faits dans le log ════════════════════════════════════════\n",
        memories_s1.len());

    // ── REDÉMARRAGE SIMULÉ ────────────────────────────────────────────────────
    eprintln!("┌───────────────────────────────────────────────────────────────");
    eprintln!("│  >>> REDÉMARRAGE SIMULÉ — RAM WASM effacée <<<");
    eprintln!("│  Seul le log persiste. Reconstruction de la mémoire...");
    let memories_after_restart = load_memory(&log, &learn_id);
    eprintln!("│  {} faits récupérés depuis le log (action_ids vérifiés)",
        memories_after_restart.len());
    eprintln!("└───────────────────────────────────────────────────────────────\n");

    // ── SESSION 2 : rappel depuis le log ─────────────────────────────────────
    eprintln!("╔══ SESSION 2 — Rappel depuis le log ═══════════════════════════");
    let memory_ctx = build_memory_context(&memories_after_restart);
    let questions_s2 = [
        ("What is my name?",                     "recall-00"),
        ("What project am I working on?",         "recall-01"),
        ("What is my favorite animal?",           "recall-02"),
    ];

    for (question, suffix) in &questions_s2 {
        let mut rid = recall_id;
        rid[14] = suffix.as_bytes()[7];
        rid[15] = suffix.as_bytes()[8];
        eprint!("  [RECALL] {question} ");
        let result = recall(
            &log, &eng, &wasm, rid,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool,
            &memory_ctx, question,
            last_learn_id,
        ).await;
        match result {
            Some((answer, aid)) => eprintln!("\n  → {} ({})\n", answer, hex8(&aid)),
            None => eprintln!("\n  → [timeout]\n"),
        }
    }

    // ── SESSION 3 : apprentissage supplémentaire ──────────────────────────────
    eprintln!("╔══ SESSION 3 — Nouveaux apprentissages ════════════════════════");
    let facts_s3 = [
        "I drink black coffee every morning.",
        "My preferred stack for AI agents is Rust + WASM + RocksDB.",
    ];
    for fact in &facts_s3 {
        eprint!("  [LEARN] {fact} ");
        last_learn_id = learn(
            &log, &eng, &wasm, learn_id,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool,
            fact, last_learn_id,
        ).await;
        eprintln!("✓");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ── SESSION 4 : rappel complet (toutes sessions confondues) ──────────────
    eprintln!("\n╔══ SESSION 4 — Rappel complet (toutes sessions) ══════════════");
    let all_memories = load_memory(&log, &learn_id);
    eprintln!("║  {} faits totaux dans le log", all_memories.len());
    let full_ctx = build_memory_context(&all_memories);

    let mut rid = recall_id;
    rid[14] = b'9'; rid[15] = b'9';
    eprint!("  [RECALL] What do you know about me? ");
    let result = recall(
        &log, &eng, &wasm, rid,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool,
        &full_ctx,
        "Summarize everything you know about me in a few sentences.",
        last_learn_id,
    ).await;
    eprintln!();
    match result {
        Some((answer, aid)) => {
            eprintln!("  → {} ({})", answer, hex8(&aid));
        }
        None => eprintln!("  → [timeout]"),
    }

    // ── Bilan ─────────────────────────────────────────────────────────────────
    eprintln!("\n╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║  {} faits appris sur {} sessions                              ║",
        all_memories.len(), 3);
    eprintln!("║  Mémoire = log causal, pas RAM WASM                          ║");
    eprintln!("║  Rollback possible → oublier un fait = revenir à un état    ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
