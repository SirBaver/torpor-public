// Benchmark T5 — H-causal-latence : valide p99 ≤ 10ms sur 10⁸ entrées.
//
// Deux mesures complémentaires :
//   1. report_percentiles() : 10 000 lookups manuels → p50/p95/p99/p99.9 en µs.
//      Donne les percentiles réels, exécuté AVANT le warmup criterion.
//   2. criterion bench_function : N itérations avec warmup → moyenne + intervalle de confiance.
//
// Usage :
//   cargo bench                                    # N=10⁶, cycle dev rapide
//   BENCH_N=100000000 cargo bench --bench causal_lookup   # N=10⁸, hardware de référence (~20 GB)
//
// Variables d'environnement :
//   BENCH_N    : nombre d'entrées à populer (défaut 1_000_000).
//   BENCH_DIR  : répertoire de la DB RocksDB. Si défini, utilisé tel quel (créé si absent) ;
//                la DB n'est PAS supprimée à la fin (utile pour bench sur NVMe local AWS,
//                où le filesystem est éphémère à l'instance). Si non défini, fallback sur
//                un TempDir système (supprimé à la fin).
//
// Sortie machine-readable (stdout) :
//   La fonction report_percentiles() émet une ligne préfixée `T5_METRICS:` suivie d'un
//   objet JSON contenant `p50_us`, `p95_us`, `p99_us`, `p99_9_us`, `pass`, `n_entries`,
//   `n_measures`, `bench_dir`, `run_started_unix_ms`, `run_ended_unix_ms`. Cette ligne
//   est destinée à être parsée par un script externe (benchmarks/t5-bundle/run.sh) qui
//   assemble les manifests JSON du protocole §4.
//
// Note : 10⁸ entrées sans compression ≈ 10–15 GB. Prévoir NVMe ≥ 1 GB/s et 16 GB RAM.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use os_poc_causal_log::{ActionId, CausalLog};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

fn get_n() -> u64 {
    std::env::var("BENCH_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Résout le répertoire de la DB selon `BENCH_DIR`.
///
/// Retourne (path, _guard). Le `_guard` doit être conservé pendant toute la durée du
/// bench : s'il s'agit d'un `TempDir`, son destructor supprime le répertoire.
enum DbHome {
    Persistent(PathBuf),
    Ephemeral(TempDir),
}

impl DbHome {
    fn path(&self) -> &std::path::Path {
        match self {
            DbHome::Persistent(p) => p.as_path(),
            DbHome::Ephemeral(d) => d.path(),
        }
    }

    fn label(&self) -> String {
        self.path().display().to_string()
    }
}

fn resolve_db_home() -> DbHome {
    match std::env::var("BENCH_DIR") {
        Ok(s) if !s.is_empty() => {
            let p = PathBuf::from(&s);
            std::fs::create_dir_all(&p)
                .unwrap_or_else(|e| panic!("BENCH_DIR={} : create_dir_all a échoué : {}", s, e));
            // Sous-répertoire dédié au bench pour ne pas marcher sur d'éventuels fichiers tiers.
            let sub = p.join(format!("t5-causal-{}", unix_ms()));
            std::fs::create_dir_all(&sub)
                .unwrap_or_else(|e| panic!("création {} a échoué : {}", sub.display(), e));
            eprintln!("BENCH_DIR={} → DB persistante dans {}", s, sub.display());
            DbHome::Persistent(sub)
        }
        _ => {
            let td = TempDir::new().expect("TempDir doit pouvoir être créé");
            eprintln!("BENCH_DIR non défini → DB éphémère dans {}", td.path().display());
            DbHome::Ephemeral(td)
        }
    }
}

/// Mesure les percentiles p50/p95/p99/p99.9 sur `N_MEASURES` lookups aléatoires.
///
/// Appelé une seule fois avant le warmup criterion — les timings ne sont pas
/// comptabilisés dans le benchmark criterion mais représentent des latences réelles
/// (pas de biais de warmup).
fn report_percentiles(
    log: &CausalLog,
    samples: &[ActionId],
    n_entries: u64,
    db_label: &str,
    run_started_unix_ms: u128,
) {
    const N_MEASURES: usize = 10_000;

    let mut timings_us: Vec<u64> = Vec::with_capacity(N_MEASURES);
    for j in 0..N_MEASURES {
        let id = &samples[j % samples.len()];
        let t0 = Instant::now();
        black_box(log.get(id).expect("lookup doit réussir"));
        timings_us.push(t0.elapsed().as_micros() as u64);
    }
    timings_us.sort_unstable();

    // p(‰) avec clamp pour éviter out-of-bounds sur les derniers percentiles
    let p = |per_mille: usize| {
        timings_us[(N_MEASURES * per_mille / 1000).min(N_MEASURES - 1)]
    };

    let p50_us = p(500);
    let p95_us = p(950);
    let p99_us = p(990);
    let p99_9_us = p(999);
    let pass = p99_us <= 10_000;
    let run_ended_unix_ms = unix_ms();

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║  T5 — Latences lookup  ({} mesures, N = {})", N_MEASURES, n_entries);
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  p50:   {:>7} µs  ({:7.3} ms)", p50_us, p50_us as f64 / 1000.0);
    eprintln!("║  p95:   {:>7} µs  ({:7.3} ms)", p95_us, p95_us as f64 / 1000.0);
    eprintln!("║  p99:   {:>7} µs  ({:7.3} ms)  ← cible P3 : ≤ 10.000 ms", p99_us, p99_us as f64 / 1000.0);
    eprintln!("║  p99.9: {:>7} µs  ({:7.3} ms)", p99_9_us, p99_9_us as f64 / 1000.0);
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  H-causal-latence : {}",
        if pass { "✓  CONFORME  (p99 ≤ 10 ms)" }
        else     { "✗  DÉGRADÉE  (p99 > 10 ms)" }
    );
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!();

    // Ligne machine-readable pour le harness shell (benchmarks/t5-bundle/run.sh).
    // Format JSON inline, une seule ligne. Échapper db_label pour éviter de casser le JSON.
    let db_label_escaped: String = db_label
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            c if (c as u32) < 0x20 => vec![],
            c => vec![c],
        })
        .collect();
    println!(
        "T5_METRICS: {{\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"p99_9_us\":{},\"pass\":{},\"n_entries\":{},\"n_measures\":{},\"bench_dir\":\"{}\",\"run_started_unix_ms\":{},\"run_ended_unix_ms\":{}}}",
        p50_us, p95_us, p99_us, p99_9_us, pass, n_entries, N_MEASURES,
        db_label_escaped, run_started_unix_ms, run_ended_unix_ms
    );
}

fn bench_lookup(c: &mut Criterion) {
    let n = get_n();
    let sample_size = 1_000;
    let run_started_unix_ms = unix_ms();

    eprintln!("\nPopulation de {} entrées synthétiques...", n);
    let home = resolve_db_home();
    let log = CausalLog::open(home.path(), None).unwrap();
    let samples = log
        .populate_synthetic(n, sample_size)
        .expect("population doit réussir");
    eprintln!("{} échantillons disponibles pour les lookups.", samples.len());

    // Percentiles réels avant le warmup criterion
    report_percentiles(&log, &samples, n, &home.label(), run_started_unix_ms);

    // Benchmark criterion : moyenne + IC sur N itérations avec warmup automatique
    let mut i = 0usize;
    c.bench_function(&format!("causal_lookup_N{}", n), |b| {
        b.iter(|| {
            let id = &samples[i % samples.len()];
            i += 1;
            black_box(log.get(id).expect("lookup doit réussir"))
        })
    });

    // Note : `home` est volontairement gardé en vie jusqu'ici. Si c'est un `Ephemeral`,
    // son `Drop` supprime le TempDir. Si c'est `Persistent`, le répertoire survit
    // (utile sur NVMe local AWS, où le filesystem disparaît avec l'instance de toute façon).
    drop(home);
}

criterion_group!(benches, bench_lookup);
criterion_main!(benches);
