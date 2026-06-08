// bounded_infer_runner -- L : C1 borne d'inference en action.
//
// Cas d'usage : systeme sous charge -- 3 agents tentent une inference simultanee
// avec pool_cap=2. Le 3e est mis en attente dans la file (max_waiting >= 1).
// Aucune requete n'est rejetee (total_rejected == 0) et tous terminent.
//
// Proprietes demontrees :
//   - C1 (ADR-0022) : at most 2 inferences paralleles.
//   - Liveness : la file ecoule meme quand la capacite est saturee.
//
// Contraste avec une API LLM stateless :
//   - API stateless : saturation = erreur 429 renvoyee a l'application.
//   - OS-pour-IA : saturation = file prioritaire avec backpressure controllee.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::io::Write as _;
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

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== bounded-infer-runner -- L : C1 borne d'inference sous charge ===");
    eprintln!("modele : {model}");
    eprintln!("pool_cap=2, N=3 agents lances simultanement");
    eprintln!();

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/bounded-infer-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");

    // pool_cap=2 : au plus 2 inferences simultanees (C1)
    let pool = Arc::new(InferencePool::new_with_queue_params(
        2, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let id_a: [u8; 16] = *b"bounded-agent-A0";
    let id_b: [u8; 16] = *b"bounded-agent-B0";
    let id_c: [u8; 16] = *b"bounded-agent-C0";

    // Sampler de la file en tache de fond
    let max_waiting = Arc::new(AtomicUsize::new(0));
    let max_w_clone = Arc::clone(&max_waiting);
    let pool_stat   = Arc::clone(&pool);
    let sampler = tokio::spawn(async move {
        loop {
            let stats = pool_stat.queue_stats();
            let total_waiting: usize = stats.waiting.iter().sum();
            max_w_clone.fetch_max(total_waiting, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let make_actor = |id: [u8; 16]| {
        let store = Arc::clone(&store);
        let log_r = Arc::clone(&log);
        let caps  = Arc::clone(&caps);
        let pool  = Arc::clone(&pool);
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id,
            store, log_r, caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(pool, PriorityClass::Batch),
            AgentProfile::Batch,
        )
    };

    let actor_a = make_actor(id_a).await.expect("actor A");
    let actor_b = make_actor(id_b).await.expect("actor B");
    let actor_c = make_actor(id_c).await.expect("actor C");

    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_c, rx_c) = tokio::sync::mpsc::channel::<Message>(2);

    tokio::spawn(os_poc_runtime::actor::run_loop(actor_a, rx_a));
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_b, rx_b));
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_c, rx_c));

    let bef_a = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
    let bef_b = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
    let bef_c = log.query_by_agent_range(&id_c, None, None).unwrap_or_default().len();

    eprintln!("Lancement simultane de 3 agents (pool_cap=2) :");
    eprintln!("  Agent A : summarize distributed systems fault tolerance");
    eprintln!("  Agent B : explain CAP theorem");
    eprintln!("  Agent C : describe Raft consensus");
    eprintln!();

    // Fire all 3 simultaneously -- 2 obtiennent un slot, 1 attend dans la file
    tx_a.send(Message::data(b"\n---\nSummarize in 2 sentences: distributed systems handle partial failures with circuit breakers, retry backoff, and health checks.".to_vec())).await.unwrap();
    tx_b.send(Message::data(b"\n---\nExplain the CAP theorem in 2 sentences for a distributed systems engineer.".to_vec())).await.unwrap();
    tx_c.send(Message::data(b"\n---\nDescribe the Raft consensus algorithm in 2 sentences.".to_vec())).await.unwrap();
    eprint!("Inferences en cours");

    let (res_a, res_b, res_c) = tokio::join!(
        wait_action_result(&log, &id_a, bef_a, 300),
        wait_action_result(&log, &id_b, bef_b, 300),
        wait_action_result(&log, &id_c, bef_c, 300),
    );
    eprintln!();
    sampler.abort();

    let final_stats = pool.queue_stats();
    let max_w = max_waiting.load(Ordering::Relaxed);

    drop(tx_a); drop(tx_b); drop(tx_c);

    eprintln!();
    eprintln!("=== RESULTATS ===");
    if let Some((t, aid)) = &res_a {
        eprintln!("  [A] {} : {}...", hex8(aid), t.chars().take(70).collect::<String>());
    } else { eprintln!("  [A] TIMEOUT"); }
    if let Some((t, aid)) = &res_b {
        eprintln!("  [B] {} : {}...", hex8(aid), t.chars().take(70).collect::<String>());
    } else { eprintln!("  [B] TIMEOUT"); }
    if let Some((t, aid)) = &res_c {
        eprintln!("  [C] {} : {}...", hex8(aid), t.chars().take(70).collect::<String>());
    } else { eprintln!("  [C] TIMEOUT"); }
    eprintln!();
    eprintln!("=== METRIQUES QUEUE ===");
    eprintln!("  total_admitted  : {}", final_stats.total_admitted);
    eprintln!("  total_rejected  : {}", final_stats.total_rejected);
    eprintln!("  max_waiting obs : {max_w}");
    eprintln!();
    eprintln!("=== ASSERTIONS C1 ===");
    let p_liveness  = res_a.is_some() && res_b.is_some() && res_c.is_some();
    let p_no_drop   = final_stats.total_rejected == 0;
    let p_bounded   = max_w >= 1;
    eprintln!("  A-liveness (3 agents termines)         : {}", if p_liveness  { "PASS" } else { "FAIL" });
    eprintln!("  A-no-drop  (total_rejected == 0)       : {}", if p_no_drop   { "PASS" } else { "FAIL" });
    eprintln!("  A-bounded  (max_waiting >= 1 observe)  : {}", if p_bounded   { "PASS" } else { "FAIL" });

    let all_pass = p_liveness && p_no_drop && p_bounded;
    eprintln!();
    if all_pass {
        eprintln!("PASS -- C1 borne d'inference");
    } else {
        eprintln!("FAIL -- C1 borne d'inference");
        std::process::exit(1);
    }
}
