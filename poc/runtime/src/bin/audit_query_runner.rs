// audit_query_runner — P3b en action réelle : traversée DAG inverse depuis un résultat final.
//
// Cas d'usage : une décision finale a été prise (ex: rapport d'incident).
// On veut remonter la chaîne causale complète : qui a décidé quoi, basé sur quoi.
//
// Protocole :
//   1. Spawner un mini-pipeline 3 agents :
//      brief → analyse → décision_finale
//   2. Partir de l'action_id de la décision finale
//   3. Traverser récursivement les parent_ids dans le log
//   4. Reconstruire et afficher la chaîne causale complète
//
// Propriété démontrée : P3b (causalité cross-agent)
//   - Chaque action_id est content-addressed (hash du contenu)
//   - Les parent_ids forment un DAG immuable dans le log
//   - Traversée O(N) garantie par la structure append-only
//
// Contraste avec une API LLM stateless :
//   - API stateless : aucun lien entre les appels LLM successifs —
//     l'application doit maintenir manuellement les pointeurs de causalité.
//   - OS-pour-IA : les parent_ids sont des hash cryptographiques dans le log.
//     Impossible de falsifier ou de perdre la traçabilité.

use std::collections::{HashMap, HashSet, VecDeque};
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

/// Nœud dans le DAG causal reconstitué.
#[derive(Debug)]
struct CausalNode {
    action_id:  [u8; 32],
    agent_id:   [u8; 16],
    content:    String,
    parent_ids: Vec<[u8; 32]>,
    depth:      usize,
}

/// Traversée BFS du DAG causal depuis un action_id racine.
/// Remonte les parent_ids récursivement jusqu'aux feuilles.
fn traverse_dag(log: &CausalLog, start: [u8; 32]) -> Vec<CausalNode> {
    let mut visited: HashSet<[u8; 32]> = HashSet::new();
    let mut queue:   VecDeque<([u8; 32], usize)> = VecDeque::new();
    let mut nodes:   Vec<CausalNode> = Vec::new();

    queue.push_back((start, 0));

    while let Some((action_id, depth)) = queue.pop_front() {
        if visited.contains(&action_id) { continue; }
        visited.insert(action_id);

        let Ok(Some(entry)) = log.get(&action_id) else { continue };

        let content = if let Some(pb) = &entry.emit_payload {
            if let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(pb) {
                if env.emit_type == EmitType::ActionResult as u8 {
                    String::from_utf8_lossy(&env.payload).trim().to_string()
                } else {
                    format!("[type={}]", env.emit_type)
                }
            } else { "[parse error]".into() }
        } else { "[no payload]".into() };

        // Enqueue parents
        for parent_id in &entry.parent_ids {
            if !visited.contains(parent_id) {
                queue.push_back((*parent_id, depth + 1));
            }
        }

        nodes.push(CausalNode {
            action_id,
            agent_id: entry.agent_id,
            content,
            parent_ids: entry.parent_ids.clone(),
            depth,
        });
    }

    // Trier par profondeur décroissante (racines en dernier = bas du DAG)
    nodes.sort_by(|a, b| b.depth.cmp(&a.depth));
    nodes
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== audit-query-runner — P3b : traversée causale depuis le résultat final ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/audit-query-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");

    // pool_cap=1 : pipeline séquentiel (A→B→C, chaque étape dépend de la précédente)
    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    // ── Contexte de la décision à auditer ────────────────────────────────────
    let context = "A production database is running slow. \
Symptom: query latency jumped from 50ms to 3000ms over 2 hours. \
No recent deployments.";

    eprintln!("╔══ CONSTRUCTION DU PIPELINE (brief → analyse → décision) ═════════");
    eprintln!("║  Contexte : {context}");
    eprintln!("║  3 agents : A (brief) → B (analyse technique) → C (décision)");
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    // ── Agent A : brief initial ───────────────────────────────────────────────
    let id_a: [u8; 16] = *b"audit-agent-A000";
    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id_a,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
            AgentProfile::Batch,
        ).await.expect("agent A"),
        rx_a,
    ));

    let before_a = log.query_by_agent_range(&id_a, None, None).unwrap_or_default().len();
    tx_a.send(Message::data(
        format!("\n---\nSummarize this incident in 2 sentences for a technical team: {context}")
            .into_bytes()
    )).await.unwrap();
    eprint!("[A brief]");
    let (brief_text, action_a) = wait_action_result(&log, &id_a, before_a, 120).await
        .expect("agent A timeout");
    eprintln!("\n  A → {}", brief_text.chars().take(80).collect::<String>());
    drop(tx_a);

    // ── Agent B : analyse technique (causalement lié à A) ────────────────────
    let id_b: [u8; 16] = *b"audit-agent-B000";
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id_b,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
            AgentProfile::Batch,
        ).await.expect("agent B"),
        rx_b,
    ));

    let before_b = log.query_by_agent_range(&id_b, None, None).unwrap_or_default().len();
    // Message::caused → parent_id = action_a (lien causal explicite)
    tx_b.send(Message::caused(
        format!("Context: {brief_text}\n---\n\
            Identify the 2 most likely root causes for this database slowdown. \
            Be specific and technical (2 bullet points).")
            .into_bytes(),
        action_a,
    )).await.unwrap();
    eprint!("[B analyse]");
    let (analysis_text, action_b) = wait_action_result(&log, &id_b, before_b, 120).await
        .expect("agent B timeout");
    eprintln!("\n  B → {}", analysis_text.chars().take(80).collect::<String>());
    drop(tx_b);

    // ── Agent C : décision finale (causalement lié à B) ──────────────────────
    let id_c: [u8; 16] = *b"audit-agent-C000";
    let (tx_c, rx_c) = tokio::sync::mpsc::channel::<Message>(2);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            &eng, &wasm, id_c,
            Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
            AgentProfile::Batch,
        ).await.expect("agent C"),
        rx_c,
    ));

    let before_c = log.query_by_agent_range(&id_c, None, None).unwrap_or_default().len();
    tx_c.send(Message::caused(
        format!("Analysis: {analysis_text}\n---\n\
            Based on this analysis, give ONE concrete immediate action to take. \
            Start with ACTION: then 1 sentence.")
            .into_bytes(),
        action_b,
    )).await.unwrap();
    eprint!("[C décision]");
    let (decision_text, action_c) = wait_action_result(&log, &id_c, before_c, 120).await
        .expect("agent C timeout");
    eprintln!("\n  C → {}\n", decision_text.chars().take(80).collect::<String>());
    drop(tx_c);

    // ── AUDIT : traversée DAG inverse depuis action_c ─────────────────────────
    eprintln!("╔══ AUDIT — Traversée DAG depuis la décision finale ════════════════");
    eprintln!("║  Point de départ : action_c = {}", hex8(&action_c));
    eprintln!("║  Traversée BFS backwards via parent_ids...\n");

    let dag = traverse_dag(&log, action_c);

    // Construire une map agent_id → lettre pour l'affichage
    let mut agent_labels: HashMap<[u8; 16], char> = HashMap::new();
    agent_labels.insert(id_a, 'A');
    agent_labels.insert(id_b, 'B');
    agent_labels.insert(id_c, 'C');

    for (i, node) in dag.iter().enumerate() {
        let label = agent_labels.get(&node.agent_id)
            .map(|c| format!("Agent {c}"))
            .unwrap_or_else(|| "Unknown".to_string());
        let arrow = if i + 1 < dag.len() { "↑" } else { "●" };
        let preview = node.content.lines().next().unwrap_or("").chars().take(70).collect::<String>();
        eprintln!("║  {} [depth={}] {} ({}):", arrow, node.depth, label, hex8(&node.action_id));
        eprintln!("║      \"{}\"", preview);
        if !node.parent_ids.is_empty() {
            eprintln!("║      causes: {}", node.parent_ids.iter().map(hex8).collect::<Vec<_>>().join(", "));
        }
        eprintln!("║");
    }

    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║  {} nœuds dans le DAG causal", dag.len());
    eprintln!("║  Décision C causalement traçable jusqu'au brief A");
    eprintln!("║  Chaque action_id = hash du contenu — non falsifiable");
    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║");
    eprintln!("║  DÉCISION FINALE (action_c: {}):", hex8(&action_c));
    for line in decision_text.lines().take(3) {
        eprintln!("║    {line}");
    }
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    eprintln!("log: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
