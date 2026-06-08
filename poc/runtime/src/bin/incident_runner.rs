// incident_runner — triage d'incident : fan-out parallèle + fan-in causal.
//
// Cas d'usage : un incident de production présente plusieurs symptômes simultanés.
// Trois spécialistes analysent en parallèle (infrastructure, BDD, sécurité).
// Un agrégateur synthétise les trois analyses en un rapport d'incident final.
//
// DAG : incident → [infra, db, security] → rapport_final
//   Chaque flèche = lien causal dans le log.
//   Le rapport final a 3 parents : les 3 ActionResults des spécialistes.
//
// ── Refacto ADR-0063 (incrément 2) ──────────────────────────────────────────────
// Cette flotte est désormais orchestrée par la bibliothèque de Routers (`os_poc_runtime::fleet`)
// au lieu de `tokio::spawn(run_loop(...))` manuel + `wait_action_result` copié-collé :
//   - les membres sont matérialisés par `Scheduler::register` (qui lance la run_loop) ;
//   - le `FleetDriver` centralise le poll-du-log (un seul point, plus de boucle par agent) ;
//   - le `FanInRouter` (famille 2) décide le routage fan-in (REPORT:<role>:… puis FINALIZE) ;
//   - la causalité passe par le canal TCB `Message::caused` (aucun CauseHandle, cf. ADR-0063 D3) ;
//   - flotte mono-tenant (T1) : la garde `tenant_of` du driver est la frontière (ADR-0063 D4).
//
// Agrégation PARTIELLE sur deadline (incrément 2b) : si un spécialiste dépasse le deadline de
// collecte, le driver émet `FleetEvent::Deadline` au `FanInRouter`, qui finalise avec les rapports
// reçus (comportement faithful des runners pré-fleet). Sur le happy-path, comportement identique.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use os_poc_capabilities::CapabilityStore;
use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType};
use os_poc_runtime::actor::{ActorInstanceBuilder, Message, TenantId};
use os_poc_runtime::fleet::{FanInRouter, FleetDriver, Route};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::scheduler::Scheduler;
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::Cache;

const INCIDENT: &str = "\
[ALERT] Production system — multiple simultaneous symptoms:\n\
- CPU on inference servers: 98% (normal: 40%)\n\
- Database query latency: 2400ms avg (normal: 240ms, 10x spike)\n\
- Authentication errors: 340/min (normal: <5/min)\n\
- Started: 14 minutes ago\n\
- No recent deployments\n\
- Affects: EU region only\
";

// Rôles des spécialistes et leurs questions
const SPECIALISTS: &[(&str, &str, &str)] = &[
    ("infra",
     "infrastructure-specialist",
     "Analyze the infrastructure symptoms: CPU spike at 98% on inference servers, EU region only. What is the most likely cause? What should be checked first?"),
    ("db",
     "database-specialist",
     "Analyze the database symptom: query latency spiked 10x (240ms→2400ms), no recent deployments. What is the most likely cause? What should be checked first?"),
    ("security",
     "security-specialist",
     "Analyze the security symptom: authentication errors jumped from <5/min to 340/min, EU region only. Is this an attack? What should be checked first?"),
];

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

/// Dernier `ActionResult` d'un agent dans le log (texte). Pour afficher le rapport final après que
/// le driver a terminé (le driver a déjà consommé l'événement ; ici on relit le log).
fn last_action_result(log: &CausalLog, agent: &[u8; 16]) -> Option<(String, [u8; 32])> {
    let ids = log.query_by_agent_range(agent, None, None).unwrap_or_default();
    for aid in ids.iter().rev() {
        let Ok(Some(e)) = log.get(aid) else { continue };
        let Some(pb) = e.emit_payload else { continue };
        let Ok(env) = EmitEnvelope::from_msgpack(&pb) else { continue };
        if env.emit_type == EmitType::ActionResult as u8 {
            return Some((String::from_utf8_lossy(&env.payload).trim().to_string(), *aid));
        }
    }
    None
}

#[tokio::main]
async fn main() {
    let model = std::env::args().nth(1).unwrap_or_else(|| "llama3.2:3b".into());
    let endpoint = "http://localhost:11434";

    eprintln!("=== incident-runner — triage parallèle + agrégation causale (fleet/FanInRouter) ===");
    eprintln!("modèle : {model}\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/incident-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(os_poc_store::ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng = make_engine();

    let wasm_step = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");
    let wasm_agg = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/incident_aggregator.wasm"))
        .expect("incident_aggregator.wasm");

    // pool_cap=3 : les 3 spécialistes tournent en parallèle
    let pool = Arc::new(InferencePool::new_with_queue_params(
        3, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));
    let tenant = TenantId(1); // flotte mono-tenant (ADR-0063 D4)

    // IDs de la flotte
    let inc_id: [u8; 16] = *b"incident-node000";
    let agg_id: [u8; 16] = *b"aggregator-00000";
    let spec_ids: Vec<[u8; 16]> = (0..SPECIALISTS.len())
        .map(|i| { let mut id = *b"specialist-00000"; id[11] = b'0' + i as u8; id })
        .collect();

    let mut scheduler = Scheduler::new();
    // Le driver observe les 3 spécialistes + l'agrégateur (le nœud-incident est un préambule).
    let mut expected = spec_ids.clone();
    expected.push(agg_id);
    let mut driver = FleetDriver::new(tenant, Arc::clone(&log), expected);

    eprintln!("┌─ INCIDENT ─────────────────────────────────────────────────────");
    for line in INCIDENT.lines() { eprintln!("│ {line}"); }
    eprintln!("└───────────────────────────────────────────────────────────────\n");

    // ── Nœud incident dans le log (racine du DAG) ────────────────────────────
    let inc_instance = ActorInstanceBuilder::new(&eng, &wasm_step, inc_id, Arc::clone(&store), Arc::clone(&log))
        .caps(Arc::clone(&caps), vec![])
        .tenant(tenant)
        .inference(InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor))
        .profile(AgentProfile::Batch)
        .session_max_duration_ms(0)
        .build().await.expect("incident node");
    scheduler.register(inc_instance);

    scheduler.send(&inc_id, Message::data(
        format!("\n---\nRecord this incident verbatim:\n{INCIDENT}").into_bytes()
    )).await.unwrap();
    eprint!("Enregistrement incident");
    let _ = std::io::stderr().flush();
    let (inc_cause, _) = driver
        .wait_result(&inc_id, Duration::from_millis(400), Duration::from_secs(120))
        .await
        .expect("incident node timeout");
    let inc_action_id = inc_cause.action_id();
    eprintln!("\nIncident dans le log (action_id: {})\n", hex8(&inc_action_id));

    // ── Enregistrement des spécialistes + agrégateur ─────────────────────────
    let mut labels = std::collections::HashMap::new();
    for (i, (role, _title, _q)) in SPECIALISTS.iter().enumerate() {
        let spec = ActorInstanceBuilder::new(&eng, &wasm_step, spec_ids[i], Arc::clone(&store), Arc::clone(&log))
            .caps(Arc::clone(&caps), vec![])
            .tenant(tenant)
            .inference(InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground))
            .profile(AgentProfile::Batch)
            .session_max_duration_ms(0)
            .build().await.expect("specialist");
        scheduler.register(spec);
        driver.prime_cursor(&spec_ids[i]);
        labels.insert(spec_ids[i], role.to_string());
    }
    let agg = ActorInstanceBuilder::new(&eng, &wasm_agg, agg_id, Arc::clone(&store), Arc::clone(&log))
        .caps(Arc::clone(&caps), vec![])
        .tenant(tenant)
        .inference(InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor))
        .profile(AgentProfile::Batch)
        .session_max_duration_ms(0)
        .build().await.expect("aggregator");
    scheduler.register(agg);
    driver.prime_cursor(&agg_id);

    // ── Fan-out : kick-off des 3 spécialistes (via le driver, garde tenant + canal TCB) ──
    eprintln!("Fan-out : 3 spécialistes lancés en parallèle...");
    for (i, (role, title, question)) in SPECIALISTS.iter().enumerate() {
        let msg = format!("You are a {title}.\nContext:\n{INCIDENT}\n---\n{question}");
        driver
            .execute(
                Route::SendCaused { to: spec_ids[i], payload: msg.into_bytes(), cause: inc_cause },
                &mut scheduler,
            )
            .await
            .expect("kick-off specialist");
        eprint!("  [{role}]");
    }
    eprintln!();

    // ── Fan-in : le FanInRouter agrège puis finalise ; le driver pilote la boucle ──
    eprintln!("Fan-in : le FanInRouter collecte les analyses puis déclenche l'agrégation...");
    let mut router = FanInRouter::new(agg_id, inc_cause, labels);
    let done = driver
        .run(&mut router, &mut scheduler, Duration::from_millis(400), Duration::from_secs(300))
        .await;

    if !done {
        eprintln!("\n[timeout — agrégation non finalisée]");
        eprintln!("\nlog: {}", tmp.display());
        return;
    }

    // ── Rapport final (relu depuis le log) ──────────────────────────────────────
    let (report, report_id) = match last_action_result(&log, &agg_id) {
        Some(x) => x,
        None => { eprintln!("\n[agrégateur sans résultat]"); return; }
    };

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║              RAPPORT D'INCIDENT FINAL                       ║");
    eprintln!("║  action_id : {}                    ║", hex8(&report_id));
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    for line in report.lines() { eprintln!("║ {line}"); }
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  DAG : incident → [infra, db, security] → rapport           ║");
    eprintln!("║  Orchestré par fleet::FanInRouter (ADR-0063)                 ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
