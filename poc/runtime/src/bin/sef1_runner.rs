// sef1-runner — binaire de test SEF-1 (persistance d'état après redémarrage).
//
// CONTRAT (spec/benchmarks/equivalence-scenarios.md §SEF-1) :
//   « Un agent persiste son état dans le store local. Après arrêt propre du runtime
//     (shutdown du canal Tokio), le store et le log sont fermés (RocksDB file locks
//     libérés), puis réouverts. L'état persisted est bit-à-bit identique. »
//
// MÉTHODE en deux phases simulant le cycle arrêt/redémarrage :
//
//   Phase 1 (pré-arrêt) :
//     - ContentStore + CausalLog ouverts sur des chemins disque réels.
//     - Agent WASM minimal (AGENT_WAT) exécute N = 100 actions.
//       Chaque action déclenche commit_barrier + emit → block + snapshot + LogEntry.
//     - On capture :
//         H_before       = hash_after du dernier ActionResult (= last_snapshot de l'agent)
//         data_hash      = SnapshotHeader.data_hash de H_before (hash du bloc de 64 B)
//         block_content  = ContentStore.get_block(data_hash) (64 octets)
//         N_log          = nombre total d'action_ids dans le log secondaire
//         seq_before     = N_ACTIONS (seq interne de l'agent = nb de commit_barrier)
//         action_id_last = action_id du dernier ActionResult
//     - tx est drop() ; handle.await termine la task run_loop (emit Terminated + drop Arcs).
//     - drop Arc<store>, drop Arc<log> → RocksDB file locks libérés.
//
//   Phase 2 (post-redémarrage) :
//     - Mêmes chemins, réouverture ContentStore + CausalLog (nouveaux Arcs).
//     - P-α : get_header(H_before) retourne Some(header) avec seq + data_hash identiques.
//     - P-β : query_by_agent_range(agent_id).len() >= N_log (log intègre, toutes entrées lisibles).
//     - P-γ : get_block(data_hash) retourne les mêmes 64 octets (contenu bloc identique).
//     - P-δ : ActorInstance::restore_from_evicted(evicted_state) + une action supplémentaire →
//             le nouvel ActionResult a hash_before == H_before (continuité causale après restart).
//
// EXIT CODES :
//   0 — pass (4 propriétés tiennent)
//   1 — fail (au moins une propriété viole SEF-1)
//   2 — erreur arguments / I/O / orchestration

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType, LogEntry};
use os_poc_runtime::actor::{ActorInstance, EvictedState, Message, run_loop, AGENT_WAT};
use os_poc_runtime::make_engine;
use os_poc_store::{ContentStore, Cache};
use wasmtime::Module;

// ── Arguments ────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Args {
    db_store: PathBuf,
    db_log:   PathBuf,
    agent_id_hex: String,
    n_actions: u64,
    out_report: PathBuf,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "sef1-runner — SEF-1 persistance d'état après redémarrage\n\
\n\
USAGE:\n\
    sef1-runner --db-store <PATH> --db-log <PATH> --agent-id <HEX32>\n\
                [--n-actions <N>]\n\
                [--out-report <PATH>]\n\
\n\
ARGS:\n\
    --db-store <PATH>    Répertoire ContentStore (créé si absent).\n\
    --db-log <PATH>      Répertoire CausalLog (créé si absent).\n\
    --agent-id <HEX32>   Hex 32-caractères = 16 octets.\n\
    --n-actions <N>      Nombre d'actions phase 1 (défaut 100).\n\
    --out-report <PATH>  Rapport JSON (défaut : report.json dans cwd).\n\
\n\
EXIT CODES:\n\
    0 = pass (P-α, P-β, P-γ, P-δ tiennent)\n\
    1 = fail (au moins une propriété viole SEF-1)\n\
    2 = erreur arguments / I/O / orchestration\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store: Option<PathBuf> = None;
    let mut db_log:   Option<PathBuf> = None;
    let mut agent_id_hex: Option<String> = None;
    let mut n_actions: u64 = 100;
    let mut out_report = PathBuf::from("report.json");

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store"   => { i += 1; db_store   = Some(PathBuf::from(&raw[i])); }
            "--db-log"     => { i += 1; db_log     = Some(PathBuf::from(&raw[i])); }
            "--agent-id"   => { i += 1; agent_id_hex = Some(raw[i].clone()); }
            "--n-actions"  => {
                i += 1;
                n_actions = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-actions doit être un entier u64");
                    std::process::exit(2);
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
        db_store:    db_store.unwrap_or_else(|| print_usage_and_exit(2)),
        db_log:      db_log.unwrap_or_else(||   print_usage_and_exit(2)),
        agent_id_hex: agent_id_hex.unwrap_or_else(|| print_usage_and_exit(2)),
        n_actions,
        out_report,
    }
}

fn parse_agent_id(hex: &str) -> [u8; 16] {
    if hex.len() != 32 {
        eprintln!("--agent-id doit être exactement 32 caractères hex (16 octets)");
        std::process::exit(2);
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(pair).expect("hex ASCII");
        out[i] = u8::from_str_radix(s, 16).unwrap_or_else(|_| {
            eprintln!("--agent-id : caractère non hex '{s}'");
            std::process::exit(2);
        });
    }
    out
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, byte| {
        s.push_str(&format!("{:02x}", byte));
        s
    })
}

// ── Helpers log ──────────────────────────────────────────────────────────────

fn all_entries_for_agent(log: &CausalLog, agent_id: &[u8; 16]) -> Vec<LogEntry> {
    let ids = log.query_by_agent_range(agent_id, None, None).unwrap_or_default();
    ids.iter().filter_map(|id| log.get(id).ok().flatten()).collect()
}

fn collect_action_results(log: &CausalLog, agent_id: &[u8; 16]) -> Vec<(LogEntry, EmitEnvelope)> {
    all_entries_for_agent(log, agent_id)
        .into_iter()
        .filter_map(|entry| {
            let payload = entry.emit_payload.as_ref()?;
            let env = EmitEnvelope::from_msgpack(payload).ok()?;
            if env.emit_type == EmitType::ActionResult as u8 {
                Some((entry, env))
            } else {
                None
            }
        })
        .collect()
}

// ── Résultats phase 1 ────────────────────────────────────────────────────────

struct Phase1Result {
    /// Hash du dernier snapshot (last_snapshot de l'agent) = H_before.
    h_before:        [u8; 32],
    /// Seq interne de l'agent après N actions (= N_ACTIONS).
    seq_before:      u64,
    /// action_id du dernier ActionResult (pour EvictedState.last_action).
    action_id_last:  [u8; 32],
    /// Hash du bloc de données stocké dans ContentStore (SnapshotHeader.data_hash de H_before).
    data_hash:       [u8; 32],
    /// Contenu brut du bloc de 64 octets (agent_id || seq_le || zeros).
    block_content:   Vec<u8>,
    /// seq logique du dernier ActionResult (env.seq = seq_before - 1).
    last_action_seq: u64,
    /// Nombre total d'action_ids dans le log secondaire avant shutdown.
    n_log_entries:   usize,
    /// SnapshotHeader.parent de H_before (pour comparaison P-α après restart).
    header_parent:   Option<[u8; 32]>,
}

// ── Phase 1 : exécution pré-arrêt ────────────────────────────────────────────

async fn run_phase1(
    db_store:  &PathBuf,
    db_log:    &PathBuf,
    agent_id:  [u8; 16],
    n_actions: u64,
) -> Result<Phase1Result, String> {
    std::fs::create_dir_all(db_store).map_err(|e| format!("mkdir store: {e}"))?;
    std::fs::create_dir_all(db_log).map_err(|e| format!("mkdir log: {e}"))?;

    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(db_store, Some(shared_cache.clone()))
            .map_err(|e| format!("ContentStore::open: {e}"))?,
    );
    let log = Arc::new(
        CausalLog::open(db_log, Some(shared_cache))
            .map_err(|e| format!("CausalLog::open: {e}"))?,
    );

    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT)
        .map_err(|e| format!("compile AGENT_WAT: {e}"))?;

    let actor = ActorInstance::new_precompiled(
        &engine, &module, agent_id,
        Arc::clone(&store), Arc::clone(&log),
    )
    .await
    .map_err(|e| format!("ActorInstance::new_precompiled: {e}"))?;

    // Spawn directement (sans Scheduler) pour garder le JoinHandle et pouvoir l'await.
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let handle = tokio::spawn(run_loop(actor, rx));

    eprintln!("[sef1 phase1] envoi de {n_actions} messages...");
    for i in 0..n_actions {
        let payload = format!("sef1-{i:08}").into_bytes();
        tx.send(Message::data(payload)).await
            .map_err(|e| format!("tx.send (i={i}): {e}"))?;
    }

    // Drain : attendre que N ActionResult soient visibles dans le log.
    let drain_deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let ars = collect_action_results(&log, &agent_id);
        if ars.len() as u64 >= n_actions {
            break;
        }
        if Instant::now() > drain_deadline {
            return Err(format!(
                "drain timeout : {} / {} ActionResult visibles",
                ars.len(), n_actions
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let action_results = collect_action_results(&log, &agent_id);
    eprintln!("[sef1 phase1] drain OK, {} ActionResult", action_results.len());

    // Dernier ActionResult (seq le plus élevé).
    let (last_entry, last_env) = action_results
        .iter()
        .max_by_key(|(_, env)| env.seq)
        .ok_or("aucun ActionResult")?
        .clone();

    let h_before = last_entry.hash_after;
    let last_action_seq = last_env.seq;
    // seq interne = env.seq + 1 (commit_barrier incrémente après avoir stocké l'ancien seq).
    let seq_before = last_action_seq + 1;
    let action_id_last = last_entry.action_id();

    // Lire SnapshotHeader pour extraire data_hash.
    let header = store
        .get_header(&h_before)
        .map_err(|e| format!("get_header: {e}"))?
        .ok_or_else(|| format!("H_before {} non trouvé dans le store", hex_encode(&h_before)))?;
    let data_hash   = header.data_hash;
    let header_parent = header.parent;

    // Lire le bloc de données (64 octets : agent_id || seq_le || zeros).
    let block_content = store
        .get_block(&data_hash)
        .map_err(|e| format!("get_block: {e}"))?
        .ok_or_else(|| format!("bloc {} non trouvé", hex_encode(&data_hash)))?;

    let n_log_entries = log.query_by_agent_range(&agent_id, None, None)
        .map_err(|e| format!("query_by_agent_range: {e}"))?.len();
    eprintln!(
        "[sef1 phase1] H_before={} seq_before={} n_log={}",
        hex_encode(&h_before), seq_before, n_log_entries
    );

    // Shutdown propre : fermer le canal → run_loop émet Terminated et se termine.
    drop(tx);
    handle.await.map_err(|e| format!("join run_loop: {e}"))?;

    // Les Arcs store + log tombent ici (fin de scope) → RocksDB file locks libérés.
    drop(store);
    drop(log);

    Ok(Phase1Result {
        h_before,
        seq_before,
        action_id_last,
        data_hash,
        block_content,
        last_action_seq,
        n_log_entries,
        header_parent,
    })
}

// ── Phase 2 : vérification post-redémarrage ──────────────────────────────────

struct Phase2Result {
    p_alpha: bool,
    p_beta:  bool,
    p_gamma: bool,
    p_delta: bool,
    n_log_after:         usize,
    header_seq_match:    bool,
    header_parent_match: bool,
    hash_before_post:    [u8; 32],
}

async fn run_phase2(
    db_store:  &PathBuf,
    db_log:    &PathBuf,
    agent_id:  [u8; 16],
    p1:        &Phase1Result,
) -> Result<Phase2Result, String> {
    // Réouverture : chemins identiques, nouveaux Arcs → simule le redémarrage.
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(db_store, Some(shared_cache.clone()))
            .map_err(|e| format!("ContentStore::open (phase2): {e}"))?,
    );
    let log = Arc::new(
        CausalLog::open(db_log, Some(shared_cache))
            .map_err(|e| format!("CausalLog::open (phase2): {e}"))?,
    );
    eprintln!("[sef1 phase2] store + log réouverts");

    // P-α : SnapshotHeader de H_before toujours lisible et cohérent.
    let header_opt = store
        .get_header(&p1.h_before)
        .map_err(|e| format!("get_header (phase2): {e}"))?;
    let (header_seq_match, header_parent_match) = match &header_opt {
        Some(h) => (
            h.seq == p1.last_action_seq,
            h.parent == p1.header_parent,
        ),
        None => (false, false),
    };
    let p_alpha = header_opt.is_some() && header_seq_match && header_parent_match;

    // P-β : toutes les entrées du log sont toujours présentes.
    let n_log_after = log.query_by_agent_range(&agent_id, None, None)
        .map_err(|e| format!("query_by_agent_range (phase2): {e}"))?.len();
    // Après shutdown propre, run_loop a émis Terminated → n_log_after >= n_log_entries.
    let p_beta = n_log_after >= p1.n_log_entries;

    // P-γ : contenu du bloc de données bit-à-bit identique.
    let block_after = store
        .get_block(&p1.data_hash)
        .map_err(|e| format!("get_block (phase2): {e}"))?
        .unwrap_or_default();
    let p_gamma = block_after == p1.block_content;

    eprintln!(
        "[sef1 phase2] P-α={} P-β={} P-γ={} n_log_after={}",
        p_alpha, p_beta, p_gamma, n_log_after
    );

    // P-δ : chaîne causale intacte — un acteur restauré depuis l'état persisté
    //       continue à hash_before == H_before.
    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT)
        .map_err(|e| format!("compile AGENT_WAT (phase2): {e}"))?;

    let evicted = EvictedState {
        id:            agent_id,
        seq:           p1.seq_before,
        last_snapshot: Some(p1.h_before),
        last_action:   Some(p1.action_id_last),
        evicted_at:    std::time::Instant::now(),
    };

    let actor2 = ActorInstance::restore_from_evicted(
        &engine, &module, &evicted,
        Arc::clone(&store), Arc::clone(&log),
    )
    .await
    .map_err(|e| format!("restore_from_evicted: {e}"))?;

    let (tx2, rx2) = tokio::sync::mpsc::channel(32);
    let handle2 = tokio::spawn(run_loop(actor2, rx2));

    // Action supplémentaire post-restart.
    tx2.send(Message::data(b"sef1-post-restart".to_vec())).await
        .map_err(|e| format!("tx2.send: {e}"))?;

    // Attendre l'apparition d'un nouvel ActionResult (au-delà de n_log_entries).
    let deadline = Instant::now() + Duration::from_secs(10);
    let hash_before_post: [u8; 32] = loop {
        let ars = collect_action_results(&log, &agent_id);
        // Il y avait déjà p1.n_log_entries - mais pas forcément des ActionResult.
        // On cherche un ActionResult avec seq > p1.last_action_seq.
        if let Some((entry, _)) = ars.iter().find(|(_, env)| env.seq > p1.last_action_seq) {
            break entry.hash_before;
        }
        if Instant::now() > deadline {
            return Err("timeout : pas de nouvel ActionResult post-restart".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let p_delta = hash_before_post == p1.h_before;
    eprintln!(
        "[sef1 phase2] P-δ={} hash_before_post={} H_before={}",
        p_delta,
        hex_encode(&hash_before_post),
        hex_encode(&p1.h_before)
    );

    drop(tx2);
    handle2.await.map_err(|e| format!("join run_loop2: {e}"))?;

    Ok(Phase2Result {
        p_alpha,
        p_beta,
        p_gamma,
        p_delta,
        n_log_after,
        header_seq_match,
        header_parent_match,
        hash_before_post,
    })
}

// ── main ─────────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = parse_args();
    let agent_id = parse_agent_id(&args.agent_id_hex);

    eprintln!(
        "[sef1-runner] agent_id={} n_actions={}",
        args.agent_id_hex, args.n_actions
    );

    // Phase 1
    let p1 = match run_phase1(&args.db_store, &args.db_log, agent_id, args.n_actions).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[sef1-runner] FATAL phase1 : {e}");
            std::process::exit(2);
        }
    };

    // Phase 2
    let p2 = match run_phase2(&args.db_store, &args.db_log, agent_id, &p1).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[sef1-runner] FATAL phase2 : {e}");
            std::process::exit(2);
        }
    };

    let all_pass = p2.p_alpha && p2.p_beta && p2.p_gamma && p2.p_delta;

    println!("=== SEF-1 verify ===");
    println!("agent_id          : {}", args.agent_id_hex);
    println!("n_actions (ph1)   : {}", args.n_actions);
    println!("H_before          : {}", hex_encode(&p1.h_before));
    println!("seq_before        : {}", p1.seq_before);
    println!("data_hash         : {}", hex_encode(&p1.data_hash));
    println!("n_log (pre)       : {}", p1.n_log_entries);
    println!("n_log (post)      : {}", p2.n_log_after);
    println!("hash_before_post  : {}", hex_encode(&p2.hash_before_post));
    println!("--- propriétés ---");
    println!(
        "  P-α  get_header(H_before) intact après restart  : {}",
        if p2.p_alpha { "pass" } else { "FAIL" }
    );
    if !p2.p_alpha {
        println!("       header_seq_match={}  header_parent_match={}", p2.header_seq_match, p2.header_parent_match);
    }
    println!(
        "  P-β  n_log_after >= n_log_pre                    : {}  ({} >= {})",
        if p2.p_beta { "pass" } else { "FAIL" },
        p2.n_log_after, p1.n_log_entries
    );
    println!(
        "  P-γ  bloc de données bit-à-bit identique          : {}",
        if p2.p_gamma { "pass" } else { "FAIL" }
    );
    println!(
        "  P-δ  hash_before_post == H_before (chaîne intact) : {}",
        if p2.p_delta { "pass" } else { "FAIL" }
    );
    println!("verdict           : {}", if all_pass { "pass" } else { "fail" });

    // Rapport JSON
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let report = format!(
        "{{\n\
  \"timestamp\": \"epoch:{ts}\",\n\
  \"scenario\": \"S13-persistence-restart\",\n\
  \"property\": \"persistance\",\n\
  \"sef\": \"SEF-1\",\n\
  \"agent_id\": \"{aid}\",\n\
  \"n_actions\": {n},\n\
  \"h_before\": \"{hb}\",\n\
  \"seq_before\": {sb},\n\
  \"data_hash\": \"{dh}\",\n\
  \"n_log_pre\": {nlpre},\n\
  \"n_log_post\": {nlpost},\n\
  \"hash_before_post\": \"{hbpost}\",\n\
  \"properties\": {{\n\
    \"P_alpha_header_intact\": {a},\n\
    \"P_beta_log_intact\": {b},\n\
    \"P_gamma_block_content_identical\": {g},\n\
    \"P_delta_chain_continuity\": {d}\n\
  }},\n\
  \"verdict\": \"{v}\"\n\
}}\n",
        ts = ts,
        aid = args.agent_id_hex,
        n = args.n_actions,
        hb = hex_encode(&p1.h_before),
        sb = p1.seq_before,
        dh = hex_encode(&p1.data_hash),
        nlpre = p1.n_log_entries,
        nlpost = p2.n_log_after,
        hbpost = hex_encode(&p2.hash_before_post),
        a = p2.p_alpha,
        b = p2.p_beta,
        g = p2.p_gamma,
        d = p2.p_delta,
        v = if all_pass { "pass" } else { "fail" },
    );
    std::fs::write(&args.out_report, report).expect("write --out-report");

    std::process::exit(if all_pass { 0 } else { 1 });
}
