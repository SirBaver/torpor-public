// sef6-runner — binaire de test SEF-6 (déterminisme de transition d'état — P5).
//
// CONTRAT P5 (spec/02-properties.md §P5) :
//   « Pour tout agent dont l'exécution ne dépend que des messages reçus et des
//     primitives explicites du système, deux instances de l'agent initialisées
//     avec un état identique et soumises à la même séquence de messages dans le
//     même ordre produisent la même séquence de messages émis et le même état
//     final (vérifié par hash). »
//
// MÉTHODE :
//   1. Deux instances distinctes (ContentStore + CausalLog séparés sur disque)
//      sont créées avec le même `agent_id`, le même module WASM (AGENT_WAT),
//      et la même horloge logique (`LogicalClock` initialisée à la même valeur).
//   2. La même séquence de N=1000 messages `Data` (payloads bytewise identiques)
//      est envoyée à chacune.
//   3. Après drain complet, on compare :
//        P-α  hash final du ContentStore : `last_snapshot` après N actions
//             est identique entre instances A et B.
//        P-β  séquence de messages émis : la liste ordonnée des `action_id`
//             (clés RocksDB du CausalLog) est bit-à-bit identique entre A et B.
//        P-γ  hash final du log causal : SHA-256 de la concaténation des
//             `action_id` ordonnés est identique (vue agrégée de P-β).
//
// PORTÉE DE LA PROPRIÉTÉ (épistémique) :
//   - L'horloge wall-clock est *substituée* par LogicalClock (S6 — substrate
//     requirement). Sans cette substitution, P5 n'est pas vérifiable et SEF-6
//     est classé hors-périmètre (cf. spec §P5 « P5 est une garantie
//     conditionnelle »).
//   - La séquence d'inputs est fixée : payloads `format!("sef6-{i:08}")` pour
//     i = 0..N. L'ordre est garanti par le canal mpsc borné de Tokio + run-loop
//     séquentiel (S5).
//   - Les sources non-déterministes externes (inférence stochastique, aléas,
//     réseau) ne sont *pas* exercées par AGENT_WAT — seul commit_barrier + emit
//     sont appelés. La vérification de SEF-6 sur un module avec agent_infer
//     requerrait un backend mocké déterministe (hors scope de cette mesure).
//
// EXIT CODES :
//   0 — pass (3 propriétés tiennent sur les deux instances)
//   1 — fail (au moins une propriété viole P5)
//   2 — erreur arguments / I/O / orchestration

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use os_poc_causal_log::CausalLog;
use os_poc_runtime::actor::{ActorInstance, Message, AGENT_WAT};
use os_poc_runtime::clock::{Clock, LogicalClock};
use os_poc_runtime::make_engine;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_store::{ContentStore, Cache};
use sha2::{Digest, Sha256};
use wasmtime::Module;

#[derive(Debug)]
struct Args {
    db_root: PathBuf,
    agent_id_hex: String,
    n_actions: u64,
    clock_start: u64,
    out_report: PathBuf,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "sef6-runner — SEF-6 déterminisme de transition d'état (P5 — spec/02 §P5)\n\
\n\
USAGE:\n\
    sef6-runner --db-root <PATH> --agent-id <HEX32>\n\
                --n-actions <N>\n\
                [--clock-start <MS>]\n\
                [--out-report <PATH>]\n\
\n\
ARGS:\n\
    --db-root <PATH>          Répertoire racine ; deux sous-répertoires sont créés :\n\
                              <PATH>/instance-a/{{store,log}} et <PATH>/instance-b/{{store,log}}.\n\
    --agent-id <HEX32>        Hex 32-caractères = 16 octets. Identique pour A et B.\n\
    --n-actions <N>           Nombre total d'actions à exécuter (typiquement 1000).\n\
    --clock-start <MS>        Valeur initiale du LogicalClock (défaut 1700000000000).\n\
                              Identique pour A et B — c'est la racine de l'égalité.\n\
    --out-report <PATH>       Rapport JSON (défaut : report.json dans cwd).\n\
\n\
EXIT CODES:\n\
    0 = pass (P-α, P-β, P-γ tiennent)\n\
    1 = fail (au moins une propriété viole P5)\n\
    2 = erreur arguments / I/O / orchestration\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_root: Option<PathBuf> = None;
    let mut agent_id_hex: Option<String> = None;
    let mut n_actions: Option<u64> = None;
    let mut clock_start: u64 = 1_700_000_000_000;
    let mut out_report: PathBuf = PathBuf::from("report.json");

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-root" => { i += 1; db_root = Some(PathBuf::from(&raw[i])); }
            "--agent-id" => { i += 1; agent_id_hex = Some(raw[i].clone()); }
            "--n-actions" => {
                i += 1;
                n_actions = Some(raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-actions doit être un entier"); std::process::exit(2);
                }));
            }
            "--clock-start" => {
                i += 1;
                clock_start = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--clock-start doit être un entier u64"); std::process::exit(2);
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

    Args {
        db_root: db_root.unwrap_or_else(|| print_usage_and_exit(2)),
        agent_id_hex: agent_id_hex.unwrap_or_else(|| print_usage_and_exit(2)),
        n_actions: n_actions.unwrap_or_else(|| print_usage_and_exit(2)),
        clock_start,
        out_report,
    }
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

fn hex_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

/// Exécute une instance : crée store + log, lance l'agent, envoie N messages,
/// drain, retourne (last_snapshot_hex, action_ids_ordered).
async fn run_instance(
    label: &str,
    db_dir: &PathBuf,
    agent_id: [u8; 16],
    n_actions: u64,
    clock_start: u64,
) -> Result<(String, Vec<[u8; 32]>), String> {
    let db_store = db_dir.join("store");
    let db_log = db_dir.join("log");
    std::fs::create_dir_all(&db_store).map_err(|e| format!("mkdir store ({label}): {e}"))?;
    std::fs::create_dir_all(&db_log).map_err(|e| format!("mkdir log ({label}): {e}"))?;
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&db_store, Some(shared_cache.clone())).map_err(|e| format!("ContentStore::open ({label}): {e}"))?);
    let log = Arc::new(CausalLog::open(&db_log, Some(shared_cache)).map_err(|e| format!("CausalLog::open ({label}): {e}"))?);

    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT).map_err(|e| format!("compile AGENT_WAT ({label}): {e}"))?;

    // S6 : horloge logique partagée par l'instance — détermine bit-à-bit chaque ts_ms / ts_us.
    let clock: Arc<dyn Clock> = Arc::new(LogicalClock::new(clock_start));

    let actor = ActorInstance::new_precompiled_with_clock(
        &engine, &module, agent_id,
        Arc::clone(&store), Arc::clone(&log),
        Arc::clone(&clock),
    ).await.map_err(|e| format!("new_precompiled_with_clock ({label}): {e}"))?;

    let mut scheduler = Scheduler::new();
    let tx = scheduler.register(actor);

    eprintln!("[sef6-runner {label}] envoi de {n_actions} messages...");
    for i in 0..n_actions {
        let payload = format!("sef6-{i:08}").into_bytes();
        tx.send(Message::data(payload)).await.map_err(|e| format!("tx.send ({label}, i={i}): {e}"))?;
    }

    // Drain : on attend que la séquence d'ActionResult complète soit visible dans le log.
    // Le canal mpsc a une capacité de 32 — à la sortie de la boucle d'envoi, jusqu'à 32
    // messages restent en attente. On poll le log secondaire index par agent_id.
    let drain_deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let ids = log.query_by_agent_range(&agent_id, None, None)
            .map_err(|e| format!("query_by_agent_range: {e}"))?;
        // AGENT_WAT émet pour chaque message Data : commit_barrier + emit(ActionResult),
        // plus run_loop log Lifecycle Active + Spawned/Terminated en boundary.
        // On compte uniquement les action_ids visibles : il y a au moins n_actions + 2
        // (Spawned initial + Active par message). On déclenche dès qu'on dépasse
        // n_actions * 2 pour donner un marqueur stable.
        // Critère de stabilité : vu le même nombre 5 polls de suite.
        let observed = ids.len() as u64;
        if observed >= 2 * n_actions {
            // Vérifie stabilité (rien de nouveau arrive en 100 ms)
            tokio::time::sleep(Duration::from_millis(100)).await;
            let ids2 = log.query_by_agent_range(&agent_id, None, None)
                .map_err(|e| format!("query_by_agent_range (stability): {e}"))?;
            if ids2.len() == ids.len() {
                break;
            }
        }
        if Instant::now() > drain_deadline {
            return Err(format!("[{label}] drain timeout : vu {observed} action_ids"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Drop tx pour terminer la run_loop proprement.
    drop(tx);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Récupère la séquence finale d'action_ids dans l'ordre (agent_ts CF = ts_ms BE).
    let ids = log.query_by_agent_range(&agent_id, None, None)
        .map_err(|e| format!("query_by_agent_range (final): {e}"))?;
    eprintln!("[sef6-runner {label}] drain OK, {} action_ids collectés", ids.len());

    // Récupère le hash final du ContentStore : on retrouve via le LogEntry du dernier emit.
    // hash_after du dernier LogEntry ActionResult == last_snapshot final.
    let mut last_snapshot_hex = String::from("none");
    for action_id in ids.iter().rev() {
        if let Ok(Some(entry)) = log.get(action_id) {
            if entry.hash_after != [0u8; 32] {
                last_snapshot_hex = hex_encode(&entry.hash_after);
                break;
            }
        }
    }

    Ok((last_snapshot_hex, ids))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = parse_args();
    let agent_id = parse_agent_id(&args.agent_id_hex);

    let dir_a = args.db_root.join("instance-a");
    let dir_b = args.db_root.join("instance-b");

    eprintln!("[sef6-runner] agent_id={} N={} clock_start={}",
        args.agent_id_hex, args.n_actions, args.clock_start);

    // Instance A
    let (snap_a, ids_a) = match run_instance("A", &dir_a, agent_id, args.n_actions, args.clock_start).await {
        Ok(v) => v,
        Err(e) => { eprintln!("[sef6-runner] FATAL instance A : {e}"); std::process::exit(2); }
    };

    // Instance B
    let (snap_b, ids_b) = match run_instance("B", &dir_b, agent_id, args.n_actions, args.clock_start).await {
        Ok(v) => v,
        Err(e) => { eprintln!("[sef6-runner] FATAL instance B : {e}"); std::process::exit(2); }
    };

    // P-α : last_snapshot final identique
    let p_alpha = snap_a == snap_b;

    // P-β : séquence action_id bit-à-bit identique
    let p_beta = ids_a == ids_b;

    // P-γ : hash agrégé de la séquence (utile pour les rapports compacts)
    let hash_seq = |ids: &[[u8; 32]]| -> [u8; 32] {
        let mut h = Sha256::new();
        for id in ids { h.update(id); }
        h.finalize().into()
    };
    let hash_a = hash_seq(&ids_a);
    let hash_b = hash_seq(&ids_b);
    let p_gamma = hash_a == hash_b;

    let all_pass = p_alpha && p_beta && p_gamma;

    // Diagnostic : si fail, identifier le premier point de divergence.
    let (first_div_idx, first_div_a, first_div_b) = if !p_beta {
        let mut idx: i64 = -1;
        let mut a_id = [0u8; 32];
        let mut b_id = [0u8; 32];
        let n_min = ids_a.len().min(ids_b.len());
        for i in 0..n_min {
            if ids_a[i] != ids_b[i] {
                idx = i as i64;
                a_id = ids_a[i];
                b_id = ids_b[i];
                break;
            }
        }
        (idx, a_id, b_id)
    } else {
        (-1, [0u8; 32], [0u8; 32])
    };

    println!("=== SEF-6 verify ===");
    println!("agent_id              : {}", args.agent_id_hex);
    println!("n_actions             : {}", args.n_actions);
    println!("clock_start           : {}", args.clock_start);
    println!("instance A last_snap  : {}", snap_a);
    println!("instance B last_snap  : {}", snap_b);
    println!("instance A action_ids : {} entrées", ids_a.len());
    println!("instance B action_ids : {} entrées", ids_b.len());
    println!("instance A hash_seq   : {}", hex_encode(&hash_a));
    println!("instance B hash_seq   : {}", hex_encode(&hash_b));
    println!("--- propriétés ---");
    println!("  P-α  last_snapshot(A) == last_snapshot(B)         : {}", if p_alpha { "pass" } else { "FAIL" });
    println!("  P-β  action_ids(A) == action_ids(B) bytewise      : {}", if p_beta  { "pass" } else { "FAIL" });
    println!("  P-γ  SHA256(action_ids(A)) == SHA256(action_ids(B)): {}", if p_gamma { "pass" } else { "FAIL" });
    if !p_beta && first_div_idx >= 0 {
        println!("  premier point de divergence : index {}", first_div_idx);
        println!("    A : {}", hex_encode(&first_div_a));
        println!("    B : {}", hex_encode(&first_div_b));
    }
    println!("verdict               : {}", if all_pass { "pass" } else { "fail" });

    // Rapport JSON
    let timestamp = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        format!("epoch:{}", secs)
    };
    let report_json = format!(
        "{{\n\
  \"timestamp\": \"{ts}\",\n\
  \"scenario\": \"S8-determinism\",\n\
  \"property\": \"P5\",\n\
  \"sef\": \"SEF-6\",\n\
  \"agent_id\": \"{aid}\",\n\
  \"n_actions\": {n},\n\
  \"clock_start\": {clk},\n\
  \"instance_a\": {{\n\
    \"last_snapshot\": \"{sa}\",\n\
    \"action_ids_count\": {ca},\n\
    \"hash_seq\": \"{ha}\"\n\
  }},\n\
  \"instance_b\": {{\n\
    \"last_snapshot\": \"{sb}\",\n\
    \"action_ids_count\": {cb},\n\
    \"hash_seq\": \"{hb}\"\n\
  }},\n\
  \"properties\": {{\n\
    \"P_alpha_last_snapshot_eq\": {a},\n\
    \"P_beta_action_ids_eq\": {b},\n\
    \"P_gamma_hash_seq_eq\": {g}\n\
  }},\n\
  \"first_divergence_index\": {fdi},\n\
  \"verdict\": \"{v}\"\n\
}}\n",
        ts = timestamp,
        aid = args.agent_id_hex,
        n = args.n_actions,
        clk = args.clock_start,
        sa = snap_a, sb = snap_b,
        ca = ids_a.len(), cb = ids_b.len(),
        ha = hex_encode(&hash_a), hb = hex_encode(&hash_b),
        a = p_alpha, b = p_beta, g = p_gamma,
        fdi = first_div_idx,
        v = if all_pass { "pass" } else { "fail" },
    );
    std::fs::write(&args.out_report, report_json).expect("write --out-report");

    std::process::exit(if all_pass { 0 } else { 1 });
}
