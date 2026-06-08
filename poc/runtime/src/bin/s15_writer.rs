// s15-writer — harness d'écriture concurrente pour S15-crash-machine-concurrent.
//
// Spawne N_AGENTS threads d'écriture concurrente sur les mêmes ContentStore + CausalLog.
// Chaque thread écrit des commits synthétiques en boucle (put_block + put_snapshot + append).
// Quand le nombre total de commits ackés atteint KILL_THRESHOLD :
//   1. Set kill_flag (les threads arrêtent après leur commit courant).
//   2. sleep(2ms) — fenêtre adversariale : certains threads sont entre put_snapshot et append.
//   3. Collecte les témoins per-agent.
//   4. Sauvegarde witness.json.
//   5. process::exit(1).
//
// ADR-0050 D4 / ADR-0027 §D3 / spec/02 §P6.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};

use os_poc_causal_log::{CausalLog, LogEntry};
use os_poc_runtime::durability::{
    AgentWitness, ConcurrentWitness, IcsrCommit, hex_encode,
};
use os_poc_store::{Cache, ContentStore, SnapshotHeader};

struct Args {
    db_store:        PathBuf,
    db_log:          PathBuf,
    witness:         PathBuf,
    n_agents:        usize,
    commits_per_agent: usize,
    kill_threshold:  usize,
    block_size:      usize,
}

fn print_usage() -> ! {
    eprintln!(
        "s15-writer — harness d'écriture concurrente S15\n\
\n\
USAGE:\n\
    s15-writer --db-store <PATH> --db-log <PATH> --witness <PATH>\n\
               [--n-agents <N>] [--commits-per-agent <C>]\n\
               [--kill-threshold <K>] [--block-size <B>]\n\
\n\
EXIT CODES:\n\
    1 — coupure contrôlée (process::exit, SIGKILL simulé)\n\
    2 — erreur d'argument / I/O\n"
    );
    std::process::exit(2);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store         = None::<PathBuf>;
    let mut db_log           = None::<PathBuf>;
    let mut witness          = None::<PathBuf>;
    let mut n_agents         = 4usize;
    let mut commits_per_agent = 25usize;
    let mut kill_threshold   = 40usize;
    let mut block_size       = 64usize;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store"          => { i += 1; db_store  = Some(PathBuf::from(&raw[i])); }
            "--db-log"            => { i += 1; db_log    = Some(PathBuf::from(&raw[i])); }
            "--witness"           => { i += 1; witness   = Some(PathBuf::from(&raw[i])); }
            "--n-agents"          => { i += 1; n_agents          = raw[i].parse().unwrap_or_else(|_| { eprintln!("--n-agents : entier"); std::process::exit(2); }); }
            "--commits-per-agent" => { i += 1; commits_per_agent = raw[i].parse().unwrap_or_else(|_| { eprintln!("--commits-per-agent : entier"); std::process::exit(2); }); }
            "--kill-threshold"    => { i += 1; kill_threshold    = raw[i].parse().unwrap_or_else(|_| { eprintln!("--kill-threshold : entier"); std::process::exit(2); }); }
            "--block-size"        => { i += 1; block_size        = raw[i].parse().unwrap_or_else(|_| { eprintln!("--block-size : entier"); std::process::exit(2); }); }
            "-h" | "--help" => print_usage(),
            other => { eprintln!("Argument inconnu: {other}"); print_usage(); }
        }
        i += 1;
    }

    Args {
        db_store:         db_store.unwrap_or_else(|| print_usage()),
        db_log:           db_log.unwrap_or_else(||   print_usage()),
        witness:          witness.unwrap_or_else(||  print_usage()),
        n_agents,
        commits_per_agent,
        kill_threshold,
        block_size,
    }
}

fn agent_id_from_index(idx: usize) -> [u8; 16] {
    let mut id = [0u8; 16];
    // Bytes 0..2 = idx as little-endian u16 ; reste = 0.
    id[0] = (idx & 0xFF) as u8;
    id[1] = ((idx >> 8) & 0xFF) as u8;
    id
}

fn main() {
    let args = parse_args();

    std::fs::create_dir_all(&args.db_store).unwrap_or_else(|e| {
        eprintln!("mkdir db-store: {e}"); std::process::exit(2);
    });
    std::fs::create_dir_all(&args.db_log).unwrap_or_else(|e| {
        eprintln!("mkdir db-log: {e}"); std::process::exit(2);
    });

    let shared_cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(&args.db_store, Some(shared_cache.clone())).unwrap_or_else(|e| {
            eprintln!("ContentStore::open: {e}"); std::process::exit(2);
        }),
    );
    let log = Arc::new(
        CausalLog::open(&args.db_log, Some(shared_cache)).unwrap_or_else(|e| {
            eprintln!("CausalLog::open: {e}"); std::process::exit(2);
        }),
    );

    // kill_flag n'est PAS vérifié dans les threads : les threads écrivent sans
    // s'arrêter jusqu'à ce que process::exit(1) les tue. Cela garantit qu'au
    // moment du kill, certains threads sont genuinement entre put_snapshot et
    // append (fenêtre adversariale).
    let total_acked  = Arc::new(AtomicUsize::new(0));
    // Per-agent acked commits (protégés par Mutex individuel).
    let per_agent: Vec<Arc<Mutex<Vec<IcsrCommit>>>> = (0..args.n_agents)
        .map(|_| Arc::new(Mutex::new(Vec::new())))
        .collect();

    eprintln!(
        "[s15-writer] démarrage : {} agents, {} commits/agent, kill_threshold={}",
        args.n_agents, args.commits_per_agent, args.kill_threshold
    );

    let mut handles = Vec::new();
    for agent_idx in 0..args.n_agents {
        let store       = Arc::clone(&store);
        let log         = Arc::clone(&log);
        let total_acked = Arc::clone(&total_acked);
        let acked_vec   = Arc::clone(&per_agent[agent_idx]);
        let agent_id    = agent_id_from_index(agent_idx);
        let block_size  = args.block_size;
        let commits_max = args.commits_per_agent;

        let handle = std::thread::spawn(move || {
            let data = vec![0xCDu8; block_size];
            let data_h = match store.put_block(&data) {
                Ok(h) => h,
                Err(e) => { eprintln!("[thread-{}] put_block initial: {e}", agent_idx); return; }
            };

            let mut parent_snap: Option<[u8; 32]> = None;
            let mut prev_action: Option<[u8; 32]> = None;
            let mut hash_before = [0u8; 32];

            // Boucle infinie — le thread ne vérifie pas de kill_flag.
            // process::exit(1) depuis le main thread le tuera mid-commit
            // (entre put_snapshot et append), créant la fenêtre adversariale.
            let mut seq = 0u64;
            loop {
                if seq >= commits_max as u64 { seq = 0; } // recommencer en boucle

                // ── écriture du commit (pas sous mutex) ──────────────────────
                let header = SnapshotHeader {
                    data_hash: data_h,
                    parent: parent_snap,
                    seq,
                    ts_us: seq * 1_000 + agent_idx as u64 * 1_000_000,
                };
                let snap_id = match store.put_snapshot(header) {
                    Ok(id) => id,
                    Err(e) => { eprintln!("[thread-{}] put_snapshot seq={seq}: {e}", agent_idx); break; }
                };

                let parent_ids = prev_action.map(|id| vec![id]).unwrap_or_default();
                let entry = LogEntry {
                    agent_id,
                    ts_ms: seq + agent_idx as u64 * 1_000_000,
                    parent_ids,
                    hash_before,
                    hash_after: data_h,
                    emit_payload: None,
                };
                let action_id = match log.append(&entry) {
                    Ok(id) => id,
                    Err(e) => { eprintln!("[thread-{}] append seq={seq}: {e}", agent_idx); break; }
                };

                // ── enregistrement de l'ack (sous mutex ~1 µs) ──────────────
                {
                    let mut guard = acked_vec.lock().unwrap();
                    // seq est relatif à la chaîne depuis le début (total des commits de ce thread)
                    let absolute_seq = guard.len() as u64;
                    guard.push(IcsrCommit {
                        seq: absolute_seq,
                        action_id_hex:   hex_encode(&action_id),
                        snapshot_id_hex: hex_encode(&snap_id),
                        data_hash_hex:   hex_encode(&data_h),
                    });
                }

                total_acked.fetch_add(1, Ordering::AcqRel);

                parent_snap  = Some(snap_id);
                prev_action  = Some(action_id);
                hash_before  = data_h;
                seq += 1;
            }
        });
        handles.push(handle);
    }

    // Attendre que le seuil soit atteint (polling léger).
    loop {
        if total_acked.load(Ordering::Relaxed) >= args.kill_threshold { break; }
        std::thread::yield_now();
    }

    // PAS de sleep : les threads continuent d'écrire sans s'arrêter.
    // process::exit(1) ci-dessous les tue mid-commit (entre put_snapshot et append)
    // pour certains d'entre eux — c'est la fenêtre adversariale.

    // Collecter les témoins par agent.
    let mut agents = Vec::with_capacity(args.n_agents);
    for (agent_idx, acked_arc) in per_agent.iter().enumerate() {
        let guard = acked_arc.lock().unwrap();
        let agent_id = agent_id_from_index(agent_idx);
        agents.push(AgentWitness {
            agent_id_hex:   hex_encode(&agent_id),
            acked_commits:  guard.clone(),
        });
    }

    let total = agents.iter().map(|a| a.acked_commits.len()).sum::<usize>();
    eprintln!("[s15-writer] témoin : {} acks total ({} agents)", total, args.n_agents);

    let witness = ConcurrentWitness {
        n_agents:       args.n_agents,
        kill_threshold: args.kill_threshold,
        agents,
    };

    witness.save(&args.witness).unwrap_or_else(|e| {
        eprintln!("witness.save: {e}"); std::process::exit(2);
    });
    eprintln!("[s15-writer] témoin sauvegardé : {}", args.witness.display());

    // SIGKILL simulé.
    use std::io::Write;
    let _ = std::io::stderr().flush();
    std::process::exit(1);
}
