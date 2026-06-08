// consensus_runner — décision collective par vote majoritaire avec audit complet.
//
// Cas d'usage : comité de validation technique. N agents votent indépendamment
// sur une proposition de déploiement. La majorité l'emporte. Toutes les voix
// (y compris les dissidences) sont committées dans le log causal.
//
// Propriétés démontrées (ADR-0054) :
//   - DAG complet : proposition → N votes → décision finale (tous dans le log)
//   - Délibération auditée : chaque vote est un ActionResult causal
//   - Dissidence préservée dans le log pour toujours
//   - Décision déterministe : tally_secretary.wasm, code pur, pas de LLM
//
// ── Refacto ADR-0063 (incrément 2b) ─────────────────────────────────────────────
// Orchestré par la bibliothèque de Routers (`os_poc_runtime::fleet`) : `Scheduler::register`
// matérialise les votants + le secrétaire, le `FleetDriver` centralise le poll-du-log, le
// `QuorumRouter` (famille 3) route chaque vote (VOTE:<verdict>) puis déclenche TALLY:<N> au seuil
// (ou tally PARTIEL si un votant dépasse le deadline — les muets comptent comme abstentions).
// Causalité via canal TCB `Message::caused` (aucun CauseHandle, ADR-0063 D3) ; flotte mono-tenant.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use os_poc_capabilities::CapabilityStore;
use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType};
use os_poc_runtime::actor::{ActorInstanceBuilder, Message, TenantId};
use os_poc_runtime::fleet::{FleetDriver, QuorumRouter, Route};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::scheduler::Scheduler;
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};

const PROPOSAL: &str = "\
Deploy NovOS v0.3 to production tonight at 02:00 AM UTC.\n\
\n\
Facts:\n\
- Includes a database schema migration (irreversible, no rollback path)\n\
- Test coverage: 78% (target is 80%)\n\
- The team lead who wrote the migration is on vacation until Monday\n\
- Deployment window: 02:00-04:00 AM (low traffic)\n\
- Last production incident: 3 weeks ago (unrelated)\n\
- Staging environment: tested successfully for 48h\
";

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

fn parse_vote(text: &str) -> &'static str {
    let upper = text.trim_start().to_ascii_uppercase();
    if upper.starts_with("APPROVE") { "APPROVE" }
    else if upper.starts_with("REJECT") { "REJECT" }
    else { "UNKNOWN" }
}

/// Dernier `ActionResult` d'un agent dans le log (texte + action_id).
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
    let n_voters: usize = std::env::args().nth(2)
        .and_then(|s| s.parse().ok()).unwrap_or(3);

    eprintln!("=== consensus-runner — vote majoritaire (fleet/QuorumRouter) ===");
    eprintln!("modèle : {model}  |  votants : {n_voters}  |  quorum : majorité simple (ADR-0054)\n");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp = std::path::PathBuf::from(format!("/tmp/consensus-{ts}"));
    std::fs::create_dir_all(&tmp).unwrap();

    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp.join("store"), Some(cache.clone())).unwrap());
    let log = Arc::new(CausalLog::open(&tmp.join("log"), Some(cache)).unwrap());
    let eng = make_engine();

    let wasm_voter = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/voter_agent.wasm"))
        .expect("voter_agent.wasm");
    let wasm_secretary = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/tally_secretary.wasm"))
        .expect("tally_secretary.wasm");
    let wasm_prop = load_module_from_file(&eng,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/task_step.wasm"))
        .expect("task_step.wasm");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        n_voters, 8, 30_000,
        OllamaBackend { model: model.clone(), endpoint: endpoint.into() },
    ));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));
    let tenant = TenantId(1); // flotte mono-tenant (ADR-0063 D4)

    let prop_id: [u8; 16] = *b"consensus-prop00";
    let sec_id: [u8; 16] = *b"tally-secret0000";
    let voter_ids: Vec<[u8; 16]> = (0..n_voters)
        .map(|i| { let mut id = *b"voter-agent-0000"; id[12] = b'0' + (i / 10) as u8; id[13] = b'0' + (i % 10) as u8; id })
        .collect();

    let mut scheduler = Scheduler::new();
    let mut expected = voter_ids.clone();
    expected.push(sec_id);
    let mut driver = FleetDriver::new(tenant, Arc::clone(&log), expected);

    eprintln!("┌─ PROPOSITION ─────────────────────────────────────────────────");
    for line in PROPOSAL.lines() { eprintln!("│ {line}"); }
    eprintln!("└───────────────────────────────────────────────────────────────\n");

    // ── D4 : nœud-proposition (racine commune des N votes) ────────────────────
    let prop_instance = ActorInstanceBuilder::new(&eng, &wasm_prop, prop_id, Arc::clone(&store), Arc::clone(&log))
        .caps(Arc::clone(&caps), vec![])
        .tenant(tenant)
        .inference(InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor))
        .profile(AgentProfile::Batch)
        .session_max_duration_ms(0)
        .build().await.expect("prop actor");
    scheduler.register(prop_instance);

    scheduler.send(&prop_id, Message::data(
        format!("\n---\nRecord this proposal verbatim: {PROPOSAL}").into_bytes()
    )).await.unwrap();
    eprint!("Enregistrement proposition");
    let _ = std::io::stderr().flush();
    let (prop_cause, _) = driver
        .wait_result(&prop_id, Duration::from_millis(400), Duration::from_secs(120))
        .await
        .expect("proposition timeout");
    eprintln!("\nProposition dans le log (action_id: {})\n", hex8(&prop_cause.action_id()));

    // ── Enregistrement des votants + secrétaire ──────────────────────────────
    for &vid in &voter_ids {
        let voter = ActorInstanceBuilder::new(&eng, &wasm_voter, vid, Arc::clone(&store), Arc::clone(&log))
            .caps(Arc::clone(&caps), vec![])
            .tenant(tenant)
            .inference(InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground))
            .profile(AgentProfile::Batch)
            .session_max_duration_ms(0)
            .build().await.expect("voter actor");
        scheduler.register(voter);
        driver.prime_cursor(&vid);
    }
    let secretary = ActorInstanceBuilder::new(&eng, &wasm_secretary, sec_id, Arc::clone(&store), Arc::clone(&log))
        .caps(Arc::clone(&caps), vec![])
        .tenant(tenant)
        .inference(InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor))
        .profile(AgentProfile::Batch)
        .session_max_duration_ms(0)
        .build().await.expect("secretary actor");
    scheduler.register(secretary);
    driver.prime_cursor(&sec_id);

    // ── Lancement des votants (cause = proposition, via le driver) ────────────
    eprintln!("Lancement de {n_voters} agents votants en parallèle...");
    for (i, &vid) in voter_ids.iter().enumerate() {
        driver
            .execute(
                Route::SendCaused { to: vid, payload: PROPOSAL.as_bytes().to_vec(), cause: prop_cause },
                &mut scheduler,
            )
            .await
            .expect("kick-off voter");
        eprint!("  Agent-{i:02}");
    }
    eprintln!("\n\n--- Délibération (QuorumRouter collecte les votes puis déclenche le tally) ---");

    // ── Le QuorumRouter collecte VOTE:* puis émet TALLY:<N> au seuil (ou partiel sur deadline) ──
    let mut router = QuorumRouter::new(sec_id, prop_cause, n_voters);
    let done = driver
        .run(&mut router, &mut scheduler, Duration::from_millis(400), Duration::from_secs(240))
        .await;

    // ── Affichage des votes (relus depuis le log pour l'audit) ───────────────
    let mut votes: Vec<(&'static str, [u8; 32])> = Vec::new();
    for (i, &vid) in voter_ids.iter().enumerate() {
        match last_action_result(&log, &vid) {
            Some((text, aid)) => {
                let verdict = parse_vote(&text);
                eprintln!("  Agent-{i:02} [{verdict}] ({})", hex8(&aid));
                votes.push((verdict, aid));
            }
            None => eprintln!("  Agent-{i:02} [abstention — pas de vote dans le log]"),
        }
    }

    // ── Résultat ─────────────────────────────────────────────────────────────
    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║                     RÉSULTAT DU VOTE                        ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");

    if !done {
        eprintln!("║  [secrétaire timeout — décision non tracée]                  ║");
        eprintln!("╚══════════════════════════════════════════════════════════════╝");
        eprintln!("\nlog: {}", tmp.display());
        return;
    }

    match last_action_result(&log, &sec_id) {
        Some((tally_text, tally_id)) => {
            for line in tally_text.lines() { eprintln!("║  {line}"); }
            eprintln!("║  action_id décision : {}", hex8(&tally_id));
            eprintln!("╠══════════════════════════════════════════════════════════════╣");
            let n_approve = votes.iter().filter(|(v, _)| *v == "APPROVE").count();
            let recomputed = if n_approve * 2 > n_voters { "APPROVED" } else { "REJECTED" };
            let consistent = tally_text.contains(recomputed);
            eprintln!("║  Recompute local : {recomputed} — cohérent log : {consistent}");
        }
        None => eprintln!("║  [secrétaire sans résultat]                                  ║"),
    }

    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  DAG : proposition → N votes → décision (ADR-0054)          ║");
    eprintln!("║  Orchestré par fleet::QuorumRouter (ADR-0063)               ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!("\nlog: {}", tmp.display());
    tokio::time::sleep(Duration::from_millis(200)).await;
}
