// self_correct_runner — A2 en action réelle : agent qui détecte son erreur et se corrige.
//
// Cas d'usage : génération de communiqué formaté.
// L'agent vérifie DÉTERMINISTEMENT si son draft commence par "ANNOUNCE:".
// Si non → agent_self_rollback(1) depuis le WASM → retry avec prompt directif.
// Si oui → confirme directement.
//
// Propriétés démontrées :
//   - A2 : self_rollback initié par le WASM lui-même (pas le runner)
//   - P6 : chaque barrier() est atomic dans le log
//   - Log append-only : draft provisoire + QUALITY:FAIL + SelfRollback + draft corrigé
//     tous présents pour audit, même si la correction a effacé l'état intermédiaire.
//
// Contraste avec une API LLM stateless :
//   - API stateless : l'app ne sait pas si l'agent s'est "auto-corrigé" — hors garanties.
//   - OS-pour-IA : le log trace le SelfRollback — l'auto-correction est un événement
//     causalement tracé, auditable, non falsifiable.

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

/// Lit toutes les entrées du log pour cet agent et les retourne en ordre.
fn read_all_log_entries(
    log: &CausalLog,
    id: &[u8; 16],
) -> Vec<([u8; 32], os_poc_causal_log::EmitType, Vec<u8>)> {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    ids.iter().filter_map(|aid| {
        let e = log.get(aid).ok()??;
        let pb = e.emit_payload?;
        let env = os_poc_causal_log::EmitEnvelope::from_msgpack(&pb).ok()?;
        let emit_type = os_poc_causal_log::EmitType::try_from(env.emit_type).ok()?;
        Some((*aid, emit_type, env.payload))
    }).collect()
}

async fn wait_terminated(
    log: &CausalLog,
    id: &[u8; 16],
    secs: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        eprint!(".");
        let _ = std::io::stderr().flush();
        let entries = read_all_log_entries(log, id);
        // L'agent a terminé quand on trouve un ActionResult qui commence par [SELF_CORRECTED] ou [CONFIRMED]
        // ou quand le log a au moins 4 entrées (pattern normal de l'agent)
        if entries.iter().any(|(_, t, p)| {
            *t == EmitType::ActionResult &&
            (p.starts_with(b"[SELF_CORRECTED]") || p.starts_with(b"[CONFIRMED]"))
        }) {
            return true;
        }
        if Instant::now() >= deadline { return false; }
    }
}

#[tokio::main]
async fn main() {
    let model    = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== self-correct-runner — A2 : auto-correction par agent_self_rollback ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/self-correct-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng   = make_engine();
    let wasm  = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/quality_writer.wasm"))
        .expect("quality_writer.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        1, 4, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let id: [u8; 16] = *b"quality-writer00";
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let actor = ActorInstance::new_precompiled_with_inference_and_profile(
        &eng, &wasm, id,
        Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps), vec![],
        SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
        InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground),
        AgentProfile::Batch,
    ).await.expect("actor");

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx));

    let task = b"NovOS - the first operating system designed for AI agents, \
with causal logging, WASM isolation, and atomic state management";

    eprintln!("Tâche : \"{}\"", String::from_utf8_lossy(task));
    eprintln!("Format attendu : réponse commençant par \"ANNOUNCE:\"\n");

    tx.send(Message::data(task.to_vec())).await.unwrap();
    eprint!("Agent en cours");

    let done = wait_terminated(&log, &id, 300).await;
    drop(tx);
    eprintln!();

    if !done {
        eprintln!("[timeout]");
        return;
    }

    // ── Reconstruction de l'audit trail depuis le log ─────────────────────────
    let entries = read_all_log_entries(&log, &id);

    eprintln!("\n╔══ AUDIT TRAIL — log append-only (toutes les entrées) ════════════");
    for (aid, emit_type, payload) in &entries {
        let type_str = match emit_type {
            EmitType::ActionResult   => "ActionResult  ",
            EmitType::SelfRollback   => "SelfRollback  ",
            EmitType::Introspect     => "Introspect    ",
            _                        => "Other         ",
        };
        let preview = match emit_type {
            EmitType::SelfRollback => {
                let depth = payload.first().copied().unwrap_or(0);
                let target = if payload.len() >= 9 {
                    u64::from_le_bytes(payload[1..9].try_into().unwrap_or([0; 8]))
                } else { 0 };
                format!("depth={depth} target_seq={target}")
            }
            _ => String::from_utf8_lossy(payload).chars().take(80).collect::<String>(),
        };
        eprintln!("║  {} {} : {}", type_str, hex8(aid), preview);
    }
    eprintln!("╠══════════════════════════════════════════════════════════════════");

    // Bilan
    let had_self_rollback = entries.iter().any(|(_, t, _)| *t == EmitType::SelfRollback);
    let final_result = entries.iter()
        .filter(|(_, t, p)| *t == EmitType::ActionResult &&
            (p.starts_with(b"[SELF_CORRECTED]") || p.starts_with(b"[CONFIRMED]")))
        .last();

    eprintln!("║  Auto-correction : {}", if had_self_rollback { "OUI — SelfRollback tracé" } else { "NON — draft accepté au premier essai" });
    if let Some((aid, _, payload)) = final_result {
        eprintln!("║  Résultat final (action_id: {}) :", hex8(aid));
        let text = String::from_utf8_lossy(payload);
        for line in text.lines().take(4) {
            eprintln!("║    {line}");
        }
    }
    eprintln!("╠══════════════════════════════════════════════════════════════════");
    eprintln!("║  {} entrées dans le log — draft provisoire + markers = audit complet", entries.len());
    eprintln!("╚══════════════════════════════════════════════════════════════════\n");

    eprintln!("log: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
