// sef2-runner — binaire de test SEF-2 (rollback transactionnel — P2).
//
// CONTRAT P2 (spec/02-properties.md §P2) :
//   « Après 1 000 actions, un rollback à l'action n°500 produit un état dont le
//   hash est identique à celui mesuré après l'action n°500. »
//
// MÉTHODE :
//   1. Un agent WASM minimal (AGENT_WAT) exécute N = 1 000 messages Data.
//      Chaque message produit un snapshot (commit_barrier) + un LogEntry ActionResult.
//   2. Pendant l'exécution on capture, à partir du log, le `hash_after` du LogEntry
//      ActionResult correspondant à l'action `k_target = 500` (i.e. `envelope.seq = k_target - 1`).
//   3. Après les N actions, `Scheduler::rollback(agent_id, target_seq = k_target - 1)` est
//      appelé. Le scheduler envoie `Message::Rollback` à l'agent ; `run_loop`
//      traverse la chaîne via `rollback_path` et émet `SchedulerRollback (0x0B)` avec
//      `hash_after = target_snap` = snapshot après action 500.
//   4. Le binaire vérifie quatre propriétés :
//        P-α  observed_after_rollback == hash_at_k        (chaîne content-addressed cohérente)
//        P-β  Le SnapshotHeader du snapshot cible a `seq == k_target - 1` (référence stable)
//        P-γ  Le dernier LogEntry SchedulerRollback de l'agent a `hash_after == hash_at_k`
//             ET son payload contient `target_seq == k_target - 1` (rollback réellement appliqué côté log)
//        P-δ  Après le rollback, envoyer une action supplémentaire produit un
//             LogEntry ActionResult avec `hash_before == hash_at_k` (la nouvelle
//             branche reprend bien depuis l'état restauré)
//
// PORTÉE DE LA PROPRIÉTÉ (épistémique) :
//   - Cas (P-α) est trivialement vrai par construction d'un store content-addressed
//     (le rollback n'écrit rien, il repointe `last_snapshot` vers un header existant
//     identifié par son hash). Sa vérification reste utile : elle exerce
//     `rollback_path` sur une chaîne de 500 sauts et garantit qu'aucun maillon n'est
//     corrompu / absent / non-recovré.
//   - Cas (P-β) à (P-δ) sont les véritables tests d'intégration du chemin
//     `Scheduler::rollback` → `run_loop` → `Message::Rollback` → ContentStore +
//     CausalLog → reprise d'exécution post-rollback.
//
// EXIT CODES :
//   0 — pass (les 4 propriétés tiennent + durée rollback ≤ borne)
//   1 — fail (au moins une propriété échoue)
//   2 — erreur arguments / I/O / orchestration

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType, LogEntry};
use os_poc_runtime::actor::{ActorInstance, Message, AGENT_WAT};
use os_poc_runtime::make_engine;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_store::{ContentStore, Cache};
use wasmtime::Module;

#[derive(Debug)]
struct Args {
    db_store: PathBuf,
    db_log: PathBuf,
    agent_id_hex: String,
    n_actions: u64,
    k_target: u64,
    rollback_budget_ms: u64,
    out_report: PathBuf,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "sef2-runner — SEF-2 rollback transactionnel (P2 — spec/02 §P2)\n\
\n\
USAGE:\n\
    sef2-runner --db-store <PATH> --db-log <PATH> --agent-id <HEX32>\n\
                --n-actions <N> --k-target <K>\n\
                [--rollback-budget-ms <MS>]\n\
                [--out-report <PATH>]\n\
\n\
ARGS:\n\
    --db-store <PATH>            Répertoire ContentStore (créé si absent).\n\
    --db-log <PATH>              Répertoire CausalLog (créé si absent).\n\
    --agent-id <HEX32>           Hex 32-caractères = 16 octets.\n\
    --n-actions <N>              Nombre total d'actions à exécuter (≥ k_target).\n\
    --k-target <K>               Action cible du rollback (1-indexé ; doit être ∈ [1, N-1]).\n\
                                 Le rollback ramène `last_snapshot` à l'état post-action K.\n\
    --rollback-budget-ms <MS>    Budget de durée du rollback (défaut 100 ms — P2).\n\
                                 Mesuré entre `Scheduler::rollback` et apparition de\n\
                                 SchedulerRollback (0x0B) dans le log.\n\
    --out-report <PATH>          Rapport JSON (défaut : report.json dans cwd).\n\
\n\
EXIT CODES:\n\
    0 = pass (4 propriétés + budget rollback OK)\n\
    1 = fail (au moins une propriété viole P2)\n\
    2 = erreur arguments / I/O / orchestration\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store: Option<PathBuf> = None;
    let mut db_log: Option<PathBuf> = None;
    let mut agent_id_hex: Option<String> = None;
    let mut n_actions: Option<u64> = None;
    let mut k_target: Option<u64> = None;
    let mut rollback_budget_ms: u64 = 100;
    let mut out_report: PathBuf = PathBuf::from("report.json");

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store" => { i += 1; db_store = Some(PathBuf::from(&raw[i])); }
            "--db-log"   => { i += 1; db_log   = Some(PathBuf::from(&raw[i])); }
            "--agent-id" => { i += 1; agent_id_hex = Some(raw[i].clone()); }
            "--n-actions" => {
                i += 1;
                n_actions = Some(raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-actions doit être un entier"); std::process::exit(2);
                }));
            }
            "--k-target" => {
                i += 1;
                k_target = Some(raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--k-target doit être un entier"); std::process::exit(2);
                }));
            }
            "--rollback-budget-ms" => {
                i += 1;
                rollback_budget_ms = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--rollback-budget-ms doit être un entier"); std::process::exit(2);
                });
            }
            "--out-report" => { i += 1; out_report = PathBuf::from(&raw[i]); }
            "-h" | "--help" => print_usage_and_exit(0),
            other => {
                eprintln!("Argument inconnu: {other}");
                print_usage_and_exit(2);
            }
        }
        i += 1;
    }

    let args = Args {
        db_store: db_store.unwrap_or_else(|| print_usage_and_exit(2)),
        db_log: db_log.unwrap_or_else(|| print_usage_and_exit(2)),
        agent_id_hex: agent_id_hex.unwrap_or_else(|| print_usage_and_exit(2)),
        n_actions: n_actions.unwrap_or_else(|| print_usage_and_exit(2)),
        k_target: k_target.unwrap_or_else(|| print_usage_and_exit(2)),
        rollback_budget_ms,
        out_report,
    };

    if args.k_target == 0 || args.k_target >= args.n_actions {
        eprintln!("--k-target doit vérifier 1 ≤ K ≤ N-1 (K={}, N={})", args.k_target, args.n_actions);
        std::process::exit(2);
    }
    args
}

fn parse_agent_id(hex: &str) -> [u8; 16] {
    if hex.len() != 32 {
        eprintln!("--agent-id doit être exactement 32 caractères hex (16 octets)");
        std::process::exit(2);
    }
    let mut out = [0u8; 16];
    for (i, byte_pair) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(byte_pair).expect("hex ASCII");
        out[i] = u8::from_str_radix(s, 16).unwrap_or_else(|_| {
            eprintln!("--agent-id : caractère non hex '{s}'");
            std::process::exit(2);
        });
    }
    out
}

fn hex_encode(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for byte in b {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

/// Retourne tous les LogEntry de `agent_id` dans l'ordre temporel (via index secondaire
/// `agent_ts` — production API). Charge chaque entry par `log.get`.
fn all_entries_for_agent(log: &CausalLog, agent_id: &[u8; 16]) -> Vec<LogEntry> {
    let ids = log.query_by_agent_range(agent_id, None, None).unwrap_or_default();
    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Ok(Some(entry)) = log.get(id) {
            out.push(entry);
        }
    }
    out
}

/// Filtre les LogEntry ActionResult (0x01) d'un agent.
/// Retourne `Vec<(LogEntry, EmitEnvelope)>` — la `seq` logique est portée par l'enveloppe.
fn collect_action_results(log: &CausalLog, agent_id: &[u8; 16]) -> Vec<(LogEntry, EmitEnvelope)> {
    let entries = all_entries_for_agent(log, agent_id);
    let mut out = Vec::new();
    for entry in entries {
        if let Some(payload) = entry.emit_payload.as_ref() {
            if let Ok(env) = EmitEnvelope::from_msgpack(payload) {
                if env.emit_type == EmitType::ActionResult as u8 {
                    out.push((entry, env));
                }
            }
        }
    }
    out
}

/// Retourne le dernier LogEntry SchedulerRollback (0x0B) de l'agent et sa décodage payload :
/// `(entry, target_seq_from_payload)`. `None` si aucun.
fn last_scheduler_rollback(log: &CausalLog, agent_id: &[u8; 16]) -> Option<(LogEntry, u64)> {
    let entries = all_entries_for_agent(log, agent_id);
    let mut found: Option<(LogEntry, u64)> = None;
    for entry in entries {
        let payload = match entry.emit_payload.as_ref() {
            Some(p) => p, None => continue,
        };
        let env = match EmitEnvelope::from_msgpack(payload) {
            Ok(e) => e, Err(_) => continue,
        };
        if env.emit_type != EmitType::SchedulerRollback as u8 {
            continue;
        }
        // Payload SchedulerRollback : [distance u8 | target_seq u64 LE | caps_invalidated u8]
        if env.payload.len() < 10 {
            continue;
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&env.payload[1..9]);
        let target_seq = u64::from_le_bytes(buf);
        found = Some((entry, target_seq));
    }
    found
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = parse_args();
    let agent_id = parse_agent_id(&args.agent_id_hex);

    std::fs::create_dir_all(&args.db_store).expect("mkdir db-store");
    std::fs::create_dir_all(&args.db_log).expect("mkdir db-log");
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&args.db_store, Some(shared_cache.clone())).expect("ContentStore::open"));
    let log = Arc::new(CausalLog::open(&args.db_log, Some(shared_cache)).expect("CausalLog::open"));

    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let actor = ActorInstance::new_precompiled(
        &engine, &module, agent_id, Arc::clone(&store), Arc::clone(&log),
    ).await.expect("ActorInstance::new_precompiled");

    let mut scheduler = Scheduler::new();
    scheduler.set_log_ref(Arc::clone(&log));
    let tx = scheduler.register(actor);

    eprintln!("[sef2-runner] N={} k_target={} agent_id={}",
        args.n_actions, args.k_target, args.agent_id_hex);

    // ── Phase 1 : N envoyer N actions ───────────────────────────────────────
    // L'agent (AGENT_WAT) appelle commit_barrier + emit pour chaque message.
    // Le mpsc canal est borné (capacity=32) ; tx.send awaite si l'agent traîne.
    for i in 0..args.n_actions {
        let payload = format!("sef2-{i:08}").into_bytes();
        tx.send(Message::data(payload)).await.expect("tx.send Data");
    }

    // Attente : la file de messages doit être drainée. Le canal a une capacité de 32
    // donc à ce stade au plus 32 messages restent en attente. On poll le log pour
    // détecter quand N ActionResult sont visibles.
    let drain_deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let action_results = collect_action_results(&log, &agent_id);
        if action_results.len() as u64 >= args.n_actions {
            break;
        }
        if Instant::now() > drain_deadline {
            eprintln!(
                "[sef2-runner] FATAL : drain timeout, vu {} / {} ActionResult",
                action_results.len(), args.n_actions
            );
            std::process::exit(2);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let action_results = collect_action_results(&log, &agent_id);
    let n_actual = action_results.len() as u64;
    eprintln!("[sef2-runner] phase 1 OK : {} ActionResult observés", n_actual);

    // ── Capture du hash à l'action k_target ────────────────────────────────
    // Convention : action i (1-indexé) ↔ envelope.seq = i - 1.
    let k = args.k_target;
    let target_seq_logical = k - 1;
    let target_entry = action_results.iter().find(|(_, env)| env.seq == target_seq_logical);
    let (target_log_entry, _target_env) = match target_entry {
        Some(t) => t.clone(),
        None => {
            eprintln!(
                "[sef2-runner] FATAL : aucun ActionResult avec seq={} (action #{})",
                target_seq_logical, k
            );
            std::process::exit(2);
        }
    };
    let hash_at_k: [u8; 32] = target_log_entry.hash_after;
    eprintln!("[sef2-runner] hash_at_k = {} (action #{}, target_seq={})",
        hex_encode(&hash_at_k), k, target_seq_logical);

    // hash_at_n : dernier ActionResult (action N) — pour traçabilité du rapport.
    let hash_at_n = action_results
        .iter()
        .find(|(_, env)| env.seq == args.n_actions - 1)
        .map(|(e, _)| e.hash_after)
        .unwrap_or([0u8; 32]);

    // ── Phase 2 : rollback ─────────────────────────────────────────────────
    eprintln!("[sef2-runner] phase 2 : Scheduler::rollback(target_seq={})", target_seq_logical);
    let rollback_start = Instant::now();
    if let Err(e) = scheduler.rollback(&agent_id, target_seq_logical).await {
        eprintln!("[sef2-runner] FATAL : Scheduler::rollback échoué : {e}");
        std::process::exit(2);
    }

    // Poll pour observer le SchedulerRollback dans le log.
    let rb_deadline = Instant::now() + Duration::from_secs(5);
    let (rb_entry, rb_target_seq, rollback_observed_ms) = loop {
        if let Some((entry, ts)) = last_scheduler_rollback(&log, &agent_id) {
            let elapsed = rollback_start.elapsed().as_millis() as u64;
            break (entry, ts, elapsed);
        }
        if Instant::now() > rb_deadline {
            eprintln!("[sef2-runner] FATAL : aucun SchedulerRollback observé après 5s");
            std::process::exit(2);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    eprintln!(
        "[sef2-runner] SchedulerRollback observé après {} ms, target_seq_payload={}, hash_after={}",
        rollback_observed_ms, rb_target_seq, hex_encode(&rb_entry.hash_after)
    );

    // ── Phase 3 : action supplémentaire post-rollback (P-δ) ─────────────────
    // Envoyer un message Data ; le `commit_barrier` du nouveau message prendra
    // comme parent le snapshot restauré → `hash_before` du LogEntry doit
    // valoir hash_at_k.
    let post_payload = b"sef2-post".to_vec();
    if let Err(e) = tx.send(Message::data(post_payload)).await {
        eprintln!("[sef2-runner] FATAL : tx.send post-rollback : {e}");
        std::process::exit(2);
    }

    // Attendre l'apparition du nouveau ActionResult (le N+1-ième).
    let post_deadline = Instant::now() + Duration::from_secs(5);
    let post_entry: LogEntry = loop {
        let ars = collect_action_results(&log, &agent_id);
        if ars.len() as u64 > n_actual {
            // Le dernier ActionResult est notre cible.
            break ars.last().unwrap().0.clone();
        }
        if Instant::now() > post_deadline {
            eprintln!("[sef2-runner] FATAL : pas de ActionResult post-rollback en 5s");
            std::process::exit(2);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    // ── Vérification des 4 propriétés ──────────────────────────────────────
    // P-α : observed_after_rollback = hash_at_k.
    //   Le SchedulerRollback log entry porte hash_after = snap_id du snapshot
    //   après action k (target_snap dans run_loop). C'est notre observed.
    let p_alpha = rb_entry.hash_after == hash_at_k;

    // P-β : SnapshotHeader du target a seq = target_seq_logical.
    let target_header = store
        .get_header(&hash_at_k)
        .ok()
        .flatten();
    let p_beta = match &target_header {
        Some(h) => h.seq == target_seq_logical,
        None => false,
    };

    // P-γ : payload SchedulerRollback porte target_seq = target_seq_logical.
    let p_gamma = rb_target_seq == target_seq_logical;

    // P-δ : nouveau ActionResult a hash_before = hash_at_k.
    let p_delta = post_entry.hash_before == hash_at_k;

    // P-ε (budget P2) : rollback observé sous le budget ms.
    let p_budget = rollback_observed_ms <= args.rollback_budget_ms;

    let all_pass = p_alpha && p_beta && p_gamma && p_delta && p_budget;

    // ── Rapport ────────────────────────────────────────────────────────────
    println!("=== SEF-2 verify ===");
    println!("agent_id              : {}", args.agent_id_hex);
    println!("n_actions             : {}", args.n_actions);
    println!("k_target              : {} (target_seq logique = {})", args.k_target, target_seq_logical);
    println!("hash_at_k             : {}", hex_encode(&hash_at_k));
    println!("hash_at_n             : {}", hex_encode(&hash_at_n));
    println!("rollback hash_after   : {}", hex_encode(&rb_entry.hash_after));
    println!("rollback target_seq   : {} (payload SchedulerRollback)", rb_target_seq);
    println!("post-action hash_before: {}", hex_encode(&post_entry.hash_before));
    println!("rollback duration     : {} ms (budget {} ms)", rollback_observed_ms, args.rollback_budget_ms);
    println!("--- propriétés ---");
    println!("  P-α  observed_after_rollback == hash_at_k       : {}", if p_alpha { "pass" } else { "FAIL" });
    println!("  P-β  target SnapshotHeader.seq == target_seq    : {}", if p_beta  { "pass" } else { "FAIL" });
    println!("  P-γ  payload.target_seq == target_seq           : {}", if p_gamma { "pass" } else { "FAIL" });
    println!("  P-δ  post-rollback action.hash_before == hash_at_k : {}", if p_delta { "pass" } else { "FAIL" });
    println!("  P-ε  rollback duration ≤ budget                 : {}", if p_budget { "pass" } else { "FAIL" });
    println!("verdict               : {}", if all_pass { "pass" } else { "fail" });

    // ── Rapport JSON ───────────────────────────────────────────────────────
    let timestamp = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        // ISO 8601 minimaliste (UTC) — équivalent au format des autres scénarios.
        let tm_secs = secs;
        let days = tm_secs / 86_400;
        let h = (tm_secs % 86_400) / 3600;
        let m = (tm_secs % 3600) / 60;
        let s = tm_secs % 60;
        // Conversion days→Y-M-D : on appelle `date` externe via fs si possible — sinon
        // on stocke juste l'unix timestamp et la date approximative. Pour rester sans
        // dépendance externe, on émet une chaîne ISO datant du run_id du caller.
        let _ = days; let _ = h; let _ = m; let _ = s;
        format!("epoch:{}", tm_secs)
    };

    let report_json = format!(
        "{{\n\
  \"timestamp\": \"{ts}\",\n\
  \"scenario\": \"S7-rollback-equivalence\",\n\
  \"agent_id\": \"{aid}\",\n\
  \"n_actions\": {n},\n\
  \"k_target\": {k},\n\
  \"target_seq_logical\": {tsl},\n\
  \"hash_at_k\": \"{hk}\",\n\
  \"hash_at_n\": \"{hn}\",\n\
  \"rollback_hash_after\": \"{rha}\",\n\
  \"rollback_target_seq_payload\": {rts},\n\
  \"post_action_hash_before\": \"{phb}\",\n\
  \"rollback_duration_ms\": {dur},\n\
  \"rollback_budget_ms\": {budget},\n\
  \"properties\": {{\n\
    \"P_alpha_hash_after_eq_hash_at_k\": {a},\n\
    \"P_beta_target_snapshot_seq\": {b},\n\
    \"P_gamma_payload_target_seq\": {g},\n\
    \"P_delta_post_action_parent\": {d},\n\
    \"P_epsilon_rollback_within_budget\": {e}\n\
  }},\n\
  \"verdict\": \"{v}\"\n\
}}\n",
        ts = timestamp,
        aid = args.agent_id_hex,
        n = args.n_actions,
        k = args.k_target,
        tsl = target_seq_logical,
        hk = hex_encode(&hash_at_k),
        hn = hex_encode(&hash_at_n),
        rha = hex_encode(&rb_entry.hash_after),
        rts = rb_target_seq,
        phb = hex_encode(&post_entry.hash_before),
        dur = rollback_observed_ms,
        budget = args.rollback_budget_ms,
        a = p_alpha,
        b = p_beta,
        g = p_gamma,
        d = p_delta,
        e = p_budget,
        v = if all_pass { "pass" } else { "fail" },
    );
    std::fs::write(&args.out_report, report_json).expect("write --out-report");

    // Drop tx pour permettre à run_loop de finir proprement.
    drop(tx);
    // Petite latence pour laisser la task se conclure (sinon RocksDB peut warn sur drop).
    tokio::time::sleep(Duration::from_millis(50)).await;

    std::process::exit(if all_pass { 0 } else { 1 });
}
