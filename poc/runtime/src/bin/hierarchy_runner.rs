// hierarchy_runner -- M : délégation hiérarchique 2 niveaux + audit DAG.
//
// Cas d'usage : equipe d'agents structuree en 2 niveaux.
//   Niveau 1 -- Manager  : recoit le brief, produit les exigences.
//   Niveau 2 -- Analysts : Analyst_sec (securite) + Analyst_perf (performance),
//               chacun causalement derive du Manager.
//   Synthese  -- Synthesizer (hierarchy_synth.wasm) : fan-in des 3 rapports,
//               produit la recommandation finale.
//
// Audit DAG final : BFS depuis la recommandation remonte 3 niveaux de causalite
// et identifie les agents responsables a chaque niveau.
//
// Proprietes demontrees :
//   P3b -- fan-in DAG : le noeud de synthese a 4 parent_ids cross-agent.
//   P3b -- audit cross-agent : BFS remonte Manager -> Analysts -> Synth.

use std::collections::{HashSet, VecDeque};
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

fn agent_label(id: &[u8; 16], id_mgr: &[u8; 16], id_sec: &[u8; 16],
               id_perf: &[u8; 16], id_synth: &[u8; 16]) -> &'static str {
    if id == id_mgr   { "[MANAGER]" }
    else if id == id_sec   { "[ANALYST-SEC]" }
    else if id == id_perf  { "[ANALYST-PERF]" }
    else if id == id_synth { "[SYNTH]" }
    else { "[?]" }
}

fn traverse_dag(
    log: &CausalLog,
    start: [u8; 32],
    id_mgr: [u8; 16], id_sec: [u8; 16], id_perf: [u8; 16], id_synth: [u8; 16],
) -> Vec<(usize, [u8; 32], [u8; 16], String)> {
    let mut visited: HashSet<[u8; 32]> = HashSet::new();
    let mut queue: VecDeque<([u8; 32], usize)> = VecDeque::new();
    let mut nodes: Vec<(usize, [u8; 32], [u8; 16], String)> = Vec::new();
    queue.push_back((start, 0));
    while let Some((aid, depth)) = queue.pop_front() {
        if visited.contains(&aid) { continue; }
        visited.insert(aid);
        let Ok(Some(entry)) = log.get(&aid) else { continue };
        let content = if let Some(pb) = &entry.emit_payload {
            if let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(pb) {
                if env.emit_type == EmitType::ActionResult as u8 {
                    String::from_utf8_lossy(&env.payload).chars().take(80).collect()
                } else { format!("[type=0x{:02x}]", env.emit_type) }
            } else { "[parse error]".into() }
        } else { "[no payload]".into() };
        for parent_id in &entry.parent_ids {
            if !visited.contains(parent_id) { queue.push_back((*parent_id, depth + 1)); }
        }
        let _ = (id_mgr, id_sec, id_perf, id_synth);
        nodes.push((depth, aid, entry.agent_id, content));
    }
    nodes.sort_by_key(|n| n.0);
    nodes
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== hierarchy-runner -- M : delegation hierarchique 2 niveaux ===");
    eprintln!("modele : {model}");
    eprintln!("Manager -> [Analyst-Sec, Analyst-Perf] -> Synthesizer");
    eprintln!();

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/hierarchy-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm_step = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let wasm_synth = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/hierarchy_synth.wasm"))
        .expect("hierarchy_synth.wasm");

    // pool_cap=3 : les 2 analystes peuvent travailler en parallele, le synth attend
    let pool = Arc::new(InferencePool::new_with_queue_params(
        3, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let id_mgr:   [u8; 16] = *b"hier-manager-000";
    let id_sec:   [u8; 16] = *b"hier-analyst-sec";
    let id_perf:  [u8; 16] = *b"hier-analyst-prf";
    let id_synth: [u8; 16] = *b"hier-synth-00000";

    let brief = "Design a high-availability banking transaction system. Requirements: 99.99% uptime, < 100ms P99 latency, PCI-DSS compliance, 10000 TPS peak load.";

    // ====================================================================
    // NIVEAU 1 : Manager analyse le brief
    // ====================================================================
    eprintln!("--- Niveau 1 : Manager ---");
    let (tx_mgr, rx_mgr) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_step, id_mgr,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
            AgentProfile::Batch,
        ).await.expect("manager actor"),
        rx_mgr,
    ));
    let bef_mgr = log.query_by_agent_range(&id_mgr, None, None).unwrap_or_default().len();
    let mgr_prompt = format!("\n---\nYou are a project manager. Summarize the key technical requirements (2-3 sentences) for: {brief}");
    tx_mgr.send(Message::data(mgr_prompt.into_bytes())).await.unwrap();
    eprint!("  [MGR]");
    let (text_mgr, act_mgr) = wait_action_result(&log, &id_mgr, bef_mgr, 120).await
        .expect("manager timeout");
    eprintln!();
    eprintln!("  action_id: {}", hex8(&act_mgr));
    for line in text_mgr.lines().take(2) { eprintln!("  {line}"); }
    drop(tx_mgr);

    // ====================================================================
    // NIVEAU 2 : Analysts en parallele, causalite depuis Manager
    // ====================================================================
    eprintln!();
    eprintln!("--- Niveau 2 : Analysts en parallele (causalite depuis Manager) ---");

    let (tx_sec,  rx_sec)  = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_perf, rx_perf) = tokio::sync::mpsc::channel::<Message>(2);

    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_step, id_sec,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("analyst sec"),
        rx_sec,
    ));
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_step, id_perf,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("analyst perf"),
        rx_perf,
    ));

    let bef_sec  = log.query_by_agent_range(&id_sec,  None, None).unwrap_or_default().len();
    let bef_perf = log.query_by_agent_range(&id_perf, None, None).unwrap_or_default().len();

    let sec_prompt  = format!("\n---\nYou are a security analyst. Based on these requirements, identify the top 3 security concerns (2-3 sentences): {text_mgr}");
    let perf_prompt = format!("\n---\nYou are a performance architect. Based on these requirements, identify the top 3 performance bottlenecks (2-3 sentences): {text_mgr}");

    // Envoyer avec causalite depuis le Manager
    tx_sec.send(Message::caused(sec_prompt.into_bytes(),   act_mgr)).await.unwrap();
    tx_perf.send(Message::caused(perf_prompt.into_bytes(), act_mgr)).await.unwrap();
    eprint!("  [SEC]"); eprint!("  [PERF]");

    let ((text_sec, act_sec), (text_perf, act_perf)) = tokio::join!(
        async { wait_action_result(&log, &id_sec,  bef_sec,  120).await.expect("sec timeout") },
        async { wait_action_result(&log, &id_perf, bef_perf, 120).await.expect("perf timeout") },
    );
    eprintln!();
    eprintln!("  [SEC]  action_id: {} cause=>{}", hex8(&act_sec),  hex8(&act_mgr));
    for line in text_sec.lines().take(2)  { eprintln!("  {line}"); }
    eprintln!("  [PERF] action_id: {} cause=>{}", hex8(&act_perf), hex8(&act_mgr));
    for line in text_perf.lines().take(2) { eprintln!("  {line}"); }
    drop(tx_sec); drop(tx_perf);

    // ====================================================================
    // SYNTHESE : fan-in des 3 rapports (Manager + Sec + Perf)
    // ====================================================================
    eprintln!();
    eprintln!("--- Synthese : fan-in causal des 3 rapports ---");

    let (tx_synth, rx_synth) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_synth, id_synth,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
            AgentProfile::Batch,
        ).await.expect("synth actor"),
        rx_synth,
    ));

    let bef_synth = log.query_by_agent_range(&id_synth, None, None).unwrap_or_default().len();

    // msg 0 : brief manager (cause = act_mgr)
    // msg 1 : rapport securite (cause = act_sec)
    // msg 2 : rapport performance (cause = act_perf) -> barrier fan-in
    eprintln!("  msg 0 : brief manager   (cause={})", hex8(&act_mgr));
    eprintln!("  msg 1 : rapport sec     (cause={})", hex8(&act_sec));
    eprintln!("  msg 2 : rapport perf    (cause={}) -> fan-in", hex8(&act_perf));
    tx_synth.send(Message::caused(text_mgr.into_bytes(),   act_mgr)).await.unwrap();
    tx_synth.send(Message::caused(text_sec.into_bytes(),   act_sec)).await.unwrap();
    tx_synth.send(Message::caused(text_perf.into_bytes(),  act_perf)).await.unwrap();
    eprint!("  [SYNTH]");

    let (text_synth, act_synth) = wait_action_result(&log, &id_synth, bef_synth, 180).await
        .expect("synth timeout");
    eprintln!();
    drop(tx_synth);

    eprintln!();
    eprintln!("=== RECOMMANDATION FINALE ===");
    eprintln!("  action_id : {}", hex8(&act_synth));
    for line in text_synth.lines().take(5) { eprintln!("  {line}"); }

    // ====================================================================
    // AUDIT DAG : BFS depuis la recommandation finale
    // ====================================================================
    eprintln!();
    eprintln!("=== AUDIT DAG (BFS depuis recommandation finale) ===");

    // Trouver l'entree fan-in du synth (celle qui a parent_ids.len() >= 3)
    let all_synth = log.query_by_agent_range(&id_synth, None, None).unwrap_or_default();
    let fan_in_id = all_synth.iter().find_map(|aid| {
        let e = log.get(aid).ok()??;
        (e.parent_ids.len() >= 3).then_some(*aid)
    }).unwrap_or(act_synth);

    let nodes = traverse_dag(&log, fan_in_id, id_mgr, id_sec, id_perf, id_synth);

    let mut last_depth = usize::MAX;
    let mut action_results_per_level: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (depth, aid, agent_id, content) in &nodes {
        if *depth != last_depth {
            eprintln!("  [Niveau {}]", depth);
            last_depth = *depth;
        }
        let role = agent_label(agent_id, &id_mgr, &id_sec, &id_perf, &id_synth);
        let preview: String = content.chars().take(60).collect();
        eprintln!("    {} {} : {}", hex8(aid), role, preview);
        if content.starts_with('[') { } else { *action_results_per_level.entry(*depth).or_default() += 1; }
    }

    eprintln!();
    eprintln!("=== ASSERTIONS ===");
    let fan_in_entry = log.get(&fan_in_id).ok().flatten();
    let n_parents = fan_in_entry.as_ref().map(|e| e.parent_ids.len()).unwrap_or(0);
    let p_fan_in   = n_parents >= 3;
    let p_dag_deep = nodes.iter().map(|n| n.0).max().unwrap_or(0) >= 2;
    let p_cross    = nodes.iter().any(|n| &n.2 == &id_sec) && nodes.iter().any(|n| &n.2 == &id_mgr);
    eprintln!("  fan-in parent_ids ({n_parents} >= 3)     : {}", if p_fan_in   { "PASS" } else { "FAIL" });
    eprintln!("  DAG profondeur >= 2 niveaux             : {}", if p_dag_deep { "PASS" } else { "FAIL" });
    eprintln!("  cross-agent : Mgr + Sec dans le DAG    : {}", if p_cross    { "PASS" } else { "FAIL" });

    let all_pass = p_fan_in && p_dag_deep && p_cross;
    eprintln!();
    if all_pass {
        eprintln!("PASS -- M : delegation hierarchique + audit DAG");
    } else {
        eprintln!("FAIL -- M : delegation hierarchique");
        std::process::exit(1);
    }
}
