// orphan_causality_runner -- P : rollback + causalite orpheline.
// Agent A fait une recommandation -> Agent B agit dessus (lien causal).
// Rollback de A -> A produit une recommandation corrigee.
// Le log montre : action_A (originale) + SchedulerRollback (0x0B) + action_A2 (corrigee)
// ET action_B qui pointe toujours vers action_A (la decision supersedee).
// Propriete : P2 est AGENT-LOCAL. Le rollback de A ne propage pas a B.
// C est honnete : l histoire est complete, les deux timelines sont visibles.

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

async fn wait_rollback_done(
    log: &CausalLog, id: &[u8; 16], after: usize, secs: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        for aid in ids.iter().skip(after) {
            let Ok(Some(e)) = log.get(aid) else { continue };
            let Some(pb) = e.emit_payload else { continue };
            let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb) else { continue };
            if env.emit_type == EmitType::SchedulerRollback as u8 { return true; }
        }
        if Instant::now() >= deadline { return false; }
    }
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";
    eprintln!("=== orphan-causality-runner -- P : rollback + causalite orpheline ===");
    eprintln!("modele : {model}");
    eprintln!("A decide -> B agit sur A -> rollback A -> A corrige");
    eprintln!("Demonstration : P2 est agent-local, B garde son lien vers action_A.");
    eprintln!();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/orphan-causality-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    // multi_turn.wasm : ne s'arrete pas seul apres chaque message (contrairement a task_step).
    // Indispensable pour le rollback : l'agent doit etre vivant pour traiter Message::Rollback.
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/multi_turn.wasm"))
        .expect("multi_turn.wasm");
    let wasm_step = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let pool = Arc::new(InferencePool::new_with_queue_params(
        2, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));
    let id_a: [u8; 16] = *b"orphan-agent-A00";
    let id_b: [u8; 16] = *b"orphan-agent-B00";
    // ========================================================================
    // ETAPE 1 : Agent A -- recommandation initiale (brief incomplet, multi_turn)
    // Agent A reste vivant (multi_turn ne s'arrete pas) pour recevoir le Rollback.
    // ========================================================================
    eprintln!("--- Etape 1 : Agent A, brief incomplet (multi_turn.wasm) ---");
    let actor_a = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm, id_a,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("actor A");
    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_a, rx_a));
    let bef_a1 = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
    let brief_incomplete = b"You are a tech advisor. Brief: startup needs fast reads, ~10k users, budget-constrained. Recommend ONE database in one sentence starting with RECOMMENDATION:";
    tx_a.send(Message::data(brief_incomplete.to_vec())).await.unwrap();
    eprint!("  [A1]");
    let (text_a1, act_a1) = wait_action_result(&log, &id_a, bef_a1, 120).await.expect("A1 timeout");
    eprintln!();
    eprintln!("  A recommandation : {}", text_a1.chars().take(80).collect::<String>());
    eprintln!("  action_id_A : {}", hex8(&act_a1));
    // ========================================================================
    // ETAPE 2 : Agent B -- agit sur la recommandation de A (lien causal)
    // ========================================================================
    eprintln!();
    eprintln!("--- Etape 2 : Agent B agit sur decision de A (cause => action_id_A) ---");
    let actor_b = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm_step, id_b,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("actor B");
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor_b, rx_b));
    let bef_b = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
    let b_task = format!(
        "\n---\nBased on this recommendation: {}\nPlan the first 3 implementation steps in one sentence each.",
        text_a1
    );
    tx_b.send(Message::caused(b_task.into_bytes(), act_a1)).await.unwrap();
    eprint!("  [B]");
    let (text_b, act_b) = wait_action_result(&log, &id_b, bef_b, 120).await.expect("B timeout");
    eprintln!();
    drop(tx_b);
    eprintln!("  B plan (cause => {}): {}", hex8(&act_a1), text_b.chars().take(70).collect::<String>());
    eprintln!("  action_id_B : {} -> parent includes action_id_A", hex8(&act_b));
    // Verifier le lien causal de B vers A
    let b_links_to_a = log.get(&act_b).ok().flatten()
        .map(|e| e.parent_ids.iter().any(|p| p == &act_a1))
        .unwrap_or(false);
    eprintln!("  B.parent_ids includes action_id_A : {}", if b_links_to_a { "CONFIRMED" } else { "NOT FOUND" });
    // ========================================================================
    // ETAPE 3 : Rollback de A via le meme canal tx_a (agent toujours vivant)
    // ========================================================================
    eprintln!();
    eprintln!("--- Etape 3 : Rollback de A via tx_a (target_seq=0) ---");
    eprintln!("  Nouvelles contraintes : HIPAA compliance + ACID required");
    let bef_a2 = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
    tx_a.send(Message::Rollback { target_seq: 0 }).await.unwrap();
    let rolled_back = wait_rollback_done(&log, &id_a, bef_a2, 5).await;
    eprintln!("  SchedulerRollback(0x0B) dans le log : {}", if rolled_back { "CONFIRMED" } else { "NOT SEEN" });
    // ========================================================================
    // ETAPE 4 : A corrige sa recommandation avec les vraies contraintes
    // ========================================================================
    eprintln!();
    eprintln!("--- Etape 4 : Agent A, brief complet -> recommandation corrigee ---");
    let bef_a3 = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
    let brief_complete = b"You are a tech advisor. Complete brief: startup, ~10k users, HIPAA compliance mandatory, ACID transactions required, budget-constrained. Recommend ONE database in one sentence starting with RECOMMENDATION:";
    tx_a.send(Message::data(brief_complete.to_vec())).await.unwrap();
    eprint!("  [A2]");
    let (text_a2, act_a2) = wait_action_result(&log, &id_a, bef_a3, 120).await.expect("A2 timeout");
    eprintln!();
    drop(tx_a);
    eprintln!("  A recommandation corrigee : {}", text_a2.chars().take(80).collect::<String>());
    eprintln!("  action_id_A2 : {}", hex8(&act_a2));
    // ========================================================================
    // AUDIT : montrer les deux timelines et la causalite orpheline
    // ========================================================================
    eprintln!();
    eprintln!("=== AUDIT DAG -- deux timelines ===");
    let timeline_a = log.query_by_agent_range(&id_a, None, None).unwrap_or_default();
    eprintln!("  Timeline A ({} entrees) :", timeline_a.len());
    for aid in &timeline_a {
        let Ok(Some(e)) = log.get(aid) else { continue };
        let etype = e.emit_payload.as_ref().and_then(|pb| {
            os_poc_causal_log::EmitEnvelope::from_msgpack(pb).ok().map(|env| env.emit_type)
        }).unwrap_or(0);
        let label = match etype {
            0x01 => "ActionResult",
            0x05 => "Lifecycle",
            0x0b => "SchedulerRollback",
            0x0c => "InferenceRequest",
            0x0d => "InferenceResponse",
            _ => "other",
        };
        let marker = if *aid == act_a1 { " <- DECISION ORIGINALE" }
            else if *aid == act_a2 { " <- DECISION CORRIGEE" } else { "" };
        eprintln!("    {} [0x{etype:02x} {label}]{marker}", hex8(aid));
    }
    eprintln!();
    eprintln!("  B action_id_B {} parent_ids :", hex8(&act_b));
    if let Ok(Some(e)) = log.get(&act_b) {
        for p in &e.parent_ids {
            let marker = if p == &act_a1 { " <- action_A ORIGINALE (supersedee!)" } else { "" };
            eprintln!("    {}{marker}", hex8(p));
        }
    }
    eprintln!();
    eprintln!("=== ASSERTIONS ===");
    let p_rollback = rolled_back;
    let p_orphan   = b_links_to_a;
    let p_corrected = text_a2 != text_a1;
    eprintln!("  SchedulerRollback(0x0B) dans log A      : {}", if p_rollback   { "PASS" } else { "FAIL" });
    eprintln!("  B.parent_ids -> action_A (orpheline)    : {}", if p_orphan     { "PASS" } else { "FAIL" });
    eprintln!("  A recommandation differente apres rollback : {}", if p_corrected { "PASS" } else { "WARN" });
    let all_pass = p_rollback && p_orphan;
    eprintln!();
    if all_pass { eprintln!("PASS -- P : rollback agent-local + causalite orpheline"); }
    else { eprintln!("FAIL"); std::process::exit(1); }
}
