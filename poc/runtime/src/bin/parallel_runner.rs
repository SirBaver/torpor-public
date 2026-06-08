// parallel_runner — N agents en parallèle, pool_cap=2, log causal partagé.
//
// Spawne N agents simultanément, chacun avec une question différente.
// Le pool d'inférence laisse 2 exécutions concurrentes (PriorityClass variées).
// Mesure : temps total vs séquentiel, nombre d'admissions concurrentes.

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

#[tokio::main]
async fn main() {
    let model    = "llama3.2:3b";
    let endpoint = "http://localhost:11434";
    let pool_cap: usize = 2;

    let questions: &[(&str, PriorityClass)] = &[
        ("Capitale de la France ?",  PriorityClass::Supervisor),
        ("Capitale de l'Italie ?",   PriorityClass::Supervisor),
        ("Capitale de l'Espagne ?",  PriorityClass::Foreground),
        ("Capitale du Portugal ?",   PriorityClass::Foreground),
    ];

    eprintln!("=== parallel-runner  {} agents, pool_cap={pool_cap} ===", questions.len());
    eprintln!("modèle: {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/parallel-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(128 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/multi_turn.wasm"))
        .expect("wasm");
    let pool  = Arc::new(InferencePool::new_with_queue_params(
        pool_cap, questions.len() + 4, 30_000,
        OllamaBackend { model: model.into(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    // Spawner tous les agents et envoyer leur question immédiatement
    let t_start = Instant::now();
    let mut handles: Vec<(usize, [u8; 16], tokio::sync::mpsc::Sender<Message>)> = Vec::new();

    for (i, (q, pclass)) in questions.iter().enumerate() {
        let mut id = [0x61u8; 16]; // 'a'×16
        id[15] = i as u8;

        let (tx, rx) = tokio::sync::mpsc::channel::<Message>(4);
        tokio::spawn(os_poc_runtime::actor::run_loop(
            ActorInstance::new_precompiled_with_inference_and_profile(
                &eng, &wasm, id,
                Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                InferencePool::as_infer_fn_with_class(Arc::clone(&pool), *pclass),
                AgentProfile::Batch,
            ).await.expect("actor"),
            rx,
        ));

        eprintln!("  Agent {i} spawné : \"{q}\" ({pclass:?})");
        tx.send(Message::data(q.as_bytes().to_vec())).await.unwrap();
        handles.push((i, id, tx));
    }

    eprintln!("\n  Tous spawnés en {:.0}ms — en attente des réponses...\n",
        t_start.elapsed().as_millis());

    // Attendre toutes les réponses
    let deadline = Instant::now() + Duration::from_secs(180);
    let n = questions.len();
    let mut done = vec![false; n];
    let mut results: Vec<Option<String>> = vec![None; n];

    while done.iter().any(|d| !d) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let stats = pool.queue_stats();
        let active = pool.active_count();
        eprint!("\r  admitted={} active={} elapsed={:.0}s    ",
            stats.total_admitted, active, t_start.elapsed().as_secs_f64());
        let _ = std::io::stderr().flush();

        for (i, id, _) in &handles {
            if done[*i] { continue; }
            let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
            let resp = ids.iter().find_map(|aid| {
                let e = log.get(aid).ok()??;
                let pb = e.emit_payload?;
                let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
                (env.emit_type == EmitType::ActionResult as u8)
                    .then(|| String::from_utf8_lossy(&env.payload).trim().to_string())
            });
            if let Some(r) = resp {
                done[*i] = true;
                results[*i] = Some(r);
            }
        }
        let _ = std::io::stderr().flush();
    }
    let elapsed = t_start.elapsed();
    eprintln!();

    // Affichage des résultats
    let stats = pool.queue_stats();
    eprintln!("\n=== Résultats ({:.1}s total) ===", elapsed.as_secs_f64());
    eprintln!("  Pool : admitted={} promoted={}",
        stats.total_admitted, stats.total_promoted);
    eprintln!();
    for (i, result) in results.iter().enumerate() {
        let (q, pclass) = &questions[i];
        let resp = result.as_deref().unwrap_or("[timeout]");
        println!("Agent {i} ({pclass:?}) : {q}\n  → {resp}\n");
    }

    // Vérifier le log partagé
    let total_entries: usize = handles.iter().map(|(_, id, _)| {
        log.query_by_agent_range(id, None, None).unwrap_or_default().len()
    }).sum();
    eprintln!("  Log partagé : {total_entries} entrées au total ({} agents)",
        questions.len());
    eprintln!("log: {}", tmp.display());
}
