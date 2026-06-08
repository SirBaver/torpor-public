// code_review_runner — pipeline de revue de code avec audit causal complet.
//
// Cas d'usage : deux agents LLM spécialisés collaborent sur une revue de code.
//   Agent A (code_reviewer) : reçoit un snippet → produit un rapport structuré
//   Agent B (severity_judge) : reçoit le rapport → classifie et émet un verdict
//
// Propriétés démontrées :
//   - Chaque décision est atomique dans le log (P6) — commit complet ou absent
//   - DAG causal cross-agent : le verdict du juge référence la review exacte qu'il a lue
//   - Audit trail tamper-evident : hash chain — modifier une entrée casse les suivantes
//   - Rejouable : reconstruire le contexte exact depuis le log à n'importe quel moment

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

// Snippet de code à reviewer — contient plusieurs problèmes intentionnels.
const CODE_SNIPPET: &str = r#"# auth.py — gestion d'authentification et de comptes

def login(db, username, password):
    sql = "SELECT * FROM users WHERE username='" + username + "'"
    user = db.execute(sql).fetchone()
    if user and user['password'] == password:
        return user
    return None

def transfer(db, from_id, to_id, amount):
    sender = db.execute("SELECT balance FROM accounts WHERE id=" + str(from_id)).fetchone()
    if sender['balance'] < amount:
        return False
    db.execute("UPDATE accounts SET balance=" + str(sender['balance'] - amount) +
               " WHERE id=" + str(from_id))
    db.execute("UPDATE accounts SET balance=balance+" + str(amount) +
               " WHERE id=" + str(to_id))
    return True
"#;

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

fn verdict_is_reject(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    if let Some(pos) = upper.find("VERDICT") {
        return upper[pos..].contains("REJECT");
    }
    false
}

fn dump_audit_trail(log: &CausalLog, ids: &[[u8; 16]]) {
    eprintln!("\n╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║                    AUDIT TRAIL (log causal)                 ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");

    for agent_id in ids {
        let agent_name = String::from_utf8_lossy(agent_id).trim_end_matches('\0').to_string();
        let entries = log.query_by_agent_range(agent_id, None, None).unwrap_or_default();
        for action_id in &entries {
            let Ok(Some(entry)) = log.get(action_id) else { continue };
            let etype = if let Some(ref pb) = entry.emit_payload {
                os_poc_causal_log::EmitEnvelope::from_msgpack(pb)
                    .map(|e| match e.emit_type {
                        t if t == EmitType::ActionResult as u8 => "ActionResult",
                        t if t == EmitType::Event as u8        => "Event",
                        _                                       => "Other",
                    })
                    .unwrap_or("?")
            } else {
                "Commit"
            };
            let parents = if entry.parent_ids.is_empty() {
                "genesis".to_string()
            } else {
                entry.parent_ids.iter().map(|p| hex8(p)).collect::<Vec<_>>().join(", ")
            };
            eprintln!("║  agent={:<22} hash={}  type={:<12} parent={}",
                agent_name, hex8(action_id), etype, parents);
        }
    }

    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  Garanties :                                                 ║");
    eprintln!("║   P6  — chaque tour est atomique (commit complet ou absent) ║");
    eprintln!("║   P3a — hash chain tamper-evident (modifier casse la suite) ║");
    eprintln!("║   P3b — causalité cross-agent vérifiable dans le log        ║");
    eprintln!("║   P1a — rejouable depuis n'importe quel point du log        ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== code-review-runner — pipeline de revue avec audit causal ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/code-review-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_reviewer = load_module_from_file(&eng,
        std::path::Path::new(
            "target/wasm32-unknown-unknown/release/examples/code_reviewer.wasm"))
        .expect("code_reviewer.wasm manquant — voir Build dans le fichier source");

    let wasm_judge = load_module_from_file(&eng,
        std::path::Path::new(
            "target/wasm32-unknown-unknown/release/examples/severity_judge.wasm"))
        .expect("severity_judge.wasm manquant — voir Build dans le fichier source");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let id_reviewer: [u8; 16] = *b"reviewer-agent00";
    let id_judge:    [u8; 16] = *b"judge-agent00000";

    // ── Spawn des deux agents ─────────────────────────────────────────────────
    let (tx_reviewer, rx_reviewer) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_reviewer, id_reviewer,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("reviewer actor"),
        rx_reviewer,
    ));

    let (tx_judge, rx_judge) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm_judge, id_judge,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("judge actor"),
        rx_judge,
    ));

    // ── Afficher le code à reviewer ───────────────────────────────────────────
    eprintln!("┌─ CODE À REVIEWER ─────────────────────────────────────────────");
    for line in CODE_SNIPPET.lines() {
        eprintln!("│ {line}");
    }
    eprintln!("└───────────────────────────────────────────────────────────────\n");

    // ── Phase 1 : Reviewer analyse le code ───────────────────────────────────
    eprint!("Agent reviewer analyse");
    let before_r = log.query_by_agent_range(&id_reviewer, None, None).unwrap_or_default().len();
    tx_reviewer.send(Message::data(CODE_SNIPPET.as_bytes().to_vec())).await.unwrap();

    let (review_text, review_action_id) = match wait_action_result(&log, &id_reviewer, before_r, 240).await {
        Some(x) => x,
        None => {
            eprintln!("\n[timeout reviewer]");
            return;
        }
    };
    eprintln!();

    eprintln!("┌─ RAPPORT DE REVIEW (action_id: {}) ───────────────",
        hex8(&review_action_id));
    for line in review_text.lines() {
        eprintln!("│ {line}");
    }
    eprintln!("└───────────────────────────────────────────────────────────────\n");

    // ── Phase 2 : Judge évalue la review (cause = action du reviewer) ────────
    eprint!("Agent judge évalue");
    let before_j = log.query_by_agent_range(&id_judge, None, None).unwrap_or_default().len();
    // Message causalement lié à l'ActionResult du reviewer
    tx_judge.send(Message::caused(review_text.as_bytes().to_vec(), review_action_id)).await.unwrap();

    let (verdict_text, verdict_action_id) = match wait_action_result(&log, &id_judge, before_j, 240).await {
        Some(x) => x,
        None => {
            eprintln!("\n[timeout judge]");
            return;
        }
    };
    eprintln!();

    let rejected = verdict_is_reject(&verdict_text);
    let verdict_label = if rejected { "REJECT" } else { "APPROVE" };

    eprintln!("┌─ VERDICT (action_id: {}, cause: {}) ─",
        hex8(&verdict_action_id), hex8(&review_action_id));
    for line in verdict_text.lines() {
        eprintln!("│ {line}");
    }
    eprintln!("└───────────────────────────────────────────────────────────────");
    eprintln!("\n>>> Verdict final : {verdict_label} <<<\n");

    // ── Audit trail ───────────────────────────────────────────────────────────
    dump_audit_trail(&log, &[id_reviewer, id_judge]);

    eprintln!("\nlog: {}", tmp.display());

    drop(tx_reviewer); drop(tx_judge);
    tokio::time::sleep(Duration::from_millis(200)).await;
}
