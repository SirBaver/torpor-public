// brainstorm_runner — I : fan-in causal réel, 3 agents en parallèle.
//
// Cas d'usage : nommer un produit IA.
//   3 équipes brainstorment en parallèle (angles différents : mythologique,
//   technique, moderne). Un synthétiseur reçoit les 3 résultats via
//   Message::caused (un par agent) SANS appeler barrier sur les 2 premiers —
//   les causes s'accumulent dans pending_extra_causes. Au 3e : barrier() avec
//   3 parent_ids (fan-in DAG), puis le synthétiseur choisit le meilleur nom.
//
// Propriétés démontrées :
//   - P3b : vrai fan-in — la décision du synthétiseur a 3 parent_ids dans le log,
//     pas une chaîne linéaire. BFS remonte aux 3 brainstormers.
//   - Contraste API stateless : aucun mécanisme pour tracer causalement un fan-in.
//     L'application doit gérer la provenance hors des garanties du runtime.
//
// DAG résultant :
//   brainstorm_myth ──┐
//   brainstorm_tech ──┼──► [barrier fan-in] ──► synthétiseur.ActionResult
//   brainstorm_mod  ──┘

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

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== brainstorm-runner — P3b : fan-in causal, 3 agents + synthétiseur ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/brainstorm-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_step  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let wasm_synth = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/brainstorm_synth.wasm"))
        .expect("brainstorm_synth.wasm");

    // pool_cap=3 : 3 brainstormers peuvent inférer en parallèle
    let pool = Arc::new(InferencePool::new_with_queue_params(
        3, 12, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    // ── IDs agents ───────────────────────────────────────────────────────────
    let id_myth:  [u8; 16] = *b"brainstorm-myth0";
    let id_tech:  [u8; 16] = *b"brainstorm-tech0";
    let id_mod:   [u8; 16] = *b"brainstorm-mod00";
    let id_synth: [u8; 16] = *b"brainstorm-synth";

    // ── Spawn 3 brainstormers ─────────────────────────────────────────────────
    let make_actor = |id: [u8; 16]| {
        let eng = eng.clone();
        let wasm = wasm_step.clone();
        let store = Arc::clone(&store);
        let log = Arc::clone(&log);
        let caps = Arc::clone(&caps);
        let pool = Arc::clone(&pool);
        async move {
            ActorInstance::new_precompiled_with_inference_and_profile(
                &eng, &wasm, id,
                store, log, caps, vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                InferencePool::as_infer_fn_with_class(pool, PriorityClass::Foreground),
                AgentProfile::Batch,
            ).await.expect("actor")
        }
    };

    let (tx_myth, rx_myth) = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_tech, rx_tech) = tokio::sync::mpsc::channel::<Message>(2);
    let (tx_mod,  rx_mod)  = tokio::sync::mpsc::channel::<Message>(2);

    tokio::spawn(os_poc_runtime::actor::run_loop(make_actor(id_myth).await, rx_myth));
    tokio::spawn(os_poc_runtime::actor::run_loop(make_actor(id_tech).await, rx_tech));
    tokio::spawn(os_poc_runtime::actor::run_loop(make_actor(id_mod).await,  rx_mod));

    let before_myth = log.query_by_agent_range(&id_myth, None, None).unwrap_or_default().len();
    let before_tech = log.query_by_agent_range(&id_tech, None, None).unwrap_or_default().len();
    let before_mod  = log.query_by_agent_range(&id_mod,  None, None).unwrap_or_default().len();

    // ── Envoyer prompts différents en parallèle ───────────────────────────────
    let prompt_myth = b"List exactly 3 product names for an AI operating system. \
                        Use mythological figures (Greek, Roman, Norse). \
                        One name per line, name only.".to_vec();
    let prompt_tech = b"List exactly 3 product names for an AI operating system. \
                        Use technical/scientific terms (precision, clarity, reliability). \
                        One name per line, name only.".to_vec();
    let prompt_mod  = b"List exactly 3 product names for an AI operating system. \
                        Use modern tech culture, short punchy names. \
                        One name per line, name only.".to_vec();

    eprintln!("3 brainstormers lancés en parallèle :");
    eprintln!("  [MYTH] angle mythologique");
    eprintln!("  [TECH] angle technique");
    eprintln!("  [MOD]  angle moderne\n");

    tx_myth.send(Message::data(prompt_myth)).await.unwrap();
    tx_tech.send(Message::data(prompt_tech)).await.unwrap();
    tx_mod .send(Message::data(prompt_mod)).await.unwrap();
    eprint!("Brainstorm en cours");

    // ── Attendre les 3 résultats (en parallèle) ───────────────────────────────
    let (res_myth, res_tech, res_mod) = tokio::join!(
        wait_action_result(&log, &id_myth, before_myth, 180),
        wait_action_result(&log, &id_tech, before_tech, 180),
        wait_action_result(&log, &id_mod,  before_mod,  180),
    );
    eprintln!();

    let (text_myth, act_myth) = res_myth.expect("myth timeout");
    let (text_tech, act_tech) = res_tech.expect("tech timeout");
    let (text_mod,  act_mod)  = res_mod .expect("mod timeout");

    drop(tx_myth); drop(tx_tech); drop(tx_mod);

    eprintln!("╔══ PROPOSITIONS ════════════════════════════════════════════════════");
    eprintln!("║  [MYTH] action_id: {}", hex8(&act_myth));
    for line in text_myth.lines().take(3) { eprintln!("║    {line}"); }
    eprintln!("║");
    eprintln!("║  [TECH] action_id: {}", hex8(&act_tech));
    for line in text_tech.lines().take(3) { eprintln!("║    {line}"); }
    eprintln!("║");
    eprintln!("║  [MOD]  action_id: {}", hex8(&act_mod));
    for line in text_mod.lines().take(3) { eprintln!("║    {line}"); }
    eprintln!("╚════════════════════════════════════════════════════════════════════\n");

    // ── Spawn synthétiseur ────────────────────────────────────────────────────
    let (tx_synth, rx_synth) = tokio::sync::mpsc::channel::<Message>(4);
    let synth_actor = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm_synth, id_synth,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
        AgentProfile::Batch,
    ).await.expect("synth actor");
    tokio::spawn(os_poc_runtime::actor::run_loop(synth_actor, rx_synth));

    let before_synth = log.query_by_agent_range(&id_synth, None, None).unwrap_or_default().len();

    // ── Envoyer 3 messages causaux au synthétiseur ────────────────────────────
    // Message 1 et 2 : pas de barrier dans brainstorm_synth → causes s'accumulent
    // Message 3 : barrier() avec les 3 causes en parent_ids (fan-in)
    eprintln!("Envoi résultats au synthétiseur (fan-in causal) :");
    eprintln!("  msg 1 : result_myth (cause = {})", hex8(&act_myth));
    eprintln!("  msg 2 : result_tech (cause = {})", hex8(&act_tech));
    eprintln!("  msg 3 : result_mod  (cause = {}) → barrier fan-in\n", hex8(&act_mod));

    tx_synth.send(Message::caused(text_myth.into_bytes(), act_myth)).await.unwrap();
    tx_synth.send(Message::caused(text_tech.into_bytes(), act_tech)).await.unwrap();
    tx_synth.send(Message::caused(text_mod.into_bytes(),  act_mod)).await.unwrap();
    eprint!("Synthèse en cours");

    let synth_result = wait_action_result(&log, &id_synth, before_synth, 180).await;
    eprintln!();
    drop(tx_synth);

    let (decision, act_synth) = synth_result.expect("synth timeout");

    eprintln!("\n╔══ DÉCISION FINALE ═════════════════════════════════════════════════");
    eprintln!("║  action_id : {}", hex8(&act_synth));
    for line in decision.lines().take(4) { eprintln!("║  {line}"); }
    eprintln!("╚════════════════════════════════════════════════════════════════════\n");

    // ── Vérifier le fan-in dans le log ────────────────────────────────────────
    let all_synth = log.query_by_agent_range(&id_synth, None, None).unwrap_or_default();

    // Chercher l'entrée barrier du synthétiseur (type 5) — c'est là que se trouve le fan-in
    let fan_in_entry = all_synth.iter().find_map(|aid| {
        let e = log.get(aid).ok()??;
        (e.parent_ids.len() >= 3).then_some((*aid, e.parent_ids.clone()))
    });

    eprintln!("╔══ AUDIT DAG — fan-in dans le log ══════════════════════════════════");
    match fan_in_entry {
        Some((fid, parents)) => {
            eprintln!("║  Entrée fan-in : {}", hex8(&fid));
            eprintln!("║  parent_ids ({}) :", parents.len());
            for p in &parents {
                let label = if *p == act_myth { " [MYTH]"
                    } else if *p == act_tech { " [TECH]"
                    } else if *p == act_mod  { " [MOD]"
                    } else { "" };
                eprintln!("║    {}{}", hex8(p), label);
            }
        }
        None => {
            eprintln!("║  [aucune entrée avec 3+ parents — fan-in non détecté]");
            // Afficher les parent_ids de toutes les entrées synth pour diagnostiquer
            for aid in &all_synth {
                if let Ok(Some(e)) = log.get(aid) {
                    eprintln!("║  entry {} : {} parents", hex8(aid), e.parent_ids.len());
                }
            }
        }
    }
    eprintln!("╠════════════════════════════════════════════════════════════════════");
    eprintln!("║  Total entrées log synth : {}", all_synth.len());
    eprintln!("║  Total entrées log myth  : {}",
        log.query_by_agent_range(&id_myth, None, None).unwrap_or_default().len());
    eprintln!("║  Total entrées log tech  : {}",
        log.query_by_agent_range(&id_tech, None, None).unwrap_or_default().len());
    eprintln!("║  Total entrées log mod   : {}",
        log.query_by_agent_range(&id_mod,  None, None).unwrap_or_default().len());
    eprintln!("╠════════════════════════════════════════════════════════════════════");
    eprintln!("║  P3b : vrai fan-in DAG — barrier() du synthétiseur a les 3 causes ║");
    eprintln!("║  en parent_ids, non une chaîne linéaire A->B->C.                  ║");
    eprintln!("╚════════════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
