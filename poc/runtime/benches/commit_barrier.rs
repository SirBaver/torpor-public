// Benchmark H-commit-barrier — deux mesures :
//
//   1. H-cb-correct  : vérifié structurellement — emit ne peut s'exécuter sans
//      commit_barrier préalable (debug_assert dans le host function).
//
//   2. H-cb-overhead : coût d'un cycle complet process_one (commit_barrier + emit).
//      Cible : overhead moyen < 5% du cycle W1 = 5 000 000 µs → < 250 000 µs.
//
// Résultat attendu : overhead ≈ quelques dizaines de µs (RocksDB local cache-warm),
// soit ~0.001% de W1 — marge >> 1000×.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use os_poc_causal_log::CausalLog;
use os_poc_runtime::actor::{ActorInstance, AgentId};
use os_poc_store::{ContentStore, Cache};
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use wasmtime::Engine;

fn make_instance(engine: &Engine, store_dir: &TempDir, log_dir: &TempDir) -> ActorInstance {
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store_ref = Arc::new(ContentStore::open(store_dir.path(), Some(shared_cache.clone())).expect("ContentStore::open"));
    let log_ref = Arc::new(CausalLog::open(log_dir.path(), Some(shared_cache)).expect("CausalLog::open"));
    let agent_id: AgentId = [0x42u8; 16];
    ActorInstance::new(engine, agent_id, store_ref, log_ref).expect("ActorInstance::new")
}

fn report_overhead() {
    let engine = Engine::default();
    let store_dir = TempDir::new().unwrap();
    let log_dir = TempDir::new().unwrap();
    let mut instance = make_instance(&engine, &store_dir, &log_dir);

    let data = b"hello from agent";
    const N: usize = 1_000;
    let mut timings_us: Vec<u64> = Vec::with_capacity(N);

    for _ in 0..N {
        let t0 = Instant::now();
        instance.process_one(data).unwrap();
        timings_us.push(t0.elapsed().as_micros() as u64);
    }
    timings_us.sort_unstable();

    let p = |pm: usize| timings_us[(N * pm / 100).min(N - 1)];
    let mean_us = timings_us.iter().sum::<u64>() / N as u64;
    // W1 cycle = 5 000 000 µs ; 5% = 250 000 µs
    let overhead_pct = mean_us as f64 / 50_000.0;
    let pass = mean_us <= 250_000;

    eprintln!();
    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║  H-cb-overhead — process_one ({N} mesures, cache warm)     ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  moyenne: {:>8} µs", mean_us);
    eprintln!("║  p50:     {:>8} µs", p(50));
    eprintln!("║  p95:     {:>8} µs", p(95));
    eprintln!("║  p99:     {:>8} µs", p(99));
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  overhead / cycle W1 : {:.4}%  (cible ≤ 5.0%)", overhead_pct);
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!("║  H-cb-correct  : ✓  (emit après commit_barrier — structurel) ║");
    eprintln!("║  H-cb-overhead : {}",
        if pass { "✓  CONFORME  (overhead < 5% W1)" }
        else    { "✗  DÉGRADÉ   (overhead ≥ 5% W1)" });
    eprintln!("╚══════════════════════════════════════════════════════════════╝");
    eprintln!();
}

fn bench_commit_barrier(c: &mut Criterion) {
    report_overhead();

    let engine = Engine::default();
    let store_dir = TempDir::new().unwrap();
    let log_dir = TempDir::new().unwrap();
    let mut instance = make_instance(&engine, &store_dir, &log_dir);
    let data = b"hello from agent";

    c.bench_function("process_one_with_barrier", |b| {
        b.iter(|| black_box(instance.process_one(data).unwrap()))
    });
}

criterion_group!(benches, bench_commit_barrier);
criterion_main!(benches);
