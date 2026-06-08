// Benchmark H-revoke — deux mesures :
//
//   1. check() hot path : p50/p95/p99 sur 10 000 lookups dans un store de ~11 111 caps.
//      Cible : p99 ≤ 1 µs (un seul HashMap lookup).
//
//   2. revoke() sur arbre entier : temps médian pour N ∈ {~1K, ~10K, ~100K}.
//      Cible : N=100K < 1 ms.
//
// Structure de l'arbre synthétique : branching=10
//   depth=3 →  1 111 caps   (~1K)
//   depth=4 → 11 111 caps   (~10K)
//   depth=5 → 111 111 caps  (~100K)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use os_poc_capabilities::{CapabilityStore, Permissions};
use std::time::Instant;

const BRANCHING: usize = 10;

fn fresh_store(depth: u32) -> (CapabilityStore, u64, Vec<u64>) {
    let mut store = CapabilityStore::new();
    let (root, samples) = store.populate_tree(depth, BRANCHING);
    (store, root, samples)
}

fn read_perm() -> Permissions {
    Permissions { read: true, write: false, execute: false, delegate: false }
}

// ─── Percentiles manuels ────────────────────────────────────────────────────

fn report_check_percentiles(store: &CapabilityStore, samples: &[u64]) {
    const N: usize = 10_000;
    let owner = [0xAAu8; 16];
    let perm = read_perm();

    let mut timings_ns: Vec<u64> = Vec::with_capacity(N);
    for i in 0..N {
        let cap_id = samples[i % samples.len()];
        let t0 = Instant::now();
        black_box(store.check(&owner, cap_id, "/res", &perm));
        timings_ns.push(t0.elapsed().as_nanos() as u64);
    }
    timings_ns.sort_unstable();

    let p = |pm: usize| timings_ns[(N * pm / 100).min(N - 1)];
    let p99 = p(99);
    let pass = p99 <= 1_000; // ≤ 1 µs = 1 000 ns

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║  H-revoke — check() hot path ({} mesures, store={} caps)   ║", N, store.count());
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  p50:   {:>7} ns", p(50));
    eprintln!("║  p95:   {:>7} ns", p(95));
    eprintln!("║  p99:   {:>7} ns  ← cible : ≤ 1 000 ns (1 µs)", p99);
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  check hot path : {}",
        if pass { "✓  CONFORME  (p99 ≤ 1 µs)" }
        else     { "✗  DÉGRADÉ   (p99 > 1 µs)" });
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!();
}

fn report_revoke_times() {
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║  H-revoke — revoke() arbre entier                           ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  {:>8}  {:>10}  {:>12}  {:>8}", "N caps", "médiane µs", "p95 µs", "< 1 ms ?");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");

    let configs: &[(u32, usize)] = &[(3, 200), (4, 50), (5, 10)];

    let mut verdict_100k = false;

    for &(depth, k) in configs {
        let mut timings_us: Vec<u64> = Vec::with_capacity(k);
        let mut n_caps = 0usize;

        for _ in 0..k {
            let (mut store, root, _) = fresh_store(depth);
            n_caps = store.count();
            let t0 = Instant::now();
            black_box(store.revoke(root));
            timings_us.push(t0.elapsed().as_micros() as u64);
        }
        timings_us.sort_unstable();

        let median = timings_us[timings_us.len() / 2];
        let p95 = timings_us[(timings_us.len() * 95 / 100).min(timings_us.len() - 1)];
        let pass = median <= 1_000; // ≤ 1 000 µs = 1 ms

        if depth == 5 { verdict_100k = pass; }

        eprintln!("║  {:>8}  {:>10}  {:>12}  {:>8}",
            n_caps, median, p95, if pass { "✓" } else { "✗" });
    }

    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  H-revoke (N=100K) : {}",
        if verdict_100k { "✓  CONFORME  (médiane < 1 ms)" }
        else            { "✗  DÉGRADÉ   (médiane ≥ 1 ms)" });
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!();
}

// ─── Criterion ──────────────────────────────────────────────────────────────

fn bench_capabilities(c: &mut Criterion) {
    // Rapport percentiles avant le warmup criterion
    let (store_10k, _, samples_10k) = fresh_store(4);
    report_check_percentiles(&store_10k, &samples_10k);
    report_revoke_times();

    // Criterion : check hot path
    let owner = [0xAAu8; 16];
    let perm = read_perm();
    let (store_check, _, samples_check) = fresh_store(4);
    let mut i = 0usize;
    c.bench_function("check_hotpath_N11k", |b| {
        b.iter(|| {
            let cap_id = samples_check[i % samples_check.len()];
            i += 1;
            black_box(store_check.check(&owner, cap_id, "/res", &perm))
        })
    });

    // Criterion : revoke arbre entier, tailles croissantes
    let mut group = c.benchmark_group("revoke_tree");
    group.sample_size(20);

    for depth in [3u32, 4, 5] {
        let n_approx = match depth { 3 => "1K", 4 => "10K", _ => "100K" };
        group.bench_with_input(
            BenchmarkId::new("N", n_approx),
            &depth,
            |b, &d| {
                b.iter_batched(
                    || fresh_store(d),
                    |(mut store, root, _)| black_box(store.revoke(root)),
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_capabilities);
criterion_main!(benches);
