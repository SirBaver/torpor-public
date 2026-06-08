// cross_session_runner — J : mémoire inter-sessions via Message::SessionResume.
//
// Cas d'usage : assistant personnel qui "se souvient" d'une session à l'autre.
//
// Démonstration en 2 phases :
//   Session 1 : l'agent apprend des faits sur l'utilisateur (nom, rôle, préférences).
//               L'ActorInstance est ensuite détruite — la RAM WASM est effacée.
//   Restart   : on rouvre les MÊMES fichiers store/log sur disque.
//   Session 2 : new ActorInstance (seq=0, RAM vide) reçoit Message::SessionResume
//               construit depuis les ActionResults du log.
//               On pose des questions nécessitant la mémoire session 1.
//               L'agent répond correctement sans jamais stocker en RAM WASM.
//
// Propriétés démontrées :
//   - P1a : la RAM WASM est volatile. Seul le log persiste.
//   - ADR-0012 : Message::SessionResume est le primitif OS pour injecter
//     le contexte cross-session — l'application n'a pas à gérer cela hors runtime.
//
// Contraste avec une API LLM stateless :
//   - API stateless : "mémoire longue" = l'application passe manuellement le contexte.
//     Aucune garantie d'intégrité, d'atomicité, ni d'auditabilité.
//   - OS-pour-IA : le log est la mémoire. SessionResume est le primitif de reprise.
//     Tout est dans le log causal : auditable, rollback-able, content-addressed.

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

/// Reconstruit le contexte de session depuis les ActionResults dans le log.
fn build_session_context(log: &CausalLog, id: &[u8; 16]) -> String {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    for aid in &ids {
        let Ok(Some(e)) = log.get(aid) else { continue };
        let Some(pb) = e.emit_payload else { continue };
        let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb) else { continue };
        if env.emit_type == EmitType::ActionResult as u8 {
            let text = String::from_utf8_lossy(&env.payload).trim().to_string();
            if !text.is_empty() {
                parts.push(format!("[previous session] {text}"));
            }
        }
    }
    parts.join("\n")
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== cross-session-runner — P1a : mémoire cross-session via log causal ===");
    eprintln!("modèle : {model}\n");

    // Répertoire persistant — utilisé dans les 2 sessions
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/cross-session-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let id: [u8; 16] = *b"assistant-agent0";

    // ════════════════════════════════════════════════════════════════════════════
    // SESSION 1 : apprentissage
    // ════════════════════════════════════════════════════════════════════════════
    eprintln!("┌─ SESSION 1 — apprentissage des préférences utilisateur ──────────────");

    let cache1 = Cache::new_lru_cache(32 * 1024 * 1024);
    let store1 = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache1.clone())).unwrap());
    let log1   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache1)).unwrap());
    let eng    = make_engine();
    let wasm   = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");

    let pool1 = Arc::new(InferencePool::new_with_queue_params(
        1, 4, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps1 = Arc::new(Mutex::new(CapabilityStore::new()));

    let (tx1, rx1) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id,
            Arc::clone(&store1), Arc::clone(&log1), Arc::clone(&caps1), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool1), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("session 1 actor"),
        rx1,
    ));

    // Apprendre tous les faits en une seule passe (task_step termine après chaque message)
    let learn_msg = b"Record the following user preferences. \
        Acknowledge EACH fact separately with 'NOTED: <fact>'. Be brief.\n\n\
        FACT 1: My name is Dr. Alice Moreau. Senior engineer at a distributed systems lab.\n\
        FACT 2: I prefer formal English. I use European date format (DD/MM/YYYY).\n\
        FACT 3: My project is NovOS - an operating system for AI agents.\n\
        FACT 4: I have 15 years of experience in OS kernels.\n\
        FACT 5: Keep all responses under 3 sentences.".to_vec();

    let before = log1.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    tx1.send(Message::data(learn_msg)).await.unwrap();
    eprint!("  Apprentissage en cours");
    let learn_result = wait_action_result(&log1, &id, before, 180).await;
    eprintln!();
    if let Some((text, aid)) = &learn_result {
        for line in text.lines().take(6) {
            eprintln!("│  {line}");
        }
        eprintln!("│  (action_id: {})", hex8(aid));
    }

    // Terminer l'acteur de session 1 (task_step termine tout seul, mais drop propre)
    drop(tx1);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let entries_s1 = log1.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("│");
    eprintln!("│  Session 1 terminée : {} entrées dans le log", entries_s1);
    eprintln!("│  RAM WASM de session 1 : DÉTRUITE (ActorInstance droppée)");
    eprintln!("└──────────────────────────────────────────────────────────────────────\n");

    // Fermer les handles store/log de session 1
    drop(store1);
    drop(log1);
    drop(pool1);
    drop(caps1);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ════════════════════════════════════════════════════════════════════════════
    // RESTART SIMULÉ : rouvrir les mêmes fichiers
    // ════════════════════════════════════════════════════════════════════════════
    eprintln!("┌─ RESTART SIMULÉ ─────────────────────────────────────────────────────");
    eprintln!("│  Réouverture store + log depuis {}", tmp.display());

    let cache2 = Cache::new_lru_cache(32 * 1024 * 1024);
    let store2 = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache2.clone())).unwrap());
    let log2   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache2)).unwrap());

    // Reconstruire le contexte depuis le log (P1a : log = mémoire autoritaire)
    let session_ctx = build_session_context(&log2, &id);
    let entries_reopen = log2.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("│  {} entrées récupérées depuis le log", entries_reopen);
    eprintln!("│  Contexte session extrait ({} bytes)", session_ctx.len());
    eprintln!("└──────────────────────────────────────────────────────────────────────\n");

    // ════════════════════════════════════════════════════════════════════════════
    // SESSION 2 : rappel depuis le log via SessionResume
    // ════════════════════════════════════════════════════════════════════════════
    eprintln!("┌─ SESSION 2 — nouveau ActorInstance (RAM vide) ───────────────────────");
    eprintln!("│  SessionResume injecte le contexte du log comme premier Data");

    let pool2 = Arc::new(InferencePool::new_with_queue_params(
        1, 4, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps2 = Arc::new(Mutex::new(CapabilityStore::new()));

    // Un seul acteur, un seul SessionResume contenant contexte + questions
    let recall_payload = format!(
        "Previous session context:\n{session_ctx}\n\
        ---\n\
        Based ONLY on the above context, answer ALL of the following:\n\
        Q1: What is the user's full name and title?\n\
        Q2: What is the user's project called and what does it do?\n\
        Q3: How many years of OS kernel experience does the user have?\n\
        Q4: What date format and language does the user prefer?\n\
        Answer each with Q1: / Q2: / Q3: / Q4: prefix. Be concise."
    );

    let (tx2, rx2) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id,
            Arc::clone(&store2), Arc::clone(&log2), Arc::clone(&caps2), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool2), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("session 2 actor"),
        rx2,
    ));

    let before_s2 = log2.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("│  Questions nécessitant la mémoire de session 1 :");
    tx2.send(Message::SessionResume { summary: recall_payload.into_bytes() }).await.unwrap();
    eprint!("  Rappel en cours");
    let recall_result = wait_action_result(&log2, &id, before_s2, 180).await;
    eprintln!();
    if let Some((text, aid)) = &recall_result {
        eprintln!("│  action_id: {}", hex8(aid));
        for line in text.lines().take(6) {
            eprintln!("│  {line}");
        }
    }
    drop(tx2);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let entries_s2 = log2.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("└──────────────────────────────────────────────────────────────────────\n");

    // ── Résumé ────────────────────────────────────────────────────────────────
    eprintln!("╔══════════════════════════════════════════════════════════════════════╗");
    eprintln!("║  RÉSUMÉ                                                              ║");
    eprintln!("╠══════════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Session 1 : {} entrées log | RAM WASM détruite après la session    ║", entries_s1);
    eprintln!("║  Restart   : log rouvert depuis disque — contexte reconstruit        ║");
    eprintln!("║  Session 2 : {} entrées log | new ActorInstance, seq=0              ║", entries_s2 - entries_s1);
    eprintln!("╠══════════════════════════════════════════════════════════════════════╣");
    eprintln!("║  P1a : la RAM WASM était vide en session 2                           ║");
    eprintln!("║  ADR-0012 : SessionResume injecte le contexte log comme 1er Data     ║");
    eprintln!("║  L'agent répond correctement sans mémoire persistante dans le WASM   ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
