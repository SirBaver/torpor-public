// supervisor_runner — pattern worker/superviseur (A3 request_validation).
//
// Flux par question :
//   1. Runner spawne un Worker A frais, lui envoie [0x01 + question]
//   2. Worker A : infer → émet réponse provisoire (ActionResult) → request_validation
//   3. Runner détecte ValidationRequest (0x08) dans le log de A
//   4. Runner envoie la réponse provisoire de A au Superviseur B
//   5. Superviseur B : infer pour évaluer → émet verdict (0x00=ok / 0x01=reject)
//   6. Runner envoie Message::ValidationResponse { verdict } à A
//   7. Worker A : lit verdict via get_verdict() → émet réponse finale → terminate
//   8. Runner affiche la réponse finale

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, ValidationVerdict, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

struct Args { model: String, endpoint: String }
impl Default for Args {
    fn default() -> Self { Self {
        model:    "llama3.2:3b".into(),
        endpoint: "http://localhost:11434".into(),
    }}
}
fn parse_args() -> Args {
    let mut a = Args::default();
    let raw: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--model"    => { i += 1; a.model = raw[i].clone(); }
            "--endpoint" => { i += 1; a.endpoint = raw[i].clone(); }
            other => { eprintln!("inconnu: {other}"); std::process::exit(1); }
        }
        i += 1;
    }
    a
}

// Poll jusqu'à trouver un emit parmi `types`, retourne (payload, action_id).
async fn wait_emit(
    log: &CausalLog, id: &[u8; 16], after: usize,
    types: &[u8], secs: u64,
) -> Option<(Vec<u8>, [u8; 32])> {
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
            if types.contains(&env.emit_type) { return Some((env.payload, *aid)); }
        }
        if Instant::now() >= deadline { return None; }
    }
}

// Spawn un worker frais et retourne son tx.
async fn spawn_worker(
    eng: &wasmtime::Engine, wasm: &wasmtime::Module,
    id: [u8; 16],
    store: Arc<os_poc_store::ContentStore>, log: Arc<CausalLog>,
    caps: Arc<Mutex<CapabilityStore>>,
    pool: Arc<InferencePool<OllamaBackend>>,
) -> tokio::sync::mpsc::Sender<Message> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng, wasm, id, store, log, caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("worker spawn"),
        rx,
    ));
    tx
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    eprintln!("=== supervisor-runner  Worker → Superviseur → ValidationResponse ===");
    eprintln!("modèle: {} @ {}", args.model, args.endpoint);
    eprintln!("(tapez /quit pour terminer)\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/supervisor-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_w = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/llm_worker.wasm"))
        .unwrap_or_else(|e| { eprintln!("worker wasm: {e}"); std::process::exit(1); });
    let wasm_s = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/llm_supervisor.wasm"))
        .unwrap_or_else(|e| { eprintln!("supervisor wasm: {e}"); std::process::exit(1); });

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: args.model.clone(), endpoint: args.endpoint.clone() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    // Superviseur B — persistant, réutilisé pour toutes les questions
    let id_b: [u8; 16] = *b"supervisor-bbbbb";
    let (tx_b, rx_b) = tokio::sync::mpsc::channel(8);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_s, id_b,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
            AgentProfile::Batch,
        ).await.expect("supervisor"),
        rx_b,
    ));

    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut turn = 0u32;

    loop {
        print!("You: ");
        std::io::stdout().flush().unwrap();
        let line = match lines.next_line().await { Ok(Some(l)) => l, _ => break };
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        if line == "/quit" { break; }
        turn += 1;

        // Worker A : frais à chaque question (il terminate après phase 2)
        // agent_id unique par turn pour éviter les collisions dans le log
        let mut id_a = [0x41u8; 16]; // 'A' × 16
        id_a[15] = (turn & 0xFF) as u8;
        let tx_a = spawn_worker(&eng, &wasm_w, id_a,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), Arc::clone(&pool)).await;

        // ── 1. Question → Worker ──────────────────────────────────────────────
        let before_a = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
        let mut payload = vec![0x01u8];
        payload.extend_from_slice(line.as_bytes());
        tx_a.send(Message::data(payload)).await.unwrap();
        eprint!("  [Worker génère");

        // ── 2. Attendre ValidationRequest (0x08) du worker ────────────────────
        let vr = wait_emit(&log, &id_a, before_a,
            &[EmitType::ValidationRequest as u8], 90).await;
        if vr.is_none() { eprintln!("]\n[Worker timeout]"); continue; }
        eprintln!("]");

        // Lire la réponse provisoire du worker
        let provisional = log.query_by_agent_range(&id_a, None, None)
            .unwrap_or_default().into_iter().skip(before_a)
            .find_map(|aid| {
                let e = log.get(&aid).ok()??;
                let pb = e.emit_payload?;
                let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
                (env.emit_type == EmitType::ActionResult as u8)
                    .then(|| String::from_utf8_lossy(&env.payload).trim().to_string())
            })
            .unwrap_or_else(|| "[vide]".into());

        eprintln!("  Provisoire: {provisional}");

        // ── 3. Provisoire → Superviseur ───────────────────────────────────────
        let before_b = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
        tx_b.send(Message::data(provisional.as_bytes().to_vec())).await.unwrap();
        eprint!("  [Superviseur évalue");

        // ── 4. Attendre verdict du superviseur ────────────────────────────────
        let sv = wait_emit(&log, &id_b, before_b,
            &[EmitType::ActionResult as u8], 90).await;
        eprintln!("]");

        let (verdict_payload, _) = sv.unwrap_or_else(|| {
            eprintln!("  [Superviseur timeout → approuve par défaut]");
            (vec![0x00], [0u8; 32])
        });
        let verdict_byte = *verdict_payload.first().unwrap_or(&0x00);
        let verdict = if verdict_byte == 0 { ValidationVerdict::Approved } else { ValidationVerdict::Rejected };
        let verdict_label = if verdict_byte == 0 { "✓ APPROUVÉ" } else { "✗ REJETÉ" };
        eprintln!("  Verdict: {verdict_label}");

        // ── 5. ValidationResponse → Worker ────────────────────────────────────
        tx_a.send(Message::ValidationResponse { verdict }).await.unwrap();
        // Apres ValidationResponse, envoyer [0x02] pour declencher phase_apply_verdict().
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx_a.send(Message::data(vec![0x02u8])).await.unwrap();
        eprint!("  [Worker finalise");

        let after_vr = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
        let fr = wait_emit(&log, &id_a, after_vr,
            &[EmitType::ActionResult as u8], 30).await;
        eprintln!("]");

        match fr {
            Some((payload, _)) => println!("Worker (tour {turn}): {}\n",
                String::from_utf8_lossy(&payload).trim()),
            None => eprintln!("[Worker phase 2 timeout]\n"),
        }
    }

    drop(tx_b);
    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("log: {}", tmp.display());
}
