// sef4-victim — binaire de test SEF-4 (TODO Axe 3 / ADR-0027 régime SIGKILL).
//
// Rôle : exécuter un agent WASM déterministe qui fait N actions (commit_barrier + emit)
// et tuer le processus à un point précis pendant l'action `kill_action`.
//
// DESIGN — pourquoi reference + crash partagent le même processus :
//
//   Le `SnapshotHeader` contient `ts_us = SystemTime::now()`, donc le `snapshot_id`
//   (= SHA-256 du header) n'est pas reproductible entre runs distincts. Conclusion :
//   on ne peut pas pré-calculer les hash_ref_pre[i] dans un run séparé puis les
//   comparer au run de crash. La référence DOIT être produite dans le même processus
//   que le crash, *avant* le kill.
//
//   Méthode :
//     1. Le binaire exécute les actions 0..kill_action sans kill (mode "warm-up").
//        À chaque étape, il sauvegarde `last_snapshot` dans une liste mémoire.
//     2. Juste avant `process_one(kill_action)`, il écrit la liste sur disque
//        (`--out-expected <path>` — fichier JSON minimal). C'est l'état pré-kill
//        connu, qui contient `hash_ref_pre[kill_action]` (l'état actuel avant
//        l'action qui va crasher).
//     3. Le binaire arme le CrashPoint et appelle `process_one(kill_action)`.
//        Le `fire()` interne tue le processus pendant cette action.
//
//   En mode `--kill-at none`, le binaire termine normalement après les N actions
//   et écrit tous les hash_ref_pre[0..=N] dans le fichier expected.
//
// Sous le régime SIGKILL (ADR-0027 D3) :
//   - Si kill avant log.append → état observable (via log) = hash_ref_pre[kill_action]
//   - Si kill après log.append → état observable = hash_ref_pre[kill_action + 1]
//   Le verifier teste les deux états admissibles.

use std::path::PathBuf;
use std::sync::Arc;

use os_poc_causal_log::CausalLog;
use os_poc_runtime::actor::{ActorInstance, AGENT_WAT};
use os_poc_runtime::crash_point::{armed, CrashPoint};
use os_poc_runtime::make_engine;
use os_poc_store::{ContentStore, Cache};
use wasmtime::Module;

#[derive(Debug)]
struct Args {
    db_store: PathBuf,
    db_log: PathBuf,
    agent_id_hex: String,
    n_actions: u64,
    kill_at: Option<(CrashPoint, u64)>,
    out_expected: PathBuf,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "sef4-victim — SEF-4 commit-barrier crash tester (ADR-0027 SIGKILL régime)\n\
\n\
USAGE:\n\
    sef4-victim --db-store <PATH> --db-log <PATH> --agent-id <HEX32>\n\
                --n-actions <N> --out-expected <PATH>\n\
                [--kill-at <POINT>:<K>]\n\
\n\
ARGS:\n\
    --db-store <PATH>            Répertoire pour ContentStore (créé si absent).\n\
    --db-log <PATH>              Répertoire pour CausalLog (créé si absent).\n\
    --agent-id <HEX32>           Hex 32-caractères = 16 octets.\n\
    --n-actions <N>              Nombre d'actions à exécuter au total.\n\
    --kill-at <POINT>:<K>        Optionnel. POINT ∈ {{pre_put_block,\n\
                                 between_put_block_and_put_snapshot,\n\
                                 post_put_snapshot_pre_log_append,\n\
                                 post_log_append}}, K = index 0-based.\n\
                                 Si omis : exécute les N actions sans kill.\n\
    --out-expected <PATH>        Fichier JSON listant les hash_ref_pre[0..=N]\n\
                                 connus avant le kill. Toujours écrit (même en\n\
                                 mode kill — il sera tronqué à kill_action+1\n\
                                 entrées car le kill empêche d'aller plus loin).\n\
\n\
EXIT CODES:\n\
    1 = kill déclenché (attendu en mode --kill-at)\n\
    0 = exécution normale terminée (mode reference)\n\
    >=2 = erreur d'orchestration\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store: Option<PathBuf> = None;
    let mut db_log: Option<PathBuf> = None;
    let mut agent_id_hex: Option<String> = None;
    let mut n_actions: Option<u64> = None;
    let mut kill_at: Option<(CrashPoint, u64)> = None;
    let mut out_expected: Option<PathBuf> = None;

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
            "--kill-at" => {
                i += 1;
                let v = &raw[i];
                if v == "none" {
                    kill_at = None;
                } else {
                    let (name, k) = v.split_once(':').unwrap_or_else(|| {
                        eprintln!("--kill-at attend <POINT>:<K> (ex: pre_put_block:3)");
                        std::process::exit(2);
                    });
                    let cp = CrashPoint::from_cli(name).unwrap_or_else(|| {
                        eprintln!("--kill-at : point inconnu '{name}'"); std::process::exit(2);
                    });
                    let k_idx: u64 = k.parse().unwrap_or_else(|_| {
                        eprintln!("--kill-at : index K doit être un entier"); std::process::exit(2);
                    });
                    kill_at = Some((cp, k_idx));
                }
            }
            "--out-expected" => { i += 1; out_expected = Some(PathBuf::from(&raw[i])); }
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
        n_actions: n_actions.unwrap_or_else(|| print_usage_and_exit(2)),
        kill_at,
        out_expected: out_expected.unwrap_or_else(|| print_usage_and_exit(2)),
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

fn hex_encode(b: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for byte in b {
        s.push_str(&format!("{:02x}", byte));
    }
    s
}

fn write_expected_json(
    path: &PathBuf,
    agent_id_hex: &str,
    hashes: &[[u8; 32]],
    kill_action: Option<u64>,
) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(&format!("  \"agent_id\": \"{}\",\n", agent_id_hex));
    json.push_str(&format!("  \"actions_seen\": {},\n", hashes.len().saturating_sub(1)));
    match kill_action {
        Some(k) => json.push_str(&format!("  \"kill_action\": {},\n", k)),
        None    => json.push_str("  \"kill_action\": null,\n"),
    }
    json.push_str("  \"hash_ref_pre\": [\n");
    for (i, h) in hashes.iter().enumerate() {
        let comma = if i + 1 < hashes.len() { "," } else { "" };
        json.push_str(&format!("    \"{}\"{}\n", hex_encode(h), comma));
    }
    json.push_str("  ]\n");
    json.push_str("}\n");
    std::fs::write(path, json).expect("write --out-expected");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = parse_args();
    let agent_id = parse_agent_id(&args.agent_id_hex);

    std::fs::create_dir_all(&args.db_store).expect("mkdir db-store");
    std::fs::create_dir_all(&args.db_log).expect("mkdir db-log");
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&args.db_store, Some(shared_cache.clone())).expect("ContentStore::open"));
    let log   = Arc::new(CausalLog::open(&args.db_log, Some(shared_cache)).expect("CausalLog::open"));

    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let mut actor = ActorInstance::new_precompiled(
        &engine, &module, agent_id, store, log,
    ).await.expect("ActorInstance::new_precompiled");

    // hash_ref_pre[i] = last_snapshot après l'action i-1. Convention :
    //   hash_ref_pre[0] = [0u8; 32] (état initial — pas de snapshot encore).
    //   hash_ref_pre[i] = snap_id du i-ème snapshot (1-indexé côté actions).
    let mut hash_ref_pre: Vec<[u8; 32]> = Vec::with_capacity((args.n_actions + 1) as usize);
    hash_ref_pre.push([0u8; 32]);

    for k in 0..args.n_actions {
        let is_kill_target = matches!(args.kill_at, Some((_, kt)) if k == kt);

        if is_kill_target {
            // Avant d'armer le kill, on persiste l'état connu pour le verifier.
            // À ce moment : hash_ref_pre contient [pre[0], pre[1], ..., pre[k]].
            // Le verifier comparera l'état observé post-recovery à pre[k] et pre[k+1]
            // — où pre[k+1] est INCONNU car l'action k va crasher.
            // Le verifier accepte aussi un hash qui ne soit pas dans cette liste si
            // l'asymétrie store/log est telle que log = pre[k]. Détails dans sef4-verify.
            write_expected_json(&args.out_expected, &args.agent_id_hex, &hash_ref_pre, args.kill_at.map(|(_, k)| k));

            // Maintenant on arme. Le `fire()` dans la host fn tuera le processus.
            armed::arm(args.kill_at.unwrap().0);
            eprintln!(
                "[sef4-victim] armed {:?} ; pre-state[{}] = {} ; appel process_one({})",
                args.kill_at.unwrap().0, k, hex_encode(&hash_ref_pre[k as usize]), k
            );
        } else {
            armed::disarm();
        }

        let _ = actor.process_one(b"sef4").await.unwrap_or_else(|e| {
            eprintln!("[sef4-victim] process_one(action={k}) a échoué: {e}");
            // On écrit ce qu'on a avant de mourir.
            write_expected_json(&args.out_expected, &args.agent_id_hex, &hash_ref_pre, args.kill_at.map(|(_, k)| k));
            std::process::exit(3);
        });

        let snap = actor.last_snapshot().unwrap_or([0u8; 32]);
        hash_ref_pre.push(snap);
    }

    // Si on arrive ici, soit on est en mode reference (kill_at=None), soit le kill
    // n'a pas été déclenché (orchestration cassée — kill_action > n_actions).
    write_expected_json(&args.out_expected, &args.agent_id_hex, &hash_ref_pre, args.kill_at.map(|(_, k)| k));

    if args.kill_at.is_some() {
        eprintln!(
            "[sef4-victim] WARNING : exécution terminée sans kill (kill_at={:?}, n_actions={})",
            args.kill_at, args.n_actions
        );
        std::process::exit(4);
    }

    println!("OK reference run terminé ({} actions, last_snapshot = {})",
        args.n_actions, hex_encode(&hash_ref_pre[hash_ref_pre.len() - 1]));
}
