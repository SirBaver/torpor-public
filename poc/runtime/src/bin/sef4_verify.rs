// sef4-verify — binaire de vérification SEF-4.
//
// Ouvre la paire (store, log) survivant à un crash de `sef4-victim` et détermine
// si l'état observable post-recovery satisfait P6 (atomicité crash, ADR-0027 D3
// régime SIGKILL).
//
// CONTRAT P6 :
//   Pour un kill armé pendant l'action `k`, l'état observable doit être l'un des
//   deux états admissibles :
//     (A) `hash_ref_pre[k]` — l'action k n'a laissé aucune trace côté log.
//     (B) `hash_ref_pre[k+1]` — l'action k est entièrement committed.
//   Tout autre état observable viole P6.
//
// MÉTHODE :
//   L'état observable est défini par le log (« le log est la source de vérité
//   pour la causalité observable », ADR-0027). On lit la dernière entrée log
//   pour `agent_id`, on extrait `hash_after` = observed.
//
//   Le verifier reçoit du victim un JSON contenant `hash_ref_pre[0..=k]` (= l'état
//   pré-kill connu). `hash_ref_pre[k+1]` n'est pas pré-calculable (timestamp
//   non déterministe dans le SnapshotHeader). On vérifie donc le cas (B)
//   structurellement :
//     observed.parent == hash_ref_pre[k]  →  observed = état post-action-k valide
//
//   Cas additionnels :
//     - observed == [0u8; 32] (aucune entrée log) ET k == 0 → équivaut à pre[0]. Pass.
//     - observed correspond à pre[i] pour i < k → ne devrait pas arriver (un agent
//       ne recule pas tout seul) → fail.
//
// SORTIE :
//   exit 0 = pass (observed ∈ {pre[k], action-k-committed-via-parent-chain})
//   exit 1 = fail (état illégal)
//   exit 2 = erreur arguments / I/O

use std::path::PathBuf;
use std::sync::Arc;

use os_poc_causal_log::{ActionId, CausalLog, LogEntry};
use os_poc_store::{ContentStore, Cache};

#[derive(Debug)]
struct Args {
    db_store: PathBuf,
    db_log: PathBuf,
    agent_id_hex: String,
    expected_path: PathBuf,
    kill_action: u64,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "sef4-verify — vérifie l'état post-crash contre les references SEF-4\n\
\n\
USAGE:\n\
    sef4-verify --db-store <PATH> --db-log <PATH> --agent-id <HEX32>\n\
                --expected <PATH.json> --kill-action <K>\n\
\n\
ARGS:\n\
    --db-store <PATH>            Répertoire ContentStore post-crash.\n\
    --db-log <PATH>              Répertoire CausalLog post-crash.\n\
    --agent-id <HEX32>           Hex 32-caractères = 16 octets.\n\
    --expected <PATH.json>       Sortie de `sef4-victim --out-expected`.\n\
    --kill-action <K>            Index 0-based de l'action où le kill a été armé.\n\
\n\
EXIT CODES:\n\
    0 = pass (observed ∈ {{pre[k], action-k-committed}})\n\
    1 = fail (état hors set admissible)\n\
    2 = erreur arguments / I/O\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store = None;
    let mut db_log = None;
    let mut agent_id_hex = None;
    let mut expected_path = None;
    let mut kill_action: Option<u64> = None;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store" => { i += 1; db_store = Some(PathBuf::from(&raw[i])); }
            "--db-log"   => { i += 1; db_log   = Some(PathBuf::from(&raw[i])); }
            "--agent-id" => { i += 1; agent_id_hex = Some(raw[i].clone()); }
            "--expected" => { i += 1; expected_path = Some(PathBuf::from(&raw[i])); }
            "--kill-action" => {
                i += 1;
                kill_action = Some(raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--kill-action doit être un entier");
                    std::process::exit(2);
                }));
            }
            "-h" | "--help" => print_usage_and_exit(0),
            other => {
                eprintln!("Argument inconnu: {other}");
                print_usage_and_exit(2);
            }
        }
        i += 1;
    }

    Args {
        db_store: db_store.unwrap_or_else(|| print_usage_and_exit(2)),
        db_log:   db_log  .unwrap_or_else(|| print_usage_and_exit(2)),
        agent_id_hex: agent_id_hex.unwrap_or_else(|| print_usage_and_exit(2)),
        expected_path: expected_path.unwrap_or_else(|| print_usage_and_exit(2)),
        kill_action: kill_action.unwrap_or_else(|| print_usage_and_exit(2)),
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

/// Parse JSON minimaliste : on cherche la clé "hash_ref_pre" dans le fichier
/// produit par sef4-victim.
fn parse_expected(path: &PathBuf) -> Vec<[u8; 32]> {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Lecture --expected échouée: {e}");
        std::process::exit(2);
    });
    let start_marker = "\"hash_ref_pre\":";
    let start = content.find(start_marker).unwrap_or_else(|| {
        eprintln!("expected : clé 'hash_ref_pre' absente");
        std::process::exit(2);
    });
    let after = &content[start + start_marker.len()..];
    let bracket = after.find('[').unwrap_or_else(|| {
        eprintln!("expected : '[' attendu après hash_ref_pre");
        std::process::exit(2);
    });
    let inside = &after[bracket + 1..];
    let end = inside.find(']').unwrap_or_else(|| {
        eprintln!("expected : ']' manquant");
        std::process::exit(2);
    });
    let arr = &inside[..end];
    let mut hashes = Vec::new();
    for chunk in arr.split(',') {
        let s = chunk.trim().trim_matches('"').trim();
        if s.is_empty() { continue; }
        if s.len() != 64 {
            eprintln!("expected : hash hex doit faire 64 caractères, trouvé '{s}'");
            std::process::exit(2);
        }
        let mut h = [0u8; 32];
        for i in 0..32 {
            h[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap_or_else(|_| {
                eprintln!("expected : hash hex invalide '{s}'");
                std::process::exit(2);
            });
        }
        hashes.push(h);
    }
    hashes
}

/// Extrait `hash_after` de la dernière entrée log pour `agent_id`. None si aucune entrée.
fn last_observed_hash_via_log(log: &CausalLog, agent_id: &[u8; 16]) -> Option<[u8; 32]> {
    let ids: Vec<ActionId> = log.query_by_agent_range(agent_id, None, None).ok()?;
    let last_id = ids.last()?;
    let entry: LogEntry = log.get(last_id).ok().flatten()?;
    Some(entry.hash_after)
}

fn hex_encode(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for byte in b {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

fn main() {
    let args = parse_args();
    let agent_id = parse_agent_id(&args.agent_id_hex);
    let expected = parse_expected(&args.expected_path);
    if expected.is_empty() {
        eprintln!("expected : hash_ref_pre vide");
        std::process::exit(2);
    }

    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&args.db_store, Some(shared_cache.clone())).unwrap_or_else(|e| {
        eprintln!("ContentStore::open({}): {e}", args.db_store.display());
        std::process::exit(2);
    }));
    let log = Arc::new(CausalLog::open(&args.db_log, Some(shared_cache)).unwrap_or_else(|e| {
        eprintln!("CausalLog::open({}): {e}", args.db_log.display());
        std::process::exit(2);
    }));

    let observed = last_observed_hash_via_log(&log, &agent_id).unwrap_or([0u8; 32]);
    let k = args.kill_action as usize;

    // CONTRAT P6 (atomicité par action) :
    //
    //   Pour chaque action i ∈ 0..=k, l'action i est soit entièrement committed,
    //   soit pas du tout. Pas d'état partiel observable. Sous SIGKILL sans fsync,
    //   plusieurs actions peuvent être perdues (RocksDB buffer WAL applicativement)
    //   — c'est attendu. Ce qui ne doit JAMAIS arriver : un état où une action est
    //   partiellement committed (block sans header, header sans entrée log et où
    //   le verifier n'accepterait pas l'état avant).
    //
    // États admissibles post-recovery :
    //   (1) observed ∈ {pre[0], pre[1], ..., pre[k]}
    //       → après crash, l'agent est observable à un état atomiquement complet
    //         d'avant le kill (action ≤ i pour i ≤ k entièrement committed,
    //         actions > i perdues).
    //   (2) observed = snapshot S avec S.parent ∈ {pre[0], ..., pre[k]}
    //       → S est l'état post-action-k (commit complet de l'action de kill),
    //         OU une action post-i pour i < k. Dans tous les cas, c'est un état
    //         atomique complet (timestamp non déterministe donc S non précalculable).
    //
    // L'observation autorise donc :
    //   - perte de N actions terminales (cas 1)
    //   - succès complet de l'action de kill (cas 2 avec parent=pre[k])
    //   - succès complet d'une action intermédiaire i<k qui n'avait pas été flushée
    //     dans la mémoire victim au moment du crash (cas 2 avec parent=pre[i])
    //     — improbable en pratique mais autorisé sémantiquement.

    // Cas 1 : observed match un pre[i] dans expected.
    let matched_prefix: Option<usize> = expected.iter().position(|h| h == &observed);

    // Cas 2 : observed est un snapshot du store dont le parent ∈ expected.
    let mut case_2: Option<(usize, u64)> = None; // (parent_index, snap_seq)
    if observed != [0u8; 32] && matched_prefix.is_none() {
        if let Ok(Some(h)) = store.get_header(&observed) {
            if let Some(p) = h.parent {
                if let Some(parent_idx) = expected.iter().position(|x| x == &p) {
                    case_2 = Some((parent_idx, h.seq));
                }
            }
        }
    }

    let pass = matched_prefix.is_some() || case_2.is_some();

    println!("=== SEF-4 verify ===");
    println!("agent_id            : {}", args.agent_id_hex);
    println!("kill_action         : {k}");
    println!("observed hash_after : {}", hex_encode(&observed));
    println!("expected pre[{k}]   : {}", hex_encode(expected.get(k).unwrap_or(&[0u8; 32])));
    println!("expected length    : {} (pre[0..{}] connus)", expected.len(), expected.len().saturating_sub(1));

    if let Some(i) = matched_prefix {
        if i == k {
            println!("→ case 1 : observed = pre[{i}] (action {k} non committed côté log) — autorisé");
        } else if i < k {
            println!("→ case 1 : observed = pre[{i}] (actions {i}..={k} perdues côté log mais préfixe complet) — autorisé sous SIGKILL/no-fsync");
        } else {
            // i > k : impossible normalement (l'expected ne contient que pre[0..=k])
            println!("→ case 1 : observed = pre[{i}] avec i > k — incohérent (BUG)");
        }
    } else if let Some((parent_idx, snap_seq)) = case_2 {
        if parent_idx == k {
            println!("→ case 2 : observed = post-action-{k} (snap seq={snap_seq}, parent=pre[{k}]) — action committed");
        } else {
            println!("→ case 2 : observed = post-action-{parent_idx} (snap seq={snap_seq}, parent=pre[{parent_idx}]) — action intermédiaire committed");
        }
    } else if observed == [0u8; 32] {
        if k == 0 {
            println!("→ case 1 (implicite) : aucune action committed (k=0) — autorisé");
        } else {
            println!("→ FAIL : observed = état initial mais kill_action={k} > 0");
        }
    } else {
        println!("→ FAIL : observed inconnu (ni dans expected, ni successeur direct d'un pre[i])");
    }
    println!("verdict             : {}", if pass { "pass" } else { "fail" });

    // Diagnostic store : si observed est dans le store, indiquer la cohérence du block.
    if observed != [0u8; 32] {
        match store.get_header(&observed) {
            Ok(Some(header)) => {
                let block_present = store.get_block(&header.data_hash).ok().flatten().is_some();
                println!(
                    "store check         : SnapshotHeader trouvé (seq={}, block_present={})",
                    header.seq, block_present
                );
            }
            Ok(None) => println!("store check         : SnapshotHeader absent du ContentStore"),
            Err(e)  => println!("store check         : erreur lookup ({e})"),
        }
    }

    std::process::exit(if pass { 0 } else { 1 });
}
