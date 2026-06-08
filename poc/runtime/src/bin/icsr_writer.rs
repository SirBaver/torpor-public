// icsr-writer — phase d'écriture du harness de durabilité I-CSR (spec/10 §4, ADR-0051 §Amendement).
//
// Écrit N commits synthétiques (store + log, sans acteur WASM) puis induit une
// coupure paramétrée. Sauvegarde le témoin JSON AVANT la coupure.
//
// MODES DE COUPURE (--cut-mode) :
//   drop   — drop propre des Arcs RocksDB (régime arrêt coopératif).
//   exit   — process::exit(1) après flush du témoin (régime SIGKILL, ADR-0027 §D1).
//   [stub] drop_caches — non implémenté ; déclencheur = accès root/VM (spec/10 §6).
//   [stub] kill_qemu   — non implémenté ; déclencheur = hardware réel (spec/10 §6).
//
// EXIT CODES :
//   0 — écriture et témoin OK
//   1 — erreur fatale
//   (process::exit(1) pour le mode exit — pas de code propre)

use std::path::PathBuf;
use std::sync::Arc;

use os_poc_causal_log::CausalLog;
use os_poc_runtime::durability::write_commits;
use os_poc_store::{Cache, ContentStore};

#[derive(Debug)]
struct Args {
    db_store:   PathBuf,
    db_log:     PathBuf,
    witness:    PathBuf,
    n_commits:  usize,
    block_size: usize,
    agent_id:   [u8; 16],
    cut_mode:   String,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "icsr-writer — phase d'écriture harness I-CSR\n\
\n\
USAGE:\n\
    icsr-writer --db-store <PATH> --db-log <PATH> --witness <PATH>\n\
                --agent-id <HEX32> [--n-commits <N>] [--block-size <B>]\n\
                [--cut-mode drop|exit]\n\
\n\
ARGS:\n\
    --db-store  <PATH>   Répertoire ContentStore.\n\
    --db-log    <PATH>   Répertoire CausalLog.\n\
    --witness   <PATH>   Fichier témoin JSON (sortie).\n\
    --agent-id  <HEX32>  Identifiant agent 16 octets en hex.\n\
    --n-commits <N>      Nombre de commits (défaut 50).\n\
    --block-size <B>     Taille du bloc de données en octets (défaut 64).\n\
    --cut-mode  <MODE>   drop (défaut) | exit\n\
\n\
EXIT CODES:\n\
    0 = écriture + témoin OK\n\
    1 = erreur\n"
    );
    std::process::exit(code);
}

fn parse_hex16(s: &str) -> [u8; 16] {
    if s.len() != 32 {
        eprintln!("--agent-id : attendu 32 caractères hex");
        std::process::exit(1);
    }
    let mut out = [0u8; 16];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let p = std::str::from_utf8(pair).expect("ASCII");
        out[i] = u8::from_str_radix(p, 16).unwrap_or_else(|_| {
            eprintln!("--agent-id : caractère non-hex");
            std::process::exit(1);
        });
    }
    out
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store   = None::<PathBuf>;
    let mut db_log     = None::<PathBuf>;
    let mut witness    = None::<PathBuf>;
    let mut agent_id_s = None::<String>;
    let mut n_commits  = 50usize;
    let mut block_size = 64usize;
    let mut cut_mode   = "drop".to_string();

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store"   => { i += 1; db_store   = Some(PathBuf::from(&raw[i])); }
            "--db-log"     => { i += 1; db_log     = Some(PathBuf::from(&raw[i])); }
            "--witness"    => { i += 1; witness    = Some(PathBuf::from(&raw[i])); }
            "--agent-id"   => { i += 1; agent_id_s = Some(raw[i].clone()); }
            "--n-commits"  => {
                i += 1;
                n_commits = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-commits : entier attendu"); std::process::exit(1);
                });
            }
            "--block-size" => {
                i += 1;
                block_size = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--block-size : entier attendu"); std::process::exit(1);
                });
            }
            "--cut-mode"   => { i += 1; cut_mode = raw[i].clone(); }
            "-h" | "--help" => print_usage_and_exit(0),
            other => {
                eprintln!("Argument inconnu: {other}");
                print_usage_and_exit(1);
            }
        }
        i += 1;
    }

    Args {
        db_store:   db_store.unwrap_or_else(|| print_usage_and_exit(1)),
        db_log:     db_log.unwrap_or_else(||   print_usage_and_exit(1)),
        witness:    witness.unwrap_or_else(||   print_usage_and_exit(1)),
        agent_id:   parse_hex16(&agent_id_s.unwrap_or_else(|| print_usage_and_exit(1))),
        n_commits,
        block_size,
        cut_mode,
    }
}

fn main() {
    let args = parse_args();

    match args.cut_mode.as_str() {
        "drop" | "exit" => {}
        "drop_caches" | "kill_qemu" => {
            eprintln!(
                "[icsr-writer] mode '{}' non implémenté (stub) — déclencheur : accès root/VM ou hardware réel (spec/10 §6)",
                args.cut_mode
            );
            std::process::exit(1);
        }
        other => {
            eprintln!("[icsr-writer] --cut-mode inconnu : '{other}'. Valeurs : drop, exit");
            std::process::exit(1);
        }
    }

    std::fs::create_dir_all(&args.db_store).unwrap_or_else(|e| {
        eprintln!("mkdir db-store: {e}"); std::process::exit(1);
    });
    std::fs::create_dir_all(&args.db_log).unwrap_or_else(|e| {
        eprintln!("mkdir db-log: {e}"); std::process::exit(1);
    });

    let shared_cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(&args.db_store, Some(shared_cache.clone())).unwrap_or_else(|e| {
            eprintln!("ContentStore::open: {e}"); std::process::exit(1);
        }),
    );
    let log = Arc::new(
        CausalLog::open(&args.db_log, Some(shared_cache)).unwrap_or_else(|e| {
            eprintln!("CausalLog::open: {e}"); std::process::exit(1);
        }),
    );

    eprintln!(
        "[icsr-writer] écriture de {} commits (block_size={} B, cut_mode={})...",
        args.n_commits, args.block_size, args.cut_mode
    );

    let mut witness = write_commits(
        &store, &log, args.agent_id, args.n_commits, args.block_size,
    ).unwrap_or_else(|e| {
        eprintln!("write_commits: {e}"); std::process::exit(1);
    });
    witness.cut_mode = args.cut_mode.clone();

    // Sauvegarder le témoin AVANT la coupure.
    witness.save(&args.witness).unwrap_or_else(|e| {
        eprintln!("witness.save: {e}"); std::process::exit(1);
    });
    eprintln!(
        "[icsr-writer] témoin sauvegardé : {} ({} commits)",
        args.witness.display(), witness.n_commits
    );

    match args.cut_mode.as_str() {
        "drop" => {
            // Drop propre : les Arcs tombent en fin de scope → RocksDB flush + close.
            drop(store);
            drop(log);
            eprintln!("[icsr-writer] coupure = drop (arrêt coopératif)");
        }
        "exit" => {
            // SIGKILL simulé : pas de destructeurs → buffers WAL potentiellement perdus.
            // Le témoin est déjà sur disque.
            eprintln!("[icsr-writer] coupure = process::exit(1) (régime SIGKILL, ADR-0027 §D1)");
            // Flush stdout/stderr avant exit brutal.
            use std::io::Write;
            let _ = std::io::stderr().flush();
            std::process::exit(1);
        }
        _ => unreachable!(),
    }
}
