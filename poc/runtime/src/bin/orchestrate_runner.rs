// orchestrate_runner — spawn dynamique déclenché par un Event dans le log.
//
// Flux :
//   1. Runner spawne l'Orchestrateur
//   2. Envoie la question → Orchestrateur analyse + émet Event("delegate:<question>")
//   3. Runner détecte l'Event dans le log (cause = action Orchestrateur)
//   4. Runner spawne un Spécialiste (multi_turn.wasm) avec la sous-question
//   5. Spécialiste répond → ActionResult dans le log
//   6. Runner envoie la réponse du Spécialiste à l'Orchestrateur (phase 0x02)
//   7. Orchestrateur synthétise → ActionResult final

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

fn hex4(b: &[u8; 32]) -> String {
    b.iter().take(4).map(|x| format!("{x:02x}")).collect()
}

async fn wait_emit_type(
    log: &CausalLog, id: &[u8; 16], after: usize, etype: u8, secs: u64,
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
            if env.emit_type == etype { return Some((env.payload, *aid)); }
        }
        if Instant::now() >= deadline { return None; }
    }
}

#[tokio::main]
async fn main() {
    let model = "llama3.2:3b";
    let endpoint = "http://localhost:11434";
    eprintln!("=== orchestrate-runner  Orchestrateur + spawn dynamique Spécialiste ===");
    eprintln!("modèle: {model}\n(tapez /quit)\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/orchestrate-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();

    let wasm_orch = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/orchestrator.wasm"))
        .expect("orchestrator.wasm");
    let wasm_spec = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/multi_turn.wasm"))
        .expect("multi_turn.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 8, 30_000,
        OllamaBackend { model: model.into(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut turn = 0u32;

    loop {
        print!("You: ");
        std::io::stdout().flush().unwrap();
        let line = match lines.next_line().await { Ok(Some(l)) => l, _ => break };
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        if line == "/quit" { break; }
        turn += 1;

        // Orchestrateur frais à chaque question
        let mut orch_id = [0x4Fu8; 16]; // 'O'×16
        orch_id[15] = turn as u8;

        let (tx_orch, rx_orch) = tokio::sync::mpsc::channel::<Message>(4);
        tokio::spawn(os_poc_runtime::actor::run_loop(
            ActorInstance::new_precompiled_with_inference_and_profile(
                &eng, &wasm_orch, orch_id,
                Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor),
                AgentProfile::Batch,
            ).await.expect("orchestrator"),
            rx_orch,
        ));

        // ── Phase 1 : envoyer question à l'orchestrateur ──────────────────────
        let before_orch = log.query_by_agent_range(&orch_id, None, None).unwrap_or_default().len();
        let mut payload = vec![0x01u8];
        payload.extend_from_slice(line.as_bytes());
        tx_orch.send(Message::data(payload)).await.unwrap();
        eprint!("  [Orchestrateur analyse");

        // ── Phase 2 : attendre Event (0x03) = signal de délégation ───────────
        let ev = wait_emit_type(&log, &orch_id, before_orch, EmitType::Event as u8, 90).await;
        eprintln!("]");

        let (event_payload, orch_action_id) = match ev {
            Some(x) => x,
            None => { eprintln!("[Orchestrateur timeout]"); continue; }
        };

        // Extraire la sous-question du payload "delegate:<question>"
        let sub_question = if event_payload.starts_with(b"delegate:") {
            String::from_utf8_lossy(&event_payload[9..]).to_string()
        } else {
            line.clone()
        };
        eprintln!("  Délégation détectée : \"{sub_question}\"");
        eprintln!("  Cause orchestrateur : {}", hex4(&orch_action_id));

        // ── Phase 3 : spawn dynamique du spécialiste ─────────────────────────
        let mut spec_id = [0x53u8; 16]; // 'S'×16
        spec_id[15] = turn as u8;

        let (tx_spec, rx_spec) = tokio::sync::mpsc::channel::<Message>(4);
        tokio::spawn(os_poc_runtime::actor::run_loop(
            ActorInstance::new_precompiled_with_inference_and_profile(
                &eng, &wasm_spec, spec_id,
                Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
                AgentProfile::Batch,
            ).await.expect("specialist"),
            rx_spec,
        ));
        eprintln!("  Spécialiste spawné dynamiquement (cause={})", hex4(&orch_action_id));

        let before_spec = log.query_by_agent_range(&spec_id, None, None).unwrap_or_default().len();
        // Envoyer la sous-question au spécialiste, avec l'action de l'orchestrateur comme cause
        tx_spec.send(Message::caused(sub_question.as_bytes().to_vec(), orch_action_id)).await.unwrap();
        eprint!("  [Spécialiste répond");

        // ── Phase 4 : attendre réponse du spécialiste (ActionResult) ──────────
        let spec_r = wait_emit_type(&log, &spec_id, before_spec, EmitType::ActionResult as u8, 90).await;
        eprintln!("]");

        let (spec_payload, spec_action_id) = match spec_r {
            Some(x) => x,
            None => { eprintln!("[Spécialiste timeout]"); continue; }
        };
        let spec_answer = String::from_utf8_lossy(&spec_payload).trim().to_string();
        eprintln!("  Spécialiste ({}) : {spec_answer}", hex4(&spec_action_id));

        // ── Phase 5 : injecter réponse dans l'orchestrateur (phase 0x02) ─────
        let after_event = log.query_by_agent_range(&orch_id, None, None).unwrap_or_default().len();
        let mut phase2 = vec![0x02u8];
        phase2.extend_from_slice(spec_answer.as_bytes());
        tx_orch.send(Message::caused(phase2, spec_action_id)).await.unwrap();
        eprint!("  [Orchestrateur synthétise");

        // ── Phase 6 : attendre synthèse finale ────────────────────────────────
        let final_r = wait_emit_type(&log, &orch_id, after_event, EmitType::ActionResult as u8, 90).await;
        eprintln!("]");

        match final_r {
            Some((p, _)) => println!("Résultat (tour {turn}): {}\n",
                String::from_utf8_lossy(&p).trim()),
            None => eprintln!("[Synthèse timeout]\n"),
        }

        drop(tx_spec);
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    eprintln!("log: {}", tmp.display());
}
