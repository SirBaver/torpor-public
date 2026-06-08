// long_task_runner — tâche longue interruptible avec reprise depuis le log causal.
//
// Cas d'usage : un agent travaille sur une tâche en N étapes. Le système est
// interrompu (crash / eviction / maintenance) entre deux étapes. Au redémarrage,
// le runner lit les étapes déjà committées dans le log et reprend exactement là
// où il s'était arrêté — sans repasser par le LLM pour les étapes déjà faites.
//
// Propriété démontrée : P3 (traçabilité) — le log est la source de vérité des
// résultats émis. L'agent task_step.wasm est délibérément stateless : il reçoit son
// contexte injecté par le runner, qui le relit depuis le log. (Correction 2026-06-06,
// verdict architect : ce n'est PAS P1a — P1a = densité RAM, jamais mesurée ici ; et
// l'état AUTORITAIRE reste le ContentStore, pas le log, cf. ADR-0027. L'interruption
// est SIMULÉE, donc ce n'est pas non plus P6 ni de la durabilité.)
//
// Contraste avec une API LLM stateless : une interruption effacerait tout contexte.
// Ici, chaque étape committée est atomique et rejouable depuis n'importe quel point.

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

// Tâche : plan de déploiement d'un agent IA en production.
const TASK_TITLE: &str = "Deploying an AI agent to production: build a step-by-step plan";

const STEPS: &[&str] = &[
    "Step 1/4 — Identify the 3 main risks of deploying an AI agent in production. \
     Be specific (not generic). One sentence per risk.",

    "Step 2/4 — For each risk identified in step 1, propose one concrete mitigation. \
     One sentence per mitigation.",

    // Interruption simulée ici — les étapes 3+ vont relire les étapes 1+2 depuis le log

    "Step 3/4 — Define the rollback criteria: under what observable conditions should \
     the deployment be rolled back automatically? List 3 conditions.",

    "Step 4/4 — Summarize the full deployment plan in exactly 3 bullet points, \
     incorporating the risks, mitigations, and rollback criteria from previous steps.",
];

// Étape où l'interruption est simulée (après cette étape, avant la suivante)
const INTERRUPT_AFTER: usize = 1; // 0-indexed → après step 2

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

/// Lit toutes les ActionResult d'un agent depuis le log (dans l'ordre).
fn read_completed_steps(log: &CausalLog, id: &[u8; 16]) -> Vec<(String, [u8; 32])> {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    ids.iter().filter_map(|aid| {
        let e = log.get(aid).ok()??;
        let pb = e.emit_payload?;
        let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
        (env.emit_type == EmitType::ActionResult as u8).then(|| {
            (String::from_utf8_lossy(&env.payload).trim().to_string(), *aid)
        })
    }).collect()
}

/// Construit le message pour une étape : contexte des étapes précédentes + instruction.
fn build_step_message(completed: &[(String, [u8; 32])], instruction: &str) -> Vec<u8> {
    let mut msg = Vec::new();
    if !completed.is_empty() {
        for (i, (result, _)) in completed.iter().enumerate() {
            let _ = write!(msg, "Step {} result: {}\n", i + 1, result);
        }
        msg.extend_from_slice(b"\n---\n");
    }
    msg.extend_from_slice(instruction.as_bytes());
    msg
}

async fn spawn_step_actor(
    eng: &wasmtime::Engine,
    wasm: &wasmtime::Module,
    id: [u8; 16],
    store: Arc<ContentStore>,
    log: Arc<CausalLog>,
    caps: Arc<Mutex<CapabilityStore>>,
    pool: &Arc<InferencePool<OllamaBackend>>,
) -> tokio::sync::mpsc::Sender<Message> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng, wasm, id, store, log, caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("actor"),
        rx,
    ));
    tx
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== long-task-runner — tâche interruptible avec reprise depuis le log ===");
    eprintln!("modèle : {model}");
    eprintln!("tâche  : {TASK_TITLE}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/long-task-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new(
            "target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm manquant — voir Build dans le fichier source");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps  = Arc::new(Mutex::new(CapabilityStore::new()));
    let id: [u8; 16] = *b"task-agent-00000";

    let mut completed: Vec<(String, [u8; 32])> = Vec::new();
    let mut interrupted = false;

    for (step_idx, instruction) in STEPS.iter().enumerate() {
        // ── Simulation d'interruption ────────────────────────────────────────
        if step_idx == INTERRUPT_AFTER + 1 {
            eprintln!();
            eprintln!("╔══════════════════════════════════════════════════════════════╗");
            eprintln!("║          >>> INTERRUPTION SYSTÈME SIMULÉE <<<               ║");
            eprintln!("║  (crash / eviction / maintenance — RAM WASM effacée)        ║");
            eprintln!("╠══════════════════════════════════════════════════════════════╣");
            eprintln!("║  Lecture des étapes committées depuis le log causal...       ║");

            // Reconstruction du contexte depuis le log (source autoritaire)
            let from_log = read_completed_steps(&log, &id);
            eprintln!("║  {} étape(s) récupérée(s) depuis le log :", from_log.len());
            for (i, (_, action_id)) in from_log.iter().enumerate() {
                eprintln!("║    étape {} → action_id={}", i + 1, hex8(action_id));
            }
            eprintln!("╠══════════════════════════════════════════════════════════════╣");
            eprintln!("║  Reprise de la tâche à l'étape {}                             ║", step_idx + 1);
            eprintln!("╚══════════════════════════════════════════════════════════════╝");
            eprintln!();

            // Remplacer le contexte local par ce qui est dans le log
            completed = from_log;
            interrupted = true;
        }

        eprint!("Étape {}/{} ", step_idx + 1, STEPS.len());
        if interrupted && step_idx == INTERRUPT_AFTER + 1 {
            eprint!("[contexte reconstruit depuis le log] ");
        }

        let msg = build_step_message(&completed, instruction);
        let before = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();

        // task_step.wasm se termine après chaque étape — on spawne un acteur frais
        let tx = spawn_step_actor(&eng, &wasm, id,
            Arc::clone(&store), Arc::clone(&log),
            Arc::clone(&caps), &pool).await;
        tx.send(Message::data(msg)).await.unwrap();

        match wait_action_result(&log, &id, before, 240).await {
            Some((result, action_id)) => {
                eprintln!();
                eprintln!("┌─ action_id: {} ─────────────────────────────────────────", hex8(&action_id));
                for line in result.lines() {
                    eprintln!("│ {line}");
                }
                eprintln!("└───────────────────────────────────────────────────────────────\n");
                completed.push((result, action_id));
            }
            None => {
                eprintln!("\n[timeout à l'étape {}]\n", step_idx + 1);
            }
        }

        drop(tx);
        // Laisser l'acteur se terminer proprement avant le prochain spawn
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // ── Résumé final ─────────────────────────────────────────────────────────
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║                    TÂCHE COMPLÈTE                           ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    let total = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();
    eprintln!("║  {} étapes committées dans le log causal ({} entrées total) ║",
        completed.len(), total);
    eprintln!("║                                                              ║");
    eprintln!("║  Sans le log : l'interruption aurait effacé {} étapes        ║",
        INTERRUPT_AFTER + 1);
    eprintln!("║  Avec le log : reprise immédiate, zéro recomputation         ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
}
