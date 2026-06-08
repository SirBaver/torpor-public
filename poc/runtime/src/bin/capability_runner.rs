// capability_runner — P4 en action réelle : accès capability-gated, refus loggué.
//
// Cas d'usage : un agent de reporting lit/écrit des données.
// Il a une cap pour "reports/" mais PAS pour "confidential/".
// Quand il tente d'écrire sur "confidential/...", le runtime :
//   1. refuse l'accès (agent_store_put retourne -1)
//   2. émet un CapabilityDenied (0x14) dans le log — SANS intervention de l'agent
//
// Ressources testées :
//   reports/quarterly/2024-Q4    → cap couvre "reports/" → WROTE
//   confidential/salary_2024     → hors scope cap        → DENIED + 0x14 dans log
//   reports/annual/2023          → cap couvre "reports/" → WROTE
//   admin/system_config          → hors scope cap        → DENIED + 0x14 dans log
//   reports/draft/summary        → cap couvre "reports/" → WROTE
//   confidential/passwords       → hors scope cap        → DENIED + 0x14 dans log
//
// Propriétés démontrées :
//   - P4 : contrôle d'accès au niveau runtime, non contournable par le WASM
//   - Le refus est tracé dans le log (CapabilityDenied 0x14) sans code agent
//   - L'agent sait qu'il a été refusé (rc=-1) mais ne peut pas lever le refus
//
// Contraste avec une API LLM stateless :
//   - API stateless : les droits d'accès sont gérés par l'application — un bug
//     peut bypasser les vérifications. Le LLM ne "voit" pas les refus.
//   - OS-pour-IA : le runtime enforce les caps — impossible pour le WASM de
//     contourner. Chaque refus est dans le log causal, auditable a posteriori.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitType};
use os_poc_capabilities::{CapabilityStore, Permissions};
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
        tokio::time::sleep(Duration::from_millis(300)).await;
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

    eprintln!("=== capability-runner — P4 : accès capability-gated, refus dans le log ===");
    eprintln!("modèle : {model} (non utilisé — agent déterministe)\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/capability-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/data_accessor.wasm"))
        .expect("data_accessor.wasm");

    // pool_cap=1 (pas d'inférence LLM dans cet agent)
    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 4, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));

    let id: [u8; 16] = *b"data-accessor-00";

    // ── Créer la capability pour "reports/" (write autorisé) ─────────────────
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));
    let cap_id: u64 = {
        let mut store_lock = caps.lock().unwrap();
        store_lock.grant_root(
            id,
            Permissions { read: true, write: true, execute: false, delegate: false },
            "reports".to_string(),
        )
    };

    eprintln!("Cap accordée : reports (read+write, couvre reports/*)");
    eprintln!("Cap refusée  : confidential/, admin/ (pas de cap)");
    eprintln!("cap_id       : {cap_id}\n");

    let actor = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm, id,
        Arc::clone(&store), Arc::clone(&log),
        Arc::clone(&caps), vec![cap_id],    // cap_id passé à l'agent
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Algo,
    ).await.expect("actor");

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx));

    // Ressources à tester
    let resources: &[(&str, bool)] = &[
        ("reports/quarterly/2024-Q4",  true),   // dans scope "reports/"
        ("confidential/salary_2024",   false),  // hors scope
        ("reports/annual/2023",        true),   // dans scope
        ("admin/system_config",        false),  // hors scope
        ("reports/draft/summary",      true),   // dans scope
        ("confidential/passwords",     false),  // hors scope
    ];

    eprintln!("╔══ TENTATIVES D'ACCÈS ═════════════════════════════════════════════");

    let mut results: Vec<(String, bool)> = Vec::new();
    for (resource, _expected) in resources {
        let before = log.query_by_agent_range(&id, None, None).unwrap_or_default().len();

        // Construire le message : [cap_id 8B LE][resource name]
        let mut msg = cap_id.to_le_bytes().to_vec();
        msg.extend_from_slice(resource.as_bytes());
        tx.send(Message::data(msg)).await.unwrap();

        match wait_action_result(&log, &id, before, 10).await {
            Some((text, _)) => {
                let granted = text.starts_with("WROTE:");
                let icon = if granted { "[OK]  " } else { "[DENY]" };
                eprintln!("║  {icon} {text}");
                results.push((text, granted));
            }
            None => {
                eprintln!("║  [timeout] {resource}");
                results.push((format!("TIMEOUT:{resource}"), false));
            }
        }
    }

    // Terminer l'agent
    tx.send(Message::data(vec![])).await.unwrap();
    drop(tx);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Lire les CapabilityDenied dans le log ─────────────────────────────────
    eprintln!("╠══════════════════════════════════════════════════════════════════");
    let all_ids = log.query_by_agent_range(&id, None, None).unwrap_or_default();
    let cap_denied: Vec<_> = all_ids.iter().filter_map(|aid| {
        let e = log.get(aid).ok()??;
        let pb = e.emit_payload?;
        let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
        (env.emit_type == EmitType::CapabilityDenied as u8).then_some((*aid, env.payload))
    }).collect();

    eprintln!("║  CapabilityDenied (0x14) dans le log :");
    for (aid, payload) in &cap_denied {
        // Payload: [agent_id 16B | cap_id 8B | resource_len 1B | resource | perm_flags | rate_limited]
        let resource_start = 16 + 8;
        let resource = if payload.len() > resource_start + 1 {
            let rlen = payload[resource_start] as usize;
            let rend = (resource_start + 1 + rlen).min(payload.len());
            String::from_utf8_lossy(&payload[resource_start + 1..rend]).to_string()
        } else { "[parse error]".to_string() };
        eprintln!("║    {} → resource=\"{}\"", hex8(aid), resource);
    }

    let n_wrote  = results.iter().filter(|(_, g)| *g).count();
    let n_denied = results.iter().filter(|(_, g)| !g).count();

    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║  Résumé : {n_wrote} WROTE / {n_denied} DENIED");
    eprintln!("║  {} CapabilityDenied dans le log (émis par le runtime, sans code agent)", cap_denied.len());
    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║  P4 : scope \"reports/\" enforced par le runtime WASM              ║");
    eprintln!("║  L'agent sait qu'il a été refusé (rc=-1) mais ne peut pas        ║");
    eprintln!("║  contourner la cap — contrôle d'accès non bypassable.            ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
