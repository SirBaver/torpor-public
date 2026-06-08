// Benchmark H-rollback-latence : valide que le rollback reste ≤ 100ms (p95) sur W2.
//
// Deux mesures complémentaires :
//   1. report_percentiles() : N_MEASURES rollbacks manuels → p50/p95/p99 en µs, verdict pass/fail.
//      Exécuté avant le warmup criterion pour des latences non biaisées.
//   2. criterion bench_function : warmup automatique + IC sur W1 et W2, profondeurs 1/10/100/1000.
//
// Paramètres W2 (spec) : profondeur 100 sur blocs 500 KB.
// Critère H-rollback-latence : p95 ≤ 100ms sur W2 profondeur 100.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use os_poc_store::ContentStore;
use std::time::Instant;
use tempfile::TempDir;

const CHAIN_LEN: u64 = 1001; // seq 0..=1000 ; depth max = 1000
const TIP_SEQ: u64 = CHAIN_LEN - 1; // 1000

/// Mesure p50/p95/p99 sur N_MEASURES rollbacks à une profondeur donnée.
/// Retourne (p50_us, p95_us, p99_us).
fn percentiles(store: &ContentStore, tip: &[u8; 32], depth: u64, n: usize) -> (u64, u64, u64) {
    let target_seq = TIP_SEQ - depth;
    let mut timings_us: Vec<u64> = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let _ = store
            .rollback_path(tip, target_seq)
            .expect("rollback doit réussir");
        timings_us.push(t0.elapsed().as_micros() as u64);
    }
    timings_us.sort_unstable();
    let p = |pm: usize| timings_us[(n * pm / 100).min(n - 1)];
    (p(50), p(95), p(99))
}

fn report_percentiles(store_w1: &ContentStore, tip_w1: &[u8; 32],
                      store_w2: &ContentStore, tip_w2: &[u8; 32]) {
    const N: usize = 1_000;

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║  H-rollback-latence — percentiles ({} mesures/cas)          ║", N);
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  {:>8}  {:>8}  {:>10}  {:>10}  {:>10}  {:>7}", "workload", "depth", "p50 µs", "p95 µs", "p99 µs", "p95≤100ms");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");

    for depth in [1u64, 10, 100, 1000] {
        for (label, store, tip) in [
            ("W1 50KB", store_w1, tip_w1),
            ("W2 500KB", store_w2, tip_w2),
        ] {
            let (p50, p95, p99) = percentiles(store, tip, depth, N);
            let pass = p95 <= 100_000;
            eprintln!("║  {:>8}  {:>8}  {:>10}  {:>10}  {:>10}  {:>7}",
                label, depth, p50, p95, p99,
                if pass { "✓" } else { "✗" });
        }
    }

    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    // Verdict sur le cas de référence spec : W2, depth=100
    let (_, p95_ref, _) = percentiles(store_w2, tip_w2, 100, N);
    let pass = p95_ref <= 100_000;
    eprintln!("║  H-rollback-latence (W2 depth=100) : {}",
        if pass { "✓  CONFORME  (p95 ≤ 100 ms)" }
        else     { "✗  DÉGRADÉE  (p95 > 100 ms)" });
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!();
}

fn bench_rollback(c: &mut Criterion) {
    let dir_w1 = TempDir::new().unwrap();
    let store_w1 = ContentStore::open(dir_w1.path(), None).unwrap();
    let tip_w1 = store_w1.build_chain(CHAIN_LEN, 50 * 1024).unwrap();

    let dir_w2 = TempDir::new().unwrap();
    let store_w2 = ContentStore::open(dir_w2.path(), None).unwrap();
    let tip_w2 = store_w2.build_chain(CHAIN_LEN, 500 * 1024).unwrap();

    report_percentiles(&store_w1, &tip_w1, &store_w2, &tip_w2);

    let mut group = c.benchmark_group("rollback_latency");
    group.sample_size(50);

    for depth in [1u64, 10, 100, 1000] {
        let target_seq = TIP_SEQ - depth;

        group.bench_with_input(
            BenchmarkId::new("W1_50KB", depth),
            &depth,
            |b, _| {
                b.iter(|| {
                    store_w1
                        .rollback_path(&tip_w1, target_seq)
                        .expect("rollback doit réussir")
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("W2_500KB", depth),
            &depth,
            |b, _| {
                b.iter(|| {
                    store_w2
                        .rollback_path(&tip_w2, target_seq)
                        .expect("rollback doit réussir")
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_rollback);
criterion_main!(benches);
