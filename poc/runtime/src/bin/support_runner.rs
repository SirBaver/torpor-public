// support_runner — support client avec routing dynamique et escalade causale.
//
// Cas d'usage : un agent de niveau 1 traite des questions clients en temps réel.
// Questions simples → réponse directe (1 inférence, Action Result dans le log).
// Questions complexes → escalade : spawn dynamique d'un spécialiste (2 inférences,
// Event + ActionResult cross-agent dans le log).
//
// Propriétés démontrées :
//   - Routing dynamique : la décision ANSWER/ESCALATE est committée dans le log
//   - Causalité cross-agent : ActionResult du spécialiste lie la réponse du triage
//   - Audit trail complet : chaque décision de routing est rejouable
//   - Spawn dynamique : le spécialiste n'existe que si la question le justifie

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

// Questions clients : mix de simples et complexes.
const QUESTIONS: &[(&str, &str)] = &[
    ("Q1", "What are your business hours and how can I reach support?"),
    ("Q2", "After your latest platform update, my production database is completely corrupted. I've lost 6 months of customer data. This is critical."),
    ("Q3", "I represent a Fortune 500 company and need to discuss an enterprise contract for 2,000 seats."),
];

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

async fn wait_emit_any(
    log: &CausalLog,
    id: &[u8; 16],
    after: usize,
    secs: u64,
) -> Option<(u8, Vec<u8>, [u8; 32])> {
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
            let t = env.emit_type;
            if t == EmitType::ActionResult as u8 || t == EmitType::Event as u8 {
                return Some((t, env.payload, *aid));
            }
        }
        if Instant::now() >= deadline { return None; }
    }
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

    eprintln!("=== support-runner — support client avec escalade causale ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/support-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_triage = load_module_from_file(&eng,
        std::path::Path::new(
            "target/wasm32-unknown-unknown/release/examples/support_triage.wasm"))
        .expect("support_triage.wasm");
    let wasm_specialist = load_module_from_file(&eng,
        std::path::Path::new(
            "target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let mut turn: u8 = 0;

    for (label, question) in QUESTIONS {
        turn += 1;
        eprintln!("┌─ {} ────────────────────────────────────────────────────────────",
            label);
        eprintln!("│ Client : {question}");
        eprintln!("└───────────────────────────────────────────────────────────────");

        // ID unique par ticket de support
        let mut triage_id = *b"support-triage00";
        triage_id[15] = turn;

        let (tx_triage, rx_triage) = tokio::sync::mpsc::channel::<Message>(4);
        tokio::spawn(os_poc_runtime::actor::run_loop(
            ActorInstance::new_precompiled_with_inference_and_profile(
                &eng, &wasm_triage, triage_id,
                Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
                AgentProfile::Batch,
            ).await.expect("triage actor"),
            rx_triage,
        ));

        let before = log.query_by_agent_range(&triage_id, None, None).unwrap_or_default().len();
        tx_triage.send(Message::data(question.as_bytes().to_vec())).await.unwrap();
        eprint!("  [Triage");

        // Attendre premier emit : ActionResult (direct) ou Event (escalade)
        let first = wait_emit_any(&log, &triage_id, before, 240).await;
        eprintln!("]");

        match first {
            None => {
                eprintln!("  [timeout]\n");
            }

            Some((etype, payload, triage_action_id))
                if etype == EmitType::ActionResult as u8 =>
            {
                // ── Réponse directe ──────────────────────────────────────────
                let answer = String::from_utf8_lossy(&payload).trim().to_string();
                eprintln!("  Routing : DIRECT (action_id: {})", hex8(&triage_action_id));
                eprintln!("  Support : {answer}\n");
            }

            Some((_, event_payload, triage_action_id)) => {
                // ── Escalade ─────────────────────────────────────────────────
                let escalate_str = String::from_utf8_lossy(&event_payload);
                // payload = "escalate:<type>:<reason>"
                let specialist_type = escalate_str
                    .strip_prefix("escalate:")
                    .and_then(|s| s.split(':').next())
                    .unwrap_or("specialist")
                    .trim()
                    .to_string();

                eprintln!("  Routing : ESCALADE → spécialiste «{}» (action_id: {})",
                    specialist_type, hex8(&triage_action_id));

                // Spawn dynamique du spécialiste (cause = action du triage)
                let mut spec_id = *b"support-spec0000";
                spec_id[14] = turn; spec_id[15] = b'S';

                let (tx_spec, rx_spec) = tokio::sync::mpsc::channel::<Message>(4);
                tokio::spawn(os_poc_runtime::actor::run_loop(
                    ActorInstance::new_precompiled_with_inference_and_profile(
                        &eng, &wasm_specialist, spec_id,
                        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
                        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
                        AgentProfile::Batch,
                    ).await.expect("specialist actor"),
                    rx_spec,
                ));

                // Message au spécialiste : contexte (type) + question originale
                let ctx = format!("You are a {specialist_type} specialist.");
                let spec_msg = format!("{ctx}\n---\n{question}");
                let before_spec = log.query_by_agent_range(&spec_id, None, None)
                    .unwrap_or_default().len();
                // Causalement lié à la décision du triage
                tx_spec.send(Message::caused(spec_msg.into_bytes(), triage_action_id))
                    .await.unwrap();
                eprint!("  [Spécialiste {specialist_type}");

                let spec_result = wait_action_result(&log, &spec_id, before_spec, 240).await;
                eprintln!("]");

                let (spec_answer, spec_action_id) = match spec_result {
                    Some(x) => x,
                    None => {
                        eprintln!("  [spécialiste timeout]\n");
                        drop(tx_triage); drop(tx_spec);
                        continue;
                    }
                };
                eprintln!("  Spécialiste ({}) : {}",
                    hex8(&spec_action_id), &spec_answer[..spec_answer.len().min(120)]);

                // Injecter la réponse du spécialiste dans le triage (phase 0x02)
                // Causalement lié à l'ActionResult du spécialiste
                let after_event = log.query_by_agent_range(&triage_id, None, None)
                    .unwrap_or_default().len();
                let mut phase2 = vec![0x02u8];
                phase2.extend_from_slice(spec_answer.as_bytes());
                tx_triage.send(Message::caused(phase2, spec_action_id)).await.unwrap();
                eprint!("  [Triage synthétise");

                let final_result = wait_action_result(&log, &triage_id, after_event, 240).await;
                eprintln!("]");

                match final_result {
                    Some((answer, final_id)) => {
                        eprintln!("  Support (action_id: {}) : {answer}\n",
                            hex8(&final_id));
                    }
                    None => eprintln!("  [synthèse timeout]\n"),
                }

                drop(tx_spec);
            }
        }

        drop(tx_triage);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // ── Résumé de l'audit trail ───────────────────────────────────────────────
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║                  AUDIT TRAIL — RÉSUMÉ                       ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  {} questions traitées                                        ║", QUESTIONS.len());
    eprintln!("║  Chaque décision ANSWER/ESCALADE est committée dans le log  ║");
    eprintln!("║  Spécialistes : spawns dynamiques, traces causales croisées ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
}
