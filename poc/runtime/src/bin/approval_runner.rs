// approval_runner — A3 en action réelle : agent propose un plan risqué,
// un revieweur IA évalue, le verdict détermine si le plan est exécuté ou bloqué.
//
// Cas d'usage : nettoyage base de données — l'agent propose de supprimer des tables
// de production. Cette opération irréversible doit être approuvée avant exécution.
//
// Flux :
//   1. Agent reçoit la tâche → génère un plan (infer) → émet provisoire → request_validation(2)
//   2. Runner détecte ValidationRequest dans le log
//   3. Runner spawne un revieweur (task_step.wasm) → évalue le risque
//   4. Runner lit le verdict du revieweur (APPROVE / REJECT)
//   5. Runner envoie Message::ValidationResponse au agent
//   6. Agent reçoit verdict → émet résultat final (APPROVED/REJECTED) → terminate
//
// Propriétés démontrées :
//   - A3 : l'agent est bloqué en AwaitingValidation jusqu'à réception du verdict
//   - P6 : plan provisoire commité avant la validation → atomic dans le log
//   - Log : plan provisoire + ValidationRequest + verdict revieweur + résultat final
//     tous causalement liés → audit trail complet de la décision
//
// Contraste avec une API LLM stateless :
//   - API stateless : aucun mécanisme pour bloquer un agent en attente d'approbation.
//     L'application doit orchestrer manuellement hors des garanties du runtime.
//   - OS-pour-IA : AwaitingValidation est un état de lifecycle garanti — l'agent
//     NE PEUT PAS procéder sans ValidationResponse. Tracé dans le log.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, ValidationVerdict, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::Cache;

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

async fn wait_emit_type(
    log: &CausalLog,
    id: &[u8; 16],
    after: usize,
    target_type: u8,
    secs: u64,
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
            if env.emit_type == target_type {
                return Some((env.payload, *aid));
            }
        }
        if Instant::now() >= deadline { return None; }
    }
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== approval-runner — A3 : validation avant action irréversible ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/approval-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_agent = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/approval_agent.wasm"))
        .expect("approval_agent.wasm");
    let wasm_step = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");

    // pool_cap=2 : agent + revieweur peuvent tourner en parallèle
    let pool = Arc::new(InferencePool::new_with_queue_params(
        2, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    // Tâche risquée : nettoyage de base de données avec DROP TABLE
    let task = b"Database cleanup: \
remove all user_sessions older than 90 days, \
drop the deprecated_api_v1 table, \
drop the temp_imports_2022 table, \
and purge orphaned records from the audit_log table";

    eprintln!("┌─ TÂCHE SOUMISE ───────────────────────────────────────────────────");
    eprintln!("│ {}", String::from_utf8_lossy(task));
    eprintln!("└───────────────────────────────────────────────────────────────────\n");

    // ── Spawn agent principal ─────────────────────────────────────────────────
    let id_agent: [u8; 16] = *b"approval-agent00";
    let (tx_agent, rx_agent) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_agent, id_agent,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("approval agent"),
        rx_agent,
    ));

    // Phase 0x01 : soumettre la tâche
    let before_agent = log.query_by_agent_range(&id_agent, None, None).unwrap_or_default().len();
    let mut msg = vec![0x01u8];
    msg.extend_from_slice(task);
    tx_agent.send(Message::data(msg)).await.unwrap();
    eprint!("Agent planifie");

    // ── Attendre le plan provisoire (ActionResult) ────────────────────────────
    let plan_result = wait_emit_type(&log, &id_agent, before_agent,
        EmitType::ActionResult as u8, 180).await;
    let (plan_payload, plan_action_id) = plan_result.expect("plan timeout");
    let plan_text = String::from_utf8_lossy(&plan_payload).trim().to_string();
    eprintln!();

    eprintln!("\n╔══ PLAN PROVISOIRE (action_id: {}) ════════════════════", hex8(&plan_action_id));
    for line in plan_text.lines().take(6) { eprintln!("║  {line}"); }
    if plan_text.lines().count() > 6 { eprintln!("║  [...]"); }
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    // ── Attendre ValidationRequest (cherche depuis before_agent — même process_one) ──
    eprint!("Agent demande validation");
    let vr = wait_emit_type(&log, &id_agent, before_agent,
        EmitType::ValidationRequest as u8, 30).await;
    if vr.is_none() {
        eprintln!("\n[ValidationRequest timeout]");
        return;
    }
    eprintln!("\n  --> Agent en AwaitingValidation (bloqué)\n");

    // ── Spawn revieweur (task_step.wasm) ─────────────────────────────────────
    let id_reviewer: [u8; 16] = *b"safety-reviewer0";
    let (tx_rev, rx_rev) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_step, id_reviewer,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
            AgentProfile::Batch,
        ).await.expect("reviewer"),
        rx_rev,
    ));

    let before_rev = log.query_by_agent_range(&id_reviewer, None, None).unwrap_or_default().len();
    let review_msg = format!(
        "You are a database safety reviewer. A database agent has proposed the following plan:\n\n\
        {plan_text}\n\
        ---\n\
        Assess this plan for safety risks. Answer starting with exactly APPROVE or REJECT \
        (uppercase, first word). Then explain in 1 sentence. \
        REJECT if the plan includes DROP TABLE, mass DELETE without WHERE clause, or irreversible bulk operations."
    );
    tx_rev.send(Message::caused(review_msg.into_bytes(), plan_action_id)).await.unwrap();
    eprint!("Revieweur evalue");

    let rev_result = wait_emit_type(&log, &id_reviewer, before_rev,
        EmitType::ActionResult as u8, 180).await;
    let (rev_payload, rev_action_id) = rev_result.expect("reviewer timeout");
    let rev_text = String::from_utf8_lossy(&rev_payload).trim().to_string();
    drop(tx_rev);
    eprintln!();

    eprintln!("╔══ VERDICT REVIEWEUR (action_id: {}) ══════════════════", hex8(&rev_action_id));
    eprintln!("║  {}", rev_text.chars().take(100).collect::<String>());
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    // ── Déterminer le verdict ─────────────────────────────────────────────────
    let verdict = if rev_text.to_uppercase().starts_with("APPROVE") {
        ValidationVerdict::Approved
    } else {
        ValidationVerdict::Rejected
    };
    let verdict_label = match verdict {
        ValidationVerdict::Approved => "APPROVED",
        ValidationVerdict::Rejected => "REJECTED",
        _                           => "TIMEOUT",
    };

    eprintln!(">>> VERDICT FINAL : {verdict_label} <<<\n");

    // ── Envoyer ValidationResponse → agent ───────────────────────────────────
    tx_agent.send(Message::ValidationResponse { verdict }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Phase 0x02 : finaliser
    let after_vr = log.query_by_agent_range(&id_agent, None, None).unwrap_or_default().len();
    tx_agent.send(Message::data(vec![0x02u8])).await.unwrap();
    eprint!("Agent finalise");

    let final_result = wait_emit_type(&log, &id_agent, after_vr,
        EmitType::ActionResult as u8, 60).await;
    eprintln!();
    drop(tx_agent);

    // ── Résultat final ────────────────────────────────────────────────────────
    eprintln!("\n╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  RÉSULTAT FINAL                                                  ║");
    eprintln!("╠══════════════════════════════════════════════════════════════════╣");
    match final_result {
        Some((payload, final_id)) => {
            let text = String::from_utf8_lossy(&payload);
            eprintln!("║  action_id : {}", hex8(&final_id));
            for line in text.lines().take(4) { eprintln!("║  {line}"); }
        }
        None => eprintln!("║  [timeout]"),
    }
    eprintln!("╠══════════════════════════════════════════════════════════════════╣");
    eprintln!("║  A3 : agent bloqué en AwaitingValidation jusqu'au verdict        ║");
    eprintln!("║  Plan provisoire + ValidationRequest + verdict = audit complet   ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
