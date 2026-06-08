// watchdog_runner -- N : watchdog trap en action (ADR-0025).
// Agent boucle infinie WASM -> epoch_interruption -> AgentCrash(WatchdogTrap 0x03).
// Superviseur lit le log, produit rapport causalement lie au crash.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, ActorInstanceBuilder, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::Cache;
use wasmtime::Module;

const INFINITE_LOOP_WAT: &str =
    "(module (func (export \"process\") (param i32) (param i32) (loop $inf (br $inf))) (memory (export \"memory\") 1))";

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}
fn hex8_16(b: &[u8; 16]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

async fn wait_emit_type(
    log: &CausalLog, id: &[u8; 16], after: usize, target_type: u8, secs: u64,
) -> Option<([u8; 32], Vec<u8>)> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        for aid in ids.iter().skip(after) {
            let Ok(Some(e)) = log.get(aid) else { continue };
            let Some(pb) = e.emit_payload else { continue };
            let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb) else { continue };
            if env.emit_type == target_type { return Some((*aid, env.payload)); }
        }
        if Instant::now() >= deadline { return None; }
    }
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
    eprintln!("=== watchdog-runner -- N : watchdog trap en action (ADR-0025) ===");
    eprintln!("modele : {model} | AgentProfile::Algo ~100ms");
    eprintln!();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/watchdog-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cache = Cache::new_lru_cache(32 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let caps  = Arc::new(Mutex::new(CapabilityStore::new()));
    let id_runaway: [u8; 16] = *b"runaway-agent-N0";
    let id_monitor: [u8; 16] = *b"monitor-agent-N0";
    let module_loop = Module::new(&eng, INFINITE_LOOP_WAT).expect("compile infinite loop WAT");
    eprintln!("--- Agent runaway : boucle infinie ---");
    let actor_runaway = ActorInstanceBuilder::new(
        &eng, &module_loop, id_runaway, Arc::clone(&store), Arc::clone(&log),
    )
        .caps(Arc::clone(&caps), vec![])
        .session_max_duration_ms(0)
        .profile(AgentProfile::Algo)
        .clock(os_poc_runtime::clock::system_clock())
        .build()
        .await
        .expect("runaway actor");
    let (tx_run, rx_run) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_runaway, rx_run));
    let bef_run = log.query_by_agent_range(&id_runaway, None, None).unwrap_or_default().len();
    tx_run.send(Message::data(b"start".to_vec())).await.unwrap();
    drop(tx_run);
    eprint!("  Attente AgentCrash(WatchdogTrap 0x03)");
    let crash_result = wait_emit_type(&log, &id_runaway, bef_run, 0x13, 10).await;
    eprintln!();
    let (crash_id, crash_payload) = crash_result.expect("watchdog n a pas fire dans 10s");
    let cause_byte = crash_payload.first().copied().unwrap_or(0xFF);
    eprintln!("  AgentCrash : {} | cause=0x{cause_byte:02x} (0x03=WatchdogTrap)", hex8(&crash_id));
    let p_watchdog = cause_byte == 0x03;
    eprintln!("  WatchdogTrap confirme : {}", if p_watchdog { "PASS" } else { "FAIL" });
    eprintln!();
    eprintln!("--- Agent superviseur : rapport d incident ---");
    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 4, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let wasm_step = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let actor_mon = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm_step, id_monitor,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("monitor actor");
    let (tx_mon, rx_mon) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_mon, rx_mon));
    let incident = format!(
        "\n---\nAn AI agent ({}) crashed: infinite loop, WatchdogTrap 0x{:02x}. \
OS killed via epoch_interruption after ~100ms (AgentProfile::Algo). \
Write a 2-sentence incident report for an ops log.",
        hex8_16(&id_runaway), cause_byte
    );
    let bef_mon = log.query_by_agent_range(&id_monitor, None, None).unwrap_or_default().len();
    tx_mon.send(Message::caused(incident.into_bytes(), crash_id)).await.unwrap();
    eprint!("  Rapport en cours");
    let report_result = wait_action_result(&log, &id_monitor, bef_mon, 120).await;
    eprintln!();
    drop(tx_mon);
    eprintln!();
    eprintln!("=== RESULTATS ===");
    eprintln!("  AgentCrash : {}", hex8(&crash_id));
    let mut p_causal = false;
    if let Some((text, aid)) = &report_result {
        eprintln!("  Rapport ({}) :", hex8(aid));
        for line in text.lines().take(3) { eprintln!("    {line}"); }
        if let Ok(Some(entry)) = log.get(aid) {
            p_causal = entry.parent_ids.iter().any(|p| p == &crash_id);
        }
        eprintln!("  rapport -> AgentCrash (lien causal) : {}", if p_causal { "PASS" } else { "FAIL" });
    }
    let all_pass = p_watchdog && report_result.is_some() && p_causal;
    eprintln!();
    if all_pass { eprintln!("PASS -- N : watchdog + supervision causale"); }
    else { eprintln!("FAIL"); std::process::exit(1); }
}
