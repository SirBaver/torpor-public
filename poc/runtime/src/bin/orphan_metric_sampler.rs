// orphan-metric-sampler — mesure de croissance des blocs orphelins du ContentStore (ADR-0055 §D4).
//
// Échantillonne `rocksdb.estimate-num-keys` sur les CFs `blocks` et `headers` à 1 Hz.
// Sortie CSV : timestamp_us, blocks_count, headers_count, delta
//
// Déclencheur GC armé si (ADR-0055 §D4) :
//   1. delta > max(1024, 0.02 × headers_count)     [condition statique]
//   2. pente OLS de delta sur fenêtre 10 min > 0   [condition dynamique — voir analyze.py]
//
// USAGE :
//   orphan-metric-sampler --db-store <PATH> [--duration-s <N>] [--out <PATH>]
//
// EXIT CODES :
//   0 — run normal (durée atteinte ou SIGINT)
//   1 — erreur arguments / ouverture store

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use os_poc_store::ContentStore;

struct Args {
    db_store:   PathBuf,
    duration_s: Option<u64>,
    out:        Option<PathBuf>,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "orphan-metric-sampler — mesure croissance blocs orphelins (ADR-0055 §D4)\n\
\n\
USAGE:\n\
    orphan-metric-sampler --db-store <PATH> [--duration-s <N>] [--out <PATH>]\n\
\n\
OPTIONS:\n\
    --db-store <PATH>    Chemin vers le ContentStore RocksDB\n\
    --duration-s <N>     Durée de sampling en secondes (défaut : illimité)\n\
    --out <PATH>         Fichier de sortie CSV (défaut : stdout)\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut db_store = None;
    let mut duration_s = None;
    let mut out = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db-store"   => db_store   = args.next().map(PathBuf::from),
            "--duration-s" => duration_s = args.next().and_then(|s| s.parse().ok()),
            "--out"        => out        = args.next().map(PathBuf::from),
            "--help" | "-h" => print_usage_and_exit(0),
            other => {
                eprintln!("argument inconnu : {other}");
                print_usage_and_exit(1);
            }
        }
    }

    Args {
        db_store: db_store.unwrap_or_else(|| { eprintln!("--db-store requis"); print_usage_and_exit(1); }),
        duration_s,
        out,
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn main() {
    let args = parse_args();

    let store = Arc::new(
        ContentStore::open(&args.db_store, None).unwrap_or_else(|e| {
            eprintln!("impossible d'ouvrir le store : {e}");
            std::process::exit(1);
        }),
    );

    let mut writer: Box<dyn Write> = match &args.out {
        Some(path) => Box::new(BufWriter::new(
            File::create(path).unwrap_or_else(|e| {
                eprintln!("impossible de créer {} : {e}", path.display());
                std::process::exit(1);
            }),
        )),
        None => Box::new(BufWriter::new(std::io::stdout())),
    };

    writeln!(writer, "timestamp_us,blocks_count,headers_count,delta").unwrap();

    let deadline = args.duration_s.map(|d| std::time::Instant::now() + Duration::from_secs(d));

    loop {
        if let Some(dl) = deadline {
            if std::time::Instant::now() >= dl {
                break;
            }
        }

        let blocks  = store.get_rocksdb_int_property("blocks",  "rocksdb.estimate-num-keys").unwrap_or(0);
        let headers = store.get_rocksdb_int_property("headers", "rocksdb.estimate-num-keys").unwrap_or(0);
        let delta   = blocks.saturating_sub(headers);
        let ts      = now_us();

        writeln!(writer, "{ts},{blocks},{headers},{delta}").unwrap();
        writer.flush().unwrap();

        std::thread::sleep(Duration::from_secs(1));
    }
}
