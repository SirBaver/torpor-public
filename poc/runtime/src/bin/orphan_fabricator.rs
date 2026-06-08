// orphan-fabricator — crée un ContentStore avec K orphelins + M commits normaux (ADR-0055 §D4).
//
// Valide empiriquement que `estimate-num-keys` mesure correctement Δ = blocks − headers.
// Sur store frais (pas de tombstones, pas de compaction L0), Δ doit être exact.
//
// USAGE :
//   orphan-fabricator --db-store <PATH> --orphans <K> --live <M>
//
// SORTIE :
//   Δ attendu, Δ mesuré, verdict PASS / FAIL

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use os_poc_store::{ContentStore, SnapshotHeader};

struct Args {
    db_store: PathBuf,
    orphans:  u64,
    live:     u64,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "orphan-fabricator — fabrique un ContentStore avec orphelins contrôlés (ADR-0055 §D4)\n\
\n\
USAGE:\n\
    orphan-fabricator --db-store <PATH> --orphans <K> --live <M>\n\
\n\
OPTIONS:\n\
    --db-store <PATH>   Chemin vers le ContentStore à créer (ne doit pas exister)\n\
    --orphans  <K>      Nombre de blocs orphelins (put_block sans put_snapshot)\n\
    --live     <M>      Nombre de commits normaux (put_block + put_snapshot)\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut db_store = None;
    let mut orphans  = None;
    let mut live     = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db-store" => db_store = args.next().map(PathBuf::from),
            "--orphans"  => orphans  = args.next().and_then(|s| s.parse().ok()),
            "--live"     => live     = args.next().and_then(|s| s.parse().ok()),
            "--help" | "-h" => print_usage_and_exit(0),
            other => { eprintln!("argument inconnu : {other}"); print_usage_and_exit(1); }
        }
    }

    Args {
        db_store: db_store.unwrap_or_else(|| { eprintln!("--db-store requis"); print_usage_and_exit(1); }),
        orphans:  orphans.unwrap_or_else(||  { eprintln!("--orphans requis");  print_usage_and_exit(1); }),
        live:     live.unwrap_or_else(||     { eprintln!("--live requis");     print_usage_and_exit(1); }),
    }
}

fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64
}

fn main() {
    let args = parse_args();

    if args.db_store.exists() {
        eprintln!("ERREUR : {} existe déjà — supprimer avant de relancer.", args.db_store.display());
        std::process::exit(1);
    }

    let store = ContentStore::open(&args.db_store, None).unwrap_or_else(|e| {
        eprintln!("impossible de créer le store : {e}");
        std::process::exit(1);
    });

    // Phase 1 : K blocs orphelins — put_block sans put_snapshot (simule crash entre les deux).
    for i in 0..args.orphans {
        let data = format!("orphan-block-{i}");
        store.put_block(data.as_bytes()).unwrap_or_else(|e| {
            eprintln!("put_block orphelin {i} échoué : {e}");
            std::process::exit(1);
        });
    }

    // Phase 2 : M commits normaux — put_block + put_snapshot (chaîne linéaire).
    let mut parent = None;
    for seq in 0..args.live {
        let data = format!("live-block-{seq}");
        let data_hash = store.put_block(data.as_bytes()).unwrap_or_else(|e| {
            eprintln!("put_block live {seq} échoué : {e}");
            std::process::exit(1);
        });
        let header = SnapshotHeader { data_hash, parent, seq, ts_us: now_us() };
        let snap_id = store.put_snapshot(header).unwrap_or_else(|e| {
            eprintln!("put_snapshot live {seq} échoué : {e}");
            std::process::exit(1);
        });
        parent = Some(snap_id);
    }

    // Lecture métrique — flush implicite avant lecture (RocksDB écrit en mémoire d'abord).
    // Pour forcer les estimations à compter les données en mémoire, on lit sans flush explicite ;
    // estimate-num-keys inclut les memtables en cours.
    let blocks  = store.get_rocksdb_int_property("blocks",  "rocksdb.estimate-num-keys").unwrap_or(0);
    let headers = store.get_rocksdb_int_property("headers", "rocksdb.estimate-num-keys").unwrap_or(0);
    let delta_mesure  = blocks.saturating_sub(headers);
    let delta_attendu = args.orphans;

    println!("=== Résultat fabrication ===");
    println!("  Orphelins créés     : {}", args.orphans);
    println!("  Commits normaux     : {}", args.live);
    println!("  blocks (estimate)   : {blocks}");
    println!("  headers (estimate)  : {headers}");
    println!("  Δ attendu           : {delta_attendu}");
    println!("  Δ mesuré            : {delta_mesure}");
    println!();

    // Tolérance ±5% ou ±2 (bruit estimate-num-keys sur memtable froide).
    let tolerance = ((delta_attendu as f64 * 0.05) as u64).max(2);
    let ecart = delta_mesure.abs_diff(delta_attendu);

    if ecart <= tolerance {
        println!("PASS — écart {ecart} ≤ tolérance {tolerance}");
        println!("estimate-num-keys mesure correctement Δ sur store frais.");
    } else {
        println!("FAIL — écart {ecart} > tolérance {tolerance}");
        println!("estimate-num-keys ne reflète pas Δ réel sur ce store.");
        std::process::exit(1);
    }
}
