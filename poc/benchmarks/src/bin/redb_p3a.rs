// Benchmark P3a — redb v4 (B+tree ACID) sur Linux
//
// Protocole (decisions/b3-storage-research.md §Benchmark P3a) :
//   Population : 10⁸ entrées (clé u64, valeur 100 B)
//   Mesure     : K=3 passes de 10 000 get(action_id) aléatoires
//   Critère    : p99 ≤ 10 ms (≡ SEF-5 RocksDB)
//   Référence  : p99 SEF-5 RocksDB = 1 368 / 1 727 / 1 850 µs
//
// Usage :
//   cargo run -p os-poc-benchmarks --bin redb_p3a --release -- [DB_PATH]
//
// DB_PATH par défaut : results/redb-p3a/db (réutilisée si déjà populée).
// Résultats JSON → results/redb-p3a/verdict.json

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rand::Rng;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("causal_log");
const N: u64 = 100_000_000;
const BATCH_SIZE: u64 = 100_000;
const N_READS: usize = 10_000;
const K: usize = 3;
const VALUE_LEN: usize = 100;
const P99_LIMIT_US: u64 = 10_000;

fn main() {
    let db_path = std::env::args().nth(1).unwrap_or_else(|| {
        std::fs::create_dir_all("results/redb-p3a").ok();
        "results/redb-p3a/db".to_string()
    });
    let results_dir = Path::new(&db_path).parent().unwrap_or(Path::new("."));

    println!("=== Benchmark P3a — redb ===");
    println!("DB path : {}", db_path);
    println!("N       : {}", N);
    println!("K       : {}", K);
    println!("N_reads : {}", N_READS);

    let db = Database::create(&db_path).expect("Database::create");

    // Compter les entrées existantes
    let existing = count_entries(&db);
    println!("Entrées existantes : {}", existing);

    if existing < N {
        println!("\n[population] {} entrées à insérer...", N - existing);
        populate(&db, existing);
    } else {
        println!("\n[population] DB déjà complète, skip.");
    }

    // K=3 passes de mesure
    println!("\n[mesure] K={} passes × {} get() aléatoires", K, N_READS);
    let mut pass_results = Vec::with_capacity(K);
    for k in 0..K {
        let stats = measure_pass(&db, k);
        println!(
            "  Pass {} : p50={:.0} µs  p95={:.0} µs  p99={:.0} µs  p99.9={:.0} µs",
            k + 1, stats.p50_us, stats.p95_us, stats.p99_us, stats.p999_us
        );
        pass_results.push(stats);
    }

    // Verdict
    let worst_p99 = pass_results.iter().map(|s| s.p99_us).fold(0.0_f64, f64::max);
    let verdict = if worst_p99 <= P99_LIMIT_US as f64 { "PASS" } else { "FAIL" };
    println!("\n=== Résultat P3a ===");
    println!("  p99 pire cas    : {:.0} µs", worst_p99);
    println!("  Cible p99       : {} µs", P99_LIMIT_US);
    println!("  Référence p99 RocksDB SEF-5 : 1 368 / 1 727 / 1 850 µs");
    println!("  VERDICT         : {}", verdict);

    // JSON structuré
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let json = build_json(&pass_results, worst_p99, verdict, ts, &db_path);
    let out_path = results_dir.join("verdict.json");
    std::fs::write(&out_path, &json).expect("write verdict.json");
    println!("\nRésultats JSON → {}", out_path.display());
}

// ── Population ─────────────────────────────────────────────────────────────

fn count_entries(db: &Database) -> u64 {
    let rtx = db.begin_read().expect("begin_read");
    match rtx.open_table(TABLE) {
        Ok(t) => t.len().expect("table.len"),
        Err(_) => 0,
    }
}

fn populate(db: &Database, start: u64) {
    let value = vec![0x42u8; VALUE_LEN];
    let n_batches = (N - start + BATCH_SIZE - 1) / BATCH_SIZE;
    let t0 = Instant::now();

    for batch in 0..n_batches {
        let batch_start = start + batch * BATCH_SIZE;
        let batch_end = (batch_start + BATCH_SIZE).min(N);

        let wtx = db.begin_write().expect("begin_write");
        {
            let mut table = wtx.open_table(TABLE).expect("open_table");
            for i in batch_start..batch_end {
                table.insert(i, value.as_slice()).expect("insert");
            }
        }
        wtx.commit().expect("commit");

        if (batch + 1) % 100 == 0 || batch + 1 == n_batches {
            let pct = (batch + 1) as f64 / n_batches as f64 * 100.0;
            let elapsed = t0.elapsed().as_secs_f64();
            let rate = (batch + 1) as f64 * BATCH_SIZE as f64 / elapsed;
            let eta = (n_batches - batch - 1) as f64 / ((batch + 1) as f64 / elapsed);
            println!(
                "  [{:6.1}%] batch {}/{} | {:.0} inserts/s | ETA {:.0} s",
                pct, batch + 1, n_batches, rate, eta
            );
        }
    }

    println!(
        "  Population terminée en {:.1} s",
        t0.elapsed().as_secs_f64()
    );
}

// ── Mesure ──────────────────────────────────────────────────────────────────

struct PassStats {
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    p999_us: f64,
}

fn measure_pass(db: &Database, pass: usize) -> PassStats {
    let mut rng = rand::thread_rng();
    let mut latencies: Vec<u64> = Vec::with_capacity(N_READS);

    let rtx = db.begin_read().expect("begin_read");
    let table = rtx.open_table(TABLE).expect("open_table");

    // Warmup : 100 lectures aléatoires non mesurées
    for _ in 0..100 {
        let key: u64 = rng.gen_range(0..N);
        let _ = table.get(key).expect("get");
    }

    for _ in 0..N_READS {
        let key: u64 = rng.gen_range(0..N);
        let t0 = Instant::now();
        let _ = table.get(key).expect("get");
        latencies.push(t0.elapsed().as_micros() as u64);
    }

    let _ = pass; // utilisé pour logs en dehors
    percentiles(&latencies)
}

fn percentiles(lat: &[u64]) -> PassStats {
    let mut sorted = lat.to_vec();
    sorted.sort_unstable();
    let p = |pct: f64| -> f64 {
        let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx] as f64
    };
    PassStats {
        p50_us: p(50.0),
        p95_us: p(95.0),
        p99_us: p(99.0),
        p999_us: p(99.9),
    }
}

// ── JSON ────────────────────────────────────────────────────────────────────

fn build_json(passes: &[PassStats], worst_p99: f64, verdict: &str, ts: u64, db_path: &str) -> String {
    let passes_json: Vec<String> = passes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                r#"    {{"pass": {}, "p50_us": {:.0}, "p95_us": {:.0}, "p99_us": {:.0}, "p999_us": {:.0}}}"#,
                i + 1, s.p50_us, s.p95_us, s.p99_us, s.p999_us
            )
        })
        .collect();

    format!(
        r#"{{
  "benchmark": "redb-p3a",
  "timestamp": {},
  "db_path": "{}",
  "n_entries": {},
  "k_passes": {},
  "n_reads_per_pass": {},
  "passes": [
{}
  ],
  "worst_p99_us": {:.0},
  "limit_p99_us": {},
  "rocksdb_ref_p99_us": [1368, 1727, 1850],
  "verdict": "{}"
}}
"#,
        ts,
        db_path,
        N,
        K,
        N_READS,
        passes_json.join(",\n"),
        worst_p99,
        P99_LIMIT_US,
        verdict,
    )
}
