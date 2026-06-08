// chain_runner — chaîne A → B → C avec causalité cross-agent (ADR-0003).
//
// A : analyse / première réponse
// B : raffine la réponse de A
// C : synthèse finale en une phrase
// Chaque agent reçoit la cause de l'agent précédent → DAG à 3 nœuds dans le log.

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

fn hex4(b: &[u8; 32]) -> String {
    b.iter().take(4).map(|x| format!("{x:02x}")).collect()
}

fn get_last(log: &CausalLog, id: &[u8; 16], after: usize) -> Option<(String, [u8; 32])> {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    if ids.len() <= after { return None; }
    let mut last = None;
    for aid in ids.iter().skip(after) {
        let Ok(Some(e)) = log.get(aid) else { continue };
        let Some(pb) = e.emit_payload else { continue };
        let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb) else { continue };
        if env.emit_type == EmitType::ActionResult as u8 {
            last = Some((String::from_utf8_lossy(&env.payload).trim().to_string(), *aid));
        }
    }
    last
}

async fn wait(log: &CausalLog, id: &[u8; 16], after: usize) -> Option<(String, [u8; 32])> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        eprint!(".");
        let _ = std::io::stderr().flush();
        if let Some(r) = get_last(log, id, after) { return Some(r); }
        if Instant::now() >= deadline { return None; }
    }
}

#[tokio::main]
async fn main() {
    let model = "llama3.2:3b";
    let endpoint = "http://localhost:11434";
    eprintln!("=== chain-runner  A → B → C (ADR-0003) ===");
    eprintln!("modèle: {model}\n(tapez /quit)\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/chain-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/multi_turn.wasm"))
        .expect("wasm");
    let pool  = Arc::new(InferencePool::new_with_queue_params(
        1, 12, 30_000,
        OllamaBackend { model: model.into(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let spawn = |id: [u8; 16]| {
        let eng = eng.clone(); let wasm = wasm.clone();
        let store = Arc::clone(&store); let log = Arc::clone(&log);
        let caps = Arc::clone(&caps); let pool = Arc::clone(&pool);
        async move {
            let (tx, rx) = tokio::sync::mpsc::channel::<Message>(4);
            tokio::spawn(os_poc_runtime::actor::run_loop(
                ActorInstance::new_precompiled_with_inference_and_profile(
                    &eng, &wasm, id, store, log, caps, vec![],
                    SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                    InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
                    AgentProfile::Batch,
                ).await.expect("actor"),
                rx,
            ));
            tx
        }
    };

    let id_a: [u8; 16] = *b"chain-agent-aaaa";
    let id_b: [u8; 16] = *b"chain-agent-bbbb";
    let id_c: [u8; 16] = *b"chain-agent-cccc";

    let tx_a = spawn(id_a).await;
    let tx_b = spawn(id_b).await;
    let tx_c = spawn(id_c).await;

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
        let ba = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
        tx_a.send(Message::data(line.as_bytes().to_vec())).await.unwrap();
        let (ra, ca) = match wait(&log, &id_a, ba).await {
            Some(x) => x, None => { eprintln!("\n[A timeout]"); continue }
        };
        eprintln!();
        println!("A: {ra}\n");

        // ── B — cause = dernière action de A ─────────────────────────────────
        eprint!("  B");
        let bb = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
        let pb = format!("Améliore cette réponse en 1-2 phrases : {ra}");
        tx_b.send(Message::caused(pb.into_bytes(), ca)).await.unwrap();
        let (rb, cb) = match wait(&log, &id_b, bb).await {
            Some(x) => x, None => { eprintln!("\n[B timeout]"); continue }
        };
        eprintln!();
        println!("B: {rb}\n");

        // ── C — cause = dernière action de B ─────────────────────────────────
        eprint!("  C");
        let bc = log.query_by_agent_range(&id_c, None, None).unwrap_or_default().len();
        let pc = format!("Résume en exactement une phrase : {rb}");
        tx_c.send(Message::caused(pc.into_bytes(), cb)).await.unwrap();
        let (rc, cc) = match wait(&log, &id_c, bc).await {
            Some(x) => x, None => { eprintln!("\n[C timeout]"); continue }
        };
        eprintln!();
        // Vérifier la chaîne de causalité C←B←A dans le log
        let b_has_a = log.query_by_agent_range(&id_b, None, None).unwrap_or_default()
            .iter().any(|aid| log.get(aid).ok().flatten()
                .map(|e| e.parent_ids.contains(&ca)).unwrap_or(false));
        let c_has_b = log.query_by_agent_range(&id_c, None, None).unwrap_or_default()
            .iter().any(|aid| log.get(aid).ok().flatten()
                .map(|e| e.parent_ids.contains(&cb)).unwrap_or(false));
        eprintln!("  [DAG: C({})←B({}) ok={c_has_b}, B({})←A({}) ok={b_has_a}]",
            hex4(&cc), hex4(&cb), hex4(&cb), hex4(&ca));
        println!("C: {rc}\n");
    }

    drop(tx_a); drop(tx_b); drop(tx_c);
    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("log: {}", tmp.display());
}
