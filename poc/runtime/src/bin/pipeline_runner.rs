// pipeline_runner — deux agents en pipeline avec causalité cross-agent (ADR-0003).
//
// Agent A (analyse) → Agent B (synthèse, cause = action A).
// Les deux partagent ContentStore + CausalLog.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

struct Args { model: String, endpoint: String, wasm: String }
impl Default for Args {
    fn default() -> Self { Self {
        model:    "llama3.2:3b".into(),
        endpoint: "http://localhost:11434".into(),
        wasm:     "target/wasm32-unknown-unknown/release/examples/multi_turn.wasm".into(),
    }}
}
fn parse_args() -> Args {
    let mut a = Args::default();
    let raw: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--model"    => { i += 1; a.model    = raw[i].clone(); }
            "--endpoint" => { i += 1; a.endpoint = raw[i].clone(); }
            "--wasm"     => { i += 1; a.wasm     = raw[i].clone(); }
            other => { eprintln!("inconnu: {other}"); std::process::exit(1); }
        }
        i += 1;
    }
    a
}

// Retourne (texte, action_id) du dernier ActionResult après `after` entrées.
fn get_last_action_result(log: &CausalLog, id: &[u8; 16], after: usize) -> Option<(String, [u8; 32])> {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    if ids.len() <= after { return None; }
    let mut last: Option<(String, [u8; 32])> = None;
    for aid in &ids[after..] {
        let Ok(Some(e)) = log.get(aid) else { continue };
        let Some(pb) = e.emit_payload else { continue };
        let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb) else { continue };
        if env.emit_type == EmitType::ActionResult as u8 {
            last = Some((String::from_utf8_lossy(&env.payload).trim().to_string(), *aid));
        }
    }
    last
}

fn short_hex(b: &[u8; 32]) -> String {
    b.iter().take(4).map(|x| format!("{x:02x}")).collect()
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    eprintln!("=== pipeline-runner  A → B (add_cause ADR-0003) ===");
    eprintln!("modèle: {} @ {}", args.model, args.endpoint);
    eprintln!("(tapez /quit pour terminer)\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/pipeline-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng, std::path::Path::new(&args.wasm))
        .unwrap_or_else(|e| { eprintln!("WASM: {e}"); std::process::exit(1); });

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: args.model.clone(), endpoint: args.endpoint.clone() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let id_a: [u8; 16] = *b"pipeline-agent-a";
    let (tx_a, rx_a) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id_a,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("actor_a"),
        rx_a,
    ));

    let id_b: [u8; 16] = *b"pipeline-agent-b";
    let (tx_b, rx_b) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id_b,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("actor_b"),
        rx_b,
    ));

    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();

    loop {
        print!("You: ");
        std::io::stdout().flush().unwrap();
        let line = match lines.next_line().await { Ok(Some(l)) => l, _ => break };
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        if line == "/quit" { break; }

        // ── A ────────────────────────────────────────────────────────────────
        eprint!("  A");
        let before_a = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
        tx_a.send(Message::data(line.as_bytes().to_vec())).await.unwrap();

        let ra = {
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                eprint!(".");
                let _ = std::io::stderr().flush();
                if let Some(r) = get_last_action_result(&log, &id_a, before_a) { break Some(r); }
                if Instant::now() >= deadline { break None; }
            }
        };
        eprintln!();
        let Some((resp_a, action_id_a)) = ra else { eprintln!("[A timeout]"); continue };
        println!("A: {resp_a}\n");

        // ── B — ADR-0003 : cause = action_id de la dernière réponse de A ──────
        eprint!("  B");
        let prompt_b = format!("Résume en une phrase : {resp_a}");
        let before_b = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
        tx_b.send(Message::caused(prompt_b.into_bytes(), action_id_a)).await.unwrap();

        let rb = {
            let deadline = Instant::now() + Duration::from_secs(180);
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                eprint!(".");
                let _ = std::io::stderr().flush();
                if let Some(r) = get_last_action_result(&log, &id_b, before_b) { break Some(r); }
                if Instant::now() >= deadline { break None; }
            }
        };
        eprintln!();
        let Some((resp_b, action_id_b)) = rb else { eprintln!("[B timeout]"); continue };
        // Vérifier que B's LogEntry parent_ids contient bien action_id_a
        let b_entry = log.query_by_agent_range(&id_b, None, None).unwrap_or_default();
        let cause_verified = b_entry.iter().any(|aid| {
            log.get(aid).ok().flatten()
                .and_then(|e| if e.parent_ids.contains(&action_id_a) { Some(()) } else { None })
                .is_some()
        });
        eprintln!("  [causal B({})←A({}) cause_in_parents={}]",
            short_hex(&action_id_b), short_hex(&action_id_a), cause_verified);
        println!("B: {resp_b}\n");
    }

    drop(tx_a); drop(tx_b);
    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("log: {}", tmp.display());
}
