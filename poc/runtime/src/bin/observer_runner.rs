// observer_runner -- O : log comme bus partage.
// Agent A et B tournent en parallele sur des questions independantes.
// Le runner lit leurs ActionResults depuis le log partage (sans canal direct),
// puis envoie les 2 rapports au monitor_agent.wasm (fan-in causal A+B).
// Propriete : le log est un bus d evenements lisible par tout agent autorise.

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
    log: &CausalLog, id: &[u8; 16], after: usize, secs: u64,
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
    eprintln!("=== observer-runner -- O : log comme bus partage ===");
    eprintln!("modele : {model}");
    eprintln!("Agent A + B en parallele -> runner lit le log -> monitor fan-in");
    eprintln!();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/observer-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm_step  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let wasm_mon = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/monitor_agent.wasm"))
        .expect("monitor_agent.wasm");
    let pool = Arc::new(InferencePool::new_with_queue_params(
        3, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));
    let id_a:   [u8; 16] = *b"observer-agt-A00";
    let id_b:   [u8; 16] = *b"observer-agt-B00";
    let id_mon: [u8; 16] = *b"observer-mon-000";
    let actor_a = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm_step, id_a,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("actor A");
    let actor_b = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm_step, id_b,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("actor B");
    let actor_mon = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm_mon, id_mon,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
        AgentProfile::Batch,
    ).await.expect("monitor actor");
    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_mon, rx_mon) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_a, rx_a));
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_b, rx_b));
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_mon, rx_mon));
    let bef_a = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
    let bef_b = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
    eprintln!("--- Agents A et B (parallele, log partage) ---");
    eprintln!("  A : top 3 challenges in AI reliability");
    eprintln!("  B : top 3 trends in AI agent frameworks");
    eprintln!();
    tx_a.send(Message::data(b"
---
List the 3 key challenges in building reliable AI systems. Be concise (3 bullet points).".to_vec())).await.unwrap();
    tx_b.send(Message::data(b"
---
List the 3 key trends in AI agent frameworks in 2024. Be concise (3 bullet points).".to_vec())).await.unwrap();
    eprint!("  [A] [B]");
    let ((text_a, act_a), (text_b, act_b)) = tokio::join!(
        async { wait_action_result(&log, &id_a, bef_a, 120).await.expect("A timeout") },
        async { wait_action_result(&log, &id_b, bef_b, 120).await.expect("B timeout") },
    );
    eprintln!();
    drop(tx_a); drop(tx_b);
    eprintln!("  [A] {} : {}...", hex8(&act_a), text_a.chars().take(60).collect::<String>());
    eprintln!("  [B] {} : {}...", hex8(&act_b), text_b.chars().take(60).collect::<String>());
    eprintln!();
    // Le runner lit les ActionResults depuis le log partage -- c est la cle de O.
    // A et B n ont pas envoye de message au monitor : le runner fait le pont.
    eprintln!("--- Runner bridge : lecture log partage -> monitor fan-in ---");
    eprintln!("  action_id A : {} (lu depuis log partage)", hex8(&act_a));
    eprintln!("  action_id B : {} (lu depuis log partage)", hex8(&act_b));
    let bef_mon = log.query_by_agent_range(&id_mon, None, None).unwrap_or_default().len();
    tx_mon.send(Message::caused(text_a.into_bytes(), act_a)).await.unwrap();
    tx_mon.send(Message::caused(text_b.into_bytes(), act_b)).await.unwrap();
    eprint!("  [MON]");
    let mon_result = wait_action_result(&log, &id_mon, bef_mon, 180).await;
    eprintln!();
    drop(tx_mon);
    eprintln!();
    eprintln!("=== RAPPORT DE SUPERVISION ===");
    let mut p_fan_in = false;
    let mut p_cross  = false;
    if let Some((text, aid)) = &mon_result {
        eprintln!("  action_id : {}", hex8(aid));
        for line in text.lines().take(4) { eprintln!("  {line}"); }
        if let Ok(Some(entry)) = log.get(aid) {
            p_fan_in = entry.parent_ids.len() >= 2;
            p_cross  = entry.parent_ids.iter().any(|p| p == &act_a)
                    && entry.parent_ids.iter().any(|p| p == &act_b);
            eprintln!("  parent_ids ({}) : {}", entry.parent_ids.len(),
                entry.parent_ids.iter().map(hex8).collect::<Vec<_>>().join(", "));
        }
    }
    eprintln!();
    eprintln!("=== ASSERTIONS ===");
    let p_monitor = mon_result.is_some();
    eprintln!("  Rapport produit                         : {}", if p_monitor { "PASS" } else { "FAIL" });
    eprintln!("  Fan-in parent_ids >= 2                  : {}", if p_fan_in  { "PASS" } else { "FAIL" });
    eprintln!("  Cross-agent (parent_A + parent_B)       : {}", if p_cross   { "PASS" } else { "FAIL" });
    let all_pass = p_monitor && p_fan_in && p_cross;
    eprintln!();
    if all_pass { eprintln!("PASS -- O : log bus partage + supervision causale"); }
    else { eprintln!("FAIL"); std::process::exit(1); }
}
