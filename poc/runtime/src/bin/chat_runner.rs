// chat_runner — runner interactif pour l'agent multi-tour.
//
// Charge multi_turn.wasm, spawn un ActorInstance avec OllamaBackend,
// puis lit stdin tour par tour et affiche les réponses depuis le CausalLog.
//
// Usage :
//   ./chat-runner [--model <model>] [--wasm <path>] [--endpoint <url>]
//
// Defaults :
//   model    : llama3.2:3b
//   endpoint : http://localhost:11434
//   wasm     : target/wasm32-unknown-unknown/release/examples/multi_turn.wasm

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

struct Args {
    model:    String,
    endpoint: String,
    wasm:     String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            model:    "llama3.2:3b".into(),
            endpoint: "http://localhost:11434".into(),
            wasm:     "target/wasm32-unknown-unknown/release/examples/multi_turn.wasm".into(),
        }
    }
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
            other => { eprintln!("arg inconnu: {other}"); std::process::exit(1); }
        }
        i += 1;
    }
    a
}

// Compte les entrées dans le log pour cet agent (utilisé pour détecter de nouveaux commits).
fn entry_count(log: &CausalLog, agent_id: &[u8; 16]) -> usize {
    log.query_by_agent_range(agent_id, None, None)
        .unwrap_or_default()
        .len()
}

// Cherche la dernière réponse (ActionResult = 0x01) après `after_count` entrées.
fn latest_action_result(log: &CausalLog, agent_id: &[u8; 16], after_count: usize) -> Option<Vec<u8>> {
    let ids = log.query_by_agent_range(agent_id, None, None).unwrap_or_default();
    if ids.len() <= after_count {
        return None;
    }
    // Parcourir les nouvelles entrées (les plus récentes en dernier)
    let mut last_result: Option<Vec<u8>> = None;
    for action_id in &ids[after_count..] {
        let Ok(Some(entry)) = log.get(action_id) else { continue };
        let Some(payload_bytes) = entry.emit_payload else { continue };
        let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&payload_bytes) else { continue };
        if env.emit_type == EmitType::ActionResult as u8 {
            last_result = Some(env.payload);
        }
    }
    last_result
}

#[tokio::main]
async fn main() {
    let args = parse_args();

    eprintln!("=== chat-runner — agent multi-tour ===");
    eprintln!("modèle   : {} @ {}", args.model, args.endpoint);
    eprintln!("wasm     : {}", args.wasm);
    eprintln!("(tapez /quit pour terminer)\n");

    // Vérifier Ollama
    let ollama_ok = std::process::Command::new("curl")
        .args(["--silent", "--max-time", "5", "--output", "/dev/null",
               "--write-out", "%{http_code}", &format!("{}/api/tags", args.endpoint)])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "200")
        .unwrap_or(false);
    if !ollama_ok {
        eprintln!("ERREUR : Ollama injoignable à {}.", args.endpoint);
        std::process::exit(1);
    }

    // Initialiser le runtime
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp_dir = std::path::PathBuf::from(format!("/tmp/chat-{ts}"));
    std::fs::create_dir_all(&tmp_dir).expect("create tmp_dir");

    let cache     = Cache::new_lru_cache(64 * 1024 * 1024);
    let store     = Arc::new(ContentStore::open(&tmp_dir.join("store"), Some(cache.clone())).unwrap());
    let log       = Arc::new(CausalLog::open(&tmp_dir.join("log"), Some(cache)).unwrap());
    let engine    = make_engine();
    let wasm_path = std::path::Path::new(&args.wasm);
    let module    = load_module_from_file(&engine, wasm_path)
        .unwrap_or_else(|e| {
            eprintln!("ERREUR chargement WASM '{}': {e}", args.wasm);
            eprintln!("Build d'abord : cargo build --target wasm32-unknown-unknown -p agent-sdk --example multi_turn --release");
            std::process::exit(1);
        });

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1,  // pool_cap : 1 slot (conversation séquentielle)
        4,
        30_000,
        OllamaBackend { model: args.model.clone(), endpoint: args.endpoint.clone() },
    ));

    let agent_id: [u8; 16] = *b"chat-agent-00001";
    let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
    let infer_fn  = InferencePool::as_infer_fn(Arc::clone(&pool));

    let actor = ActorInstance::new_precompiled_with_inference_and_profile(
        &engine, &module, agent_id,
        Arc::clone(&store), Arc::clone(&log),
        Arc::clone(&cap_store), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        infer_fn,
        AgentProfile::Batch, // 30 000 ticks × 10ms = 5 min — couvre inférence CPU llama3.2:3b
    ).await.expect("ActorInstance");

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx));

    // Boucle de conversation
    use tokio::io::AsyncBufReadExt;
    let stdin  = tokio::io::stdin();
    let mut lines = tokio::io::BufReader::new(stdin).lines();

    loop {
        print!("You: ");
        std::io::stdout().flush().unwrap();

        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            _ => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        if line == "/quit" { break; }

        let before = entry_count(&log, &agent_id);
        if tx.send(Message::data(line.into_bytes())).await.is_err() {
            eprintln!("agent terminé");
            break;
        }

        // Attendre la réponse (polling log, timeout 120s)
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut response_shown = false;
        while Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Some(resp) = latest_action_result(&log, &agent_id, before) {
                let text = String::from_utf8_lossy(&resp);
                println!("Agent:{text}\n");
                response_shown = true;
                break;
            }
        }
        if !response_shown {
            println!("Agent: [timeout]\n");
        }
    }

    drop(tx);
    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("Session terminée. Log dans {}", tmp_dir.display());
}
