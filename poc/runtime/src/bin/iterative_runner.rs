// iterative_runner — boucle draft→critique avec amélioration itérative.
//
// Cas d'usage : rédaction assistée avec validation qualité. Un agent rédige,
// un critique évalue (ACCEPT ou REVISE). Si REVISE, le draft repart avec
// le feedback comme contexte. Boucle jusqu'à ACCEPT ou max_iter atteint.
//
// Propriétés démontrées :
//   - Chaque draft et chaque critique est un ActionResult atomique dans le log
//   - La chaîne causale montre l'évolution : draft1→critique1→draft2→critique2→...
//   - Rollback possible à n'importe quel draft intermédiaire (P2)
//   - Avec une API LLM stateless : cette boucle doit être gérée manuellement,
//     sans trace des états intermédiaires ni possibilité de rollback

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

const TASK: &str = "\
Write a single paragraph press release (3-5 sentences) announcing NovOS v1.0.\n\
NovOS is an OS designed specifically for AI agents: WASM-sandboxed agents, \
causal append-only log (every action content-addressed), rollback, eviction/resume.\n\
Requirements:\n\
- Open with a hook (not 'We are pleased to announce')\n\
- Name at least 2 concrete technical differentiators\n\
- End with a forward-looking statement\n\
- Tone: technical but accessible, confident\
";

const MAX_ITER: usize = 3;

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

fn parse_verdict(text: &str) -> &'static str {
    let upper = text.trim_start().to_ascii_uppercase();
    if upper.starts_with("ACCEPT") { "ACCEPT" }
    else if upper.starts_with("REVISE") { "REVISE" }
    else { "REVISE" } // fail-safe: on révise si ambigu
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

async fn spawn_actor(
    eng: &wasmtime::Engine,
    wasm: &wasmtime::Module,
    id: [u8; 16],
    store: Arc<os_poc_store::ContentStore>,
    log: Arc<CausalLog>,
    caps: Arc<Mutex<CapabilityStore>>,
    pool: &Arc<InferencePool<OllamaBackend>>,
    priority: PriorityClass,
) -> tokio::sync::mpsc::Sender<Message> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng, wasm, id, store, log, caps, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(pool), priority),
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

    eprintln!("=== iterative-runner — boucle draft→critique (max {MAX_ITER} itérations) ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/iterative-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_draft  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let wasm_critic = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/critic_agent.wasm"))
        .expect("critic_agent.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    eprintln!("┌─ TÂCHE ────────────────────────────────────────────────────────");
    for line in TASK.lines() { eprintln!("│ {line}"); }
    eprintln!("└───────────────────────────────────────────────────────────────\n");

    let mut feedback: Option<String> = None;
    let mut last_draft_id: Option<[u8; 32]> = None;
    let mut final_draft: Option<String> = None;

    for iter in 1..=MAX_ITER {
        eprintln!("── Itération {iter}/{MAX_ITER} ─────────────────────────────────────────");

        // ── Draft ────────────────────────────────────────────────────────────
        let mut draft_id_bytes = *b"draft-agent-0000";
        draft_id_bytes[12] = b'0' + iter as u8;

        // Construire le message : tâche + feedback éventuel du critique
        let draft_msg = if let Some(ref fb) = feedback {
            format!("Task: {TASK}\n\nPrevious critique to address:\n{fb}\n---\nWrite an improved draft addressing all the critique points above.")
        } else {
            format!("{TASK}\n---\nWrite the draft.")
        };

        eprint!("  Draft");
        let before_d = log.query_by_agent_range(&draft_id_bytes, None, None).unwrap_or_default().len();
        let tx_d = spawn_actor(&eng, &wasm_draft, draft_id_bytes,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool,
            PriorityClass::Foreground).await;

        let msg = if let Some(parent_id) = last_draft_id {
            Message::caused(draft_msg.into_bytes(), parent_id)
        } else {
            Message::data(draft_msg.into_bytes())
        };
        tx_d.send(msg).await.unwrap();

        let (draft_text, draft_action_id) = match wait_action_result(&log, &draft_id_bytes, before_d, 240).await {
            Some(x) => x,
            None => { eprintln!("\n  [draft timeout]\n"); break; }
        };
        eprintln!();
        drop(tx_d);

        eprintln!("  ┌─ Draft {} (action_id: {}) ────────────────────────────", iter, hex8(&draft_action_id));
        for line in draft_text.lines() { eprintln!("  │ {line}"); }
        eprintln!("  └───────────────────────────────────────────────────────────");

        // ── Critique ─────────────────────────────────────────────────────────
        let mut critic_id_bytes = *b"critic-agent-000";
        critic_id_bytes[13] = b'0' + iter as u8;

        let critic_msg = format!("{TASK}\n---\n{draft_text}");

        eprint!("\n  Critique");
        let before_c = log.query_by_agent_range(&critic_id_bytes, None, None).unwrap_or_default().len();
        let tx_c = spawn_actor(&eng, &wasm_critic, critic_id_bytes,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), &pool,
            PriorityClass::Foreground).await;
        // Causalement lié au draft qu'il évalue
        tx_c.send(Message::caused(critic_msg.into_bytes(), draft_action_id)).await.unwrap();

        let (critique_text, _critique_id) = match wait_action_result(&log, &critic_id_bytes, before_c, 240).await {
            Some(x) => x,
            None => { eprintln!("\n  [critique timeout]\n"); break; }
        };
        eprintln!();
        drop(tx_c);

        let verdict = parse_verdict(&critique_text);
        eprintln!("  Verdict : [{verdict}]");
        let critique_body = critique_text.lines().skip(1)
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>().join("\n  │ ");
        if !critique_body.is_empty() {
            eprintln!("  │ {critique_body}");
        }
        eprintln!();

        last_draft_id = Some(draft_action_id);

        if verdict == "ACCEPT" {
            eprintln!("  >>> ACCEPTÉ à l'itération {iter} <<<");
            final_draft = Some(draft_text);
            break;
        } else {
            feedback = Some(critique_text.lines().skip(1)
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>().join("\n"));
            if iter == MAX_ITER {
                eprintln!("  >>> MAX ITÉRATIONS ATTEINT — dernier draft retenu <<<");
                final_draft = Some(draft_text);
            }
        }
    }

    // ── Résumé ────────────────────────────────────────────────────────────────
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║                      RÉSULTAT FINAL                         ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    if let Some(ref draft) = final_draft {
        for line in draft.lines() { eprintln!("║ {line}"); }
        eprintln!("╠══════════════════════════════════════════════════════════════╣");
    }
    eprintln!("║  Log : chaque draft + critique = ActionResult atomique       ║");
    eprintln!("║  Chaîne causale complète visible dans le log                 ║");
    eprintln!("║  Rollback possible à n'importe quel draft intermédiaire (P2) ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
