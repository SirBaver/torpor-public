// Benchmark T5-bis — P3b end-to-end : append (WAL fsync) → get(action_id).
//
// Différences avec T5 (P3a, causal_lookup.rs) :
//   - T5 mesure `get(action_id)` seul, sur DB statique préchargée. C'est P3a.
//   - T5-bis mesure le cycle complet qu'un agent en production paie : `append_durable()`
//     (WAL fsync forcé) puis `get(action_id)` immédiat sur la même action. C'est P3b.
//
// Pourquoi `append_durable` et pas `append` :
//   La spec P3b (spec/02-properties.md) impose explicitement « WAL fsync ».
//   `append()` standard ne fsync pas — le WAL est écrit sur le fd OS mais le sync est
//   différé (typiquement par `bytes_per_sync` ou compaction). Sans fsync forcé, la
//   mesure ne couvre pas le coût durable. `append_durable()` (set_sync=true) est le
//   contrat correct pour P3b.
//
// Pourquoi `get` immédiat sur la même action_id :
//   La spec P3b dit « la latence depuis le moment de l'appel append à la réponse get ».
//   Sémantiquement : ce qu'un agent paie avant de pouvoir relire une action qu'il vient
//   d'émettre. Le `get` sur la même action_id est en cache (memtable) — ce qui isole
//   le coût fsync du coût lookup. Le `get` sur une action ancienne aléatoire serait
//   un mix P3a+P3b qui ne discriminerait plus la borne 20 ms.
//
// Profil de mesure :
//   1. Population : `populate_synthetic(N)` pour atteindre le régime LSM stable
//      (plusieurs niveaux, compaction terminée). N=10⁸ équivalent T5 P3a.
//   2. Mesure : 10 000 cycles { entry distincte → append_durable → get }, latence
//      chronométrée pour le cycle entier. Chaque cycle crée une nouvelle entrée
//      (agent_id, ts_ms distincts) pour ne pas hit l'idempotence content-addressed.
//
// Usage :
//   cargo bench --bench causal_end_to_end                    # N=10⁶, cycle dev rapide
//   BENCH_N=100000000 cargo bench --bench causal_end_to_end  # N=10⁸, qualification P3b
//
// Variables d'environnement :
//   BENCH_N   : nombre d'entrées préchargées dans la DB (défaut 1_000_000).
//   BENCH_DIR : répertoire de la DB. Si défini, utilisé tel quel (créé si absent) ;
//               la DB n'est PAS supprimée à la fin. Si non défini, fallback TempDir.
//
// Sortie machine-readable (stdout) :
//   Une ligne préfixée `T5BIS_METRICS:` suivie d'un objet JSON contenant
//   `p50_us`, `p95_us`, `p99_us`, `p99_9_us`, `pass` (true si p99 ≤ 20_000 µs),
//   `n_entries`, `n_measures`, `bench_dir`, `run_started_unix_ms`, `run_ended_unix_ms`.
//
// Note : 10⁸ entrées sans compression ≈ 10–15 GB. Prévoir NVMe ≥ 1 GB/s et 16 GB RAM.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use os_poc_causal_log::{ActionId, CausalLog, LogEntry};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

const P99_TARGET_US: u64 = 20_000; // P3b borne 20 ms

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
            let sub = p.join(format!("t5bis-causal-{}", unix_ms()));
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

/// Mesure 10 000 cycles { append_durable → get }, retourne les percentiles.
///
/// Chaque cycle crée une entrée distincte (`agent_id` unique par cycle pour ne pas
/// collisionner avec le payload synthétique de populate_synthetic). Le timer démarre
/// avant l'appel `append_durable` et s'arrête après la réponse `get`.
fn report_percentiles(
    log: &CausalLog,
    n_entries: u64,
    db_label: &str,
    run_started_unix_ms: u128,
) {
    const N_MEASURES: usize = 10_000;

    // Préfixe d'agent_id réservé à T5-bis pour ne pas collisionner avec les agents de
    // populate_synthetic (0xAA). On varie les 14 derniers octets pour avoir 2^112 valeurs.
    let mut timings_us: Vec<u64> = Vec::with_capacity(N_MEASURES);

    for j in 0..N_MEASURES {
        let mut agent_id = [0u8; 16];
        agent_id[0] = 0xBB;
        agent_id[1] = 0xBB;
        // Encoder j sur les 8 derniers octets : suffisant pour 2^64 cycles distincts.
        agent_id[8..].copy_from_slice(&(j as u64).to_be_bytes());

        let entry = LogEntry {
            agent_id,
            ts_ms: n_entries + j as u64, // décale au-delà des ts_ms de populate_synthetic
            parent_ids: vec![],
            hash_before: [0xCCu8; 32],
            hash_after: [0xDDu8; 32],
            emit_payload: None,
        };

        let t0 = Instant::now();
        let id: ActionId = log
            .append_durable(&entry)
            .expect("append_durable doit réussir");
        let fetched = log.get(&id).expect("get doit réussir");
        let elapsed = t0.elapsed();
        black_box(fetched);

        timings_us.push(elapsed.as_micros() as u64);
    }
    timings_us.sort_unstable();

    let p = |per_mille: usize| {
        timings_us[(N_MEASURES * per_mille / 1000).min(N_MEASURES - 1)]
    };

    let p50_us = p(500);
    let p95_us = p(950);
    let p99_us = p(990);
    let p99_9_us = p(999);
    let pass = p99_us <= P99_TARGET_US;
    let run_ended_unix_ms = unix_ms();

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════╗");
    eprintln!("║  T5-bis — Latences append_durable+get  ({} cycles, N_pré = {})", N_MEASURES, n_entries);
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  p50:   {:>7} µs  ({:7.3} ms)", p50_us, p50_us as f64 / 1000.0);
    eprintln!("║  p95:   {:>7} µs  ({:7.3} ms)", p95_us, p95_us as f64 / 1000.0);
    eprintln!("║  p99:   {:>7} µs  ({:7.3} ms)  ← cible P3b : ≤ 20.000 ms", p99_us, p99_us as f64 / 1000.0);
    eprintln!("║  p99.9: {:>7} µs  ({:7.3} ms)", p99_9_us, p99_9_us as f64 / 1000.0);
    eprintln!("╠══════════════════════════════════════════════════════════╣");
    eprintln!("║  P3b end-to-end : {}",
        if pass { "✓  CONFORME  (p99 ≤ 20 ms)" }
        else     { "✗  DÉGRADÉE  (p99 > 20 ms — borne à amender ou hardware sous-dimensionné)" }
    );
    eprintln!("╚══════════════════════════════════════════════════════════╝");
    eprintln!();

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
        "T5BIS_METRICS: {{\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"p99_9_us\":{},\"pass\":{},\"n_entries\":{},\"n_measures\":{},\"bench_dir\":\"{}\",\"run_started_unix_ms\":{},\"run_ended_unix_ms\":{},\"p99_target_us\":{},\"target_property\":\"P3b\"}}",
        p50_us, p95_us, p99_us, p99_9_us, pass, n_entries, N_MEASURES,
        db_label_escaped, run_started_unix_ms, run_ended_unix_ms, P99_TARGET_US
    );
}

fn bench_end_to_end(c: &mut Criterion) {
    let n = get_n();
    let run_started_unix_ms = unix_ms();

    eprintln!("\nPopulation de {} entrées synthétiques (régime LSM stable)...", n);
    let home = resolve_db_home();
    let log = CausalLog::open(home.path(), None).unwrap();
    // sample_size=0 : on n'a pas besoin d'un échantillon, T5-bis génère ses propres entrées.
    let _ = log
        .populate_synthetic(n, 0)
        .expect("population doit réussir");
    eprintln!("Population terminée. Démarrage du cycle append_durable + get.");

    // Mesure principale avant warmup criterion (latences réelles, pas biaisées).
    report_percentiles(&log, n, &home.label(), run_started_unix_ms);

    // Bench criterion auxiliaire : moyenne + IC. Chaque itération est un cycle complet.
    let mut i = 0u64;
    c.bench_function(&format!("causal_end_to_end_Npre{}", n), |b| {
        b.iter(|| {
            let mut agent_id = [0u8; 16];
            agent_id[0] = 0xCC; // distinct du préfixe utilisé dans report_percentiles
            agent_id[8..].copy_from_slice(&i.to_be_bytes());
            let entry = LogEntry {
                agent_id,
                ts_ms: n + 10_000_000 + i, // au-delà des ts_ms de report_percentiles
                parent_ids: vec![],
                hash_before: [0xEEu8; 32],
                hash_after: [0xFFu8; 32],
                emit_payload: None,
            };
            i += 1;
            let id = log.append_durable(&entry).expect("append_durable");
            black_box(log.get(&id).expect("get"))
        })
    });

    drop(home);
}

criterion_group!(benches, bench_end_to_end);
criterion_main!(benches);
