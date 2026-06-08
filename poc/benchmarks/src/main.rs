// Lanceur de benchmarks manuels W1/W2/W3 — complément aux benchmarks Criterion.
// Usage : cargo run -p os-poc-benchmarks -- [w1|w2|w3|capabilities|t6|t6-phase-a [N]|t6-soak [N [H]]|t5-ter [a|b] [N]|t7-wake [N [ND [CAP [K]]]]|t8-sat [NA [ND [CAP [MINS [CMS]]]]]|t5-p3c [N_PREPOP [N_WRITERS [N_READS]]]|compare-sandbox [N]|all]

// jemalloc rend les pages à l'OS après free() — évite la rétention glibc ptmalloc2
// qui fausse rss_adj post-compaction (ADR-0034 D1, TODO P3A).
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use os_poc_capabilities::{CapabilityStore, Permissions};
use os_poc_causal_log::CausalLog;
use os_poc_runtime::actor::{ActorInstance, AgentId, Message, AGENT_WAT, W1_AGENT_WAT};
use os_poc_runtime::inference::PriorityClass;
use os_poc_runtime::io_queue::IoAdmissionQueue;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_store::{ContentStore, Cache};
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use wasmtime::Module;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = args.get(1).map(String::as_str).unwrap_or("all");

    match target {
        "w1"           => bench_w1_density(),
        "w2"           => bench_w2_rollback(),
        "capabilities" => bench_capabilities_revoke(),
        "t6"           => bench_t6_actor_density(),
        "t6-phase-a"   => {
            let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
            bench_t6_phase_a(n);
        }
        "t6-soak"      => {
            let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(500);
            let hours: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4.0);
            bench_t6_soak(n, hours);
        }
        "t5-ter"       => {
            let mode = args.get(2).map(String::as_str).unwrap_or("a");
            let n: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(100_000_000);
            bench_t5_ter(mode, n);
        }
        "t7-wake"      => {
            let n_agents:  usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50);
            let n_dormant: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
            let cap_io:    usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
            let k_runs:    u32   = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(3);
            bench_t7_wake(n_agents, n_dormant, cap_io, k_runs);
        }
        "t8-sat"       => {
            let n_active:   usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50);
            let n_dormant:  usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
            let cap_io:     usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
            let dur_mins:   f64   = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(5.0);
            let cycle_ms:   u64   = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(5_000);
            let prepop_n:   u64   = args.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
            bench_t8_sat(n_active, n_dormant, cap_io, dur_mins, cycle_ms, prepop_n);
        }
        "compare-sandbox" => {
            let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
            bench_compare_sandbox(n);
        }
        "t5-p3c" => {
            let n_prepop:  u64   = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
            let n_writers: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
            let n_reads:   usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(10_000);
            bench_t5_p3c(n_prepop, n_writers, n_reads);
        }
        "all"          => {
            bench_w1_density();
            bench_w2_rollback();
            bench_capabilities_revoke();
            // t6 exclu de "all" : requiert bare metal Linux et plus de RAM
        }
        other => eprintln!("cible inconnue: {other}  (valeurs : w1 w2 capabilities t6 t6-phase-a all)"),
    }
}

/// H-densité : mesure l'overhead mémoire par agent sur un workload W1.
fn bench_w1_density() {
    println!("=== W1 : overhead mémoire par agent ===");

    let dir = TempDir::new().unwrap();
    let store = ContentStore::open(dir.path(), None).unwrap();

    let block_size = 50 * 1024; // 50 KB état agent W1
    let n_agents = 1000;

    let t0 = Instant::now();
    let mut tips = Vec::with_capacity(n_agents);
    for _ in 0..n_agents {
        let tip = store.build_chain(10, block_size).unwrap();
        tips.push(tip);
    }
    let elapsed = t0.elapsed();

    println!(
        "  {} agents, 10 snapshots chacun, bloc {}KB → {:.1}ms total ({:.2}µs/agent)",
        n_agents,
        block_size / 1024,
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1e6 / n_agents as f64,
    );
    println!("  Résultats à comparer avec : Docker overhead 50–200 MB, BEAM ~8KB/processus");
}

/// H-rollback-latence : rollback à différentes profondeurs sur W2.
fn bench_w2_rollback() {
    println!("=== W2 : latence de rollback ===");

    let dir = TempDir::new().unwrap();
    let store = ContentStore::open(dir.path(), None).unwrap();

    let n = 1000u64;
    let block_size = 500 * 1024; // 500 KB état agent W2
    let tip = store.build_chain(n, block_size).unwrap();

    for depth in [1u64, 10, 100, 999] {
        let target_seq = n - 1 - depth;
        let samples = 100;
        let t0 = Instant::now();
        for _ in 0..samples {
            let _ = store.rollback_path(&tip, target_seq).unwrap();
        }
        let elapsed = t0.elapsed();
        let p_mean = elapsed.as_secs_f64() * 1000.0 / samples as f64;
        let criterion = if p_mean <= 100.0 { "✓" } else { "✗ DÉPASSE 100ms" };
        println!(
            "  rollback -{} actions : {:.2}ms moyenne sur {} samples  {}",
            depth, p_mean, samples, criterion
        );
    }
}

/// H-revoke : coût de révocation en arbre de capabilities croissant.
fn bench_capabilities_revoke() {
    println!("=== H-revoke : coût de révocation ===");

    let agent_root = [0x01u8; 16];
    let agent_child = [0x02u8; 16];

    let perm_full = || Permissions {
        read: true,
        write: true,
        execute: true,
        delegate: true,
    };

    for tree_size in [100usize, 1_000, 10_000, 100_000] {
        let mut store = CapabilityStore::new();
        let root = store.grant_root(agent_root, perm_full(), "resource://all".to_string());

        // Construire un arbre linéaire de `tree_size` capabilities.
        let mut current = root;
        for i in 0..tree_size {
            let child = store
                .delegate(current, &agent_root, agent_child, perm_full(), format!("resource://{}", i))
                .unwrap();
            current = child;
        }

        let count_before = store.count();
        let t0 = Instant::now();
        let revoked = store.revoke(root);
        let elapsed = t0.elapsed();

        println!(
            "  arbre {} caps → révocation de {} caps en {:.2}µs  ({})",
            count_before,
            revoked,
            elapsed.as_secs_f64() * 1e6,
            if elapsed.as_secs_f64() * 1e6 < 1000.0 { "✓" } else { "! vérifier overhead CPU" }
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// T6 — H-densité : overhead par acteur Wasmtime/Tokio vs baseline Docker (W1)
// ─────────────────────────────────────────────────────────────────────────────

/// RSS du process courant en KB (Linux /proc/self/status VmRSS).
fn read_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// T6 : mesure l'overhead mémoire et la latence de spawn par ActorInstance.
///
/// Deux modes :
///   AGENT_WAT  — 1 page (64 KB WASM), mesure l'overhead infra pur.
///   W1_AGENT_WAT — 800 pages (50 MB WASM touchés au start), mesure avec état W1 complet.
///
/// Critères H-densité :
///   overhead Wasmtime ≤ 10 MB/acteur (hors état app 50 MB)  → spawn_overhead ≤ 10 240 KB
///   spawn ≤ 2 ms/acteur
///   ratio densité vs Docker ≥ 5× (voir benchmarks/t6-docker-baseline.sh)
fn bench_t6_actor_density() {
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(bench_t6_actor_density_async())
}

async fn bench_t6_actor_density_async() {
    println!("=== T6 : densité d'acteurs Wasmtime/Tokio (H-densité) ===\n");

    // ── 1. Infrastructure partagée ──────────────────────────────────────────
    let rss_0 = read_rss_kb();

    let engine = os_poc_runtime::make_engine();
    let rss_post_engine = read_rss_kb();

    let store_dir = TempDir::new().unwrap();
    let log_dir   = TempDir::new().unwrap();
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store_shared = Arc::new(ContentStore::open(store_dir.path(), Some(shared_cache.clone())).unwrap());
    let log_shared   = Arc::new(CausalLog::open(log_dir.path(), Some(shared_cache)).unwrap());
    let rss_post_infra = read_rss_kb();

    println!("  Infrastructure partagée (coût unique, amorti sur N acteurs) :");
    println!("    Engine Wasmtime  : {:+5} KB", rss_post_engine as i64 - rss_0 as i64);
    println!("    ContentStore+Log : {:+5} KB", rss_post_infra as i64 - rss_post_engine as i64);
    println!("    Total partagé    : {:+5} KB\n", rss_post_infra as i64 - rss_0 as i64);

    // ── 2. Compilation des modules (JIT Cranelift — unique par module) ──────
    let t_jit = Instant::now();
    let module_minimal = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let jit_minimal_us = t_jit.elapsed().as_micros();

    let t_jit = Instant::now();
    let module_w1 = Module::new(&engine, W1_AGENT_WAT).expect("compile W1_AGENT_WAT");
    let jit_w1_us = t_jit.elapsed().as_micros();

    let rss_post_modules = read_rss_kb();
    println!("  Compilation JIT (coût unique partagé entre toutes les instances) :");
    println!("    AGENT_WAT    : {} µs, {:+} KB", jit_minimal_us, rss_post_modules as i64 - rss_post_infra as i64);
    println!("    W1_AGENT_WAT : {} µs", jit_w1_us);
    println!();

    // ── 3. Mode minimal (AGENT_WAT, 1 page = 64 KB WASM) ───────────────────
    println!("  --- Mode minimal (AGENT_WAT, 1 page = 64 KB WASM) ---");
    println!("  {:>4}  {:>9}  {:>14}  {:>12}  {:>10}  {:>10}",
             "N", "RSS (KB)", "overhead/acteur", "spawn/acteur", "RAM ≤10MB?", "spawn ≤2ms?");

    let rss_base = read_rss_kb();
    let mut instances_min: Vec<ActorInstance> = Vec::new();

    for &n_target in &[1usize, 10, 50, 100, 200] {
        let n_before = instances_min.len();
        let t0 = Instant::now();
        for i in n_before..n_target {
            let mut id: AgentId = [0u8; 16];
            id[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let inst = ActorInstance::new_precompiled(
                &engine, &module_minimal, id, store_shared.clone(), log_shared.clone(),
            ).await.expect("new_precompiled minimal");
            instances_min.push(inst);
        }
        let elapsed = t0.elapsed();
        let n_batch = (n_target - n_before).max(1);

        let rss_now      = read_rss_kb();
        let overhead_per = rss_now.saturating_sub(rss_base) as f64 / n_target as f64;
        let spawn_us     = elapsed.as_micros() as f64 / n_batch as f64;

        let ok_ram   = if overhead_per <= 10_240.0 { "✓" } else { "✗" };
        let ok_spawn = if spawn_us     <=  2_000.0 { "✓" } else { "✗" };

        println!("  {:>4}  {:>9}  {:>12.0} KB  {:>9.0} µs  {:>10}  {:>10}",
                 n_target, rss_now, overhead_per, spawn_us, ok_ram, ok_spawn);
    }

    // Density projection
    let overhead_200 = instances_min.last().map(|_| {
        (read_rss_kb().saturating_sub(rss_base) as f64) / 200.0
    }).unwrap_or(0.0);
    drop(instances_min);

    let density_wasmtime = 16_777_216.0 / (51_200.0 + overhead_200); // 16 GB / (50 MB app + infra)
    println!("\n  Densité projetée 16 GB (50 MB app + overhead_minimal)  : {:.0} acteurs", density_wasmtime);
    println!("  Baseline Docker ≈ 50–200 MB overhead/container         : {:.0}–{:.0} containers",
             16_777_216.0_f64 / (51_200.0 + 204_800.0),
             16_777_216.0_f64 / (51_200.0 +  51_200.0));
    println!("  Ratio estimé                                            : {:.1}×–{:.1}×\n",
             density_wasmtime / (16_777_216.0 / (51_200.0 + 204_800.0)),
             density_wasmtime / (16_777_216.0 / (51_200.0 +  51_200.0)));

    // ── 4. Mode W1 (W1_AGENT_WAT, 800 pages = 50 MB WASM touchés) ───────────
    println!("  --- Mode W1 (W1_AGENT_WAT, 800 pages = 50 MB WASM — start fn touche chaque page) ---");
    println!("  (spawn inclut ~800 page faults par acteur)\n");
    println!("  {:>4}  {:>9}  {:>14}  {:>12}", "N", "RSS (KB)", "overhead/acteur", "spawn/acteur");

    let rss_base_w1 = read_rss_kb();
    let mut instances_w1: Vec<ActorInstance> = Vec::new();

    for &n_target in &[1usize, 5, 10, 20] {
        let n_before = instances_w1.len();
        let t0 = Instant::now();
        for i in n_before..n_target {
            let mut id: AgentId = [0u8; 16];
            id[..8].copy_from_slice(&((i + 10_000) as u64).to_le_bytes());
            let inst = ActorInstance::new_precompiled(
                &engine, &module_w1, id, store_shared.clone(), log_shared.clone(),
            ).await.expect("new_precompiled W1");
            instances_w1.push(inst);
        }
        let elapsed = t0.elapsed();
        let n_batch = (n_target - n_before).max(1);

        let rss_now      = read_rss_kb();
        let overhead_per = rss_now.saturating_sub(rss_base_w1) as f64 / n_target as f64;
        let spawn_us     = elapsed.as_micros() as f64 / n_batch as f64;

        // overhead_infra = overhead total - WASM memory (800 pages × 4 KB touched = 3 200 KB dirty)
        let wasm_dirty_kb = 800.0 * 4.0; // 1 dirty 4KB page per 64KB region
        let infra_overhead = (overhead_per - wasm_dirty_kb).max(0.0);

        println!("  {:>4}  {:>9}  {:>12.0} KB  {:>9.0} µs  (dont ~{:.0} KB WASM dirty, ~{:.0} KB infra)",
                 n_target, rss_now, overhead_per, spawn_us, wasm_dirty_kb, infra_overhead);
    }

    drop(instances_w1);

    // ── 5. Verdict ──────────────────────────────────────────────────────────
    println!("\n  Verdict H-densité :");
    println!("    Critère overhead ≤ 10 MB/acteur : voir colonne RAM ci-dessus");
    println!("    Critère spawn ≤ 2 ms/acteur     : voir colonne spawn ci-dessus");
    println!("    Ratio densité ≥ 5×              : projection ci-dessus (Docker baseline requis)");
    println!("\n  Statut : INDICATIF (1 hardware, TempDir, 1 run).");
    println!("  Pour 'partiellement validé' : NVMe ≥ 1 GB/s, K≥3 runs, + t6-docker-baseline.sh.");
}

// ─────────────────────────────────────────────────────────────────────────────
// T6 Phase A — H-densité-hébergée : agents vivants, dormants dans inbox.recv()
//
// Différence vs bench_t6_actor_density :
//   - Les agents sont enregistrés dans le scheduler (run_loop actif, task Tokio vivante).
//   - Le RSS mesuré inclut le coût des tâches Tokio + inbox mpsc, pas seulement l'ActorInstance.
//   - C'est la mesure canonique de P1a (densité hébergée) selon spec/02 §P1a.
//
// Usage : cargo run -p os-poc-benchmarks -- t6-phase-a [N]
// Sortie : JSON dans results/T6/phase-a/<timestamp>/wasmtime_n<N>.json
// ─────────────────────────────────────────────────────────────────────────────

fn bench_t6_phase_a(n: usize) {
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(bench_t6_phase_a_async(n))
}

async fn bench_t6_phase_a_async(n: usize) {
    println!("=== T6 Phase A — H-densité-hébergée : {} agents dormants ===\n", n);

    // ── 1. Infrastructure partagée ──────────────────────────────────────────
    let rss_0 = read_rss_kb();

    // make_engine() : epoch_interruption + async_support (requis pour func_wrap_async)
    let engine = os_poc_runtime::make_engine();
    let rss_post_engine = read_rss_kb();

    let store_dir = TempDir::new().unwrap();
    let log_dir   = TempDir::new().unwrap();
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store_shared = Arc::new(ContentStore::open(store_dir.path(), Some(shared_cache.clone())).unwrap());
    let log_shared   = Arc::new(CausalLog::open(log_dir.path(), Some(shared_cache)).unwrap());
    let rss_post_infra = read_rss_kb();

    println!("  Infrastructure partagée (coût unique, amorti sur N agents) :");
    println!("    Engine Wasmtime  : {:+5} KB", rss_post_engine as i64 - rss_0 as i64);
    println!("    ContentStore+Log : {:+5} KB", rss_post_infra as i64 - rss_post_engine as i64);
    println!("    Total partagé    : {:+5} KB\n", rss_post_infra as i64 - rss_0 as i64);

    // ── 2. Compilation JIT du module (partagé entre toutes les instances via CoW) ──
    let t_jit = Instant::now();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let jit_us = t_jit.elapsed().as_micros();
    let rss_post_module = read_rss_kb();
    println!("  Compilation JIT (CoW partagé) : {} µs, {:+} KB\n",
             jit_us, rss_post_module as i64 - rss_post_infra as i64);

    // ── 3. Spawn N agents dans le scheduler (run_loop vivant, dormant sur inbox) ──
    let mut scheduler = Scheduler::new();
    let rss_pre_spawn = read_rss_kb();
    let t_spawn = Instant::now();

    for i in 0..n {
        let mut id: AgentId = [0u8; 16];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let instance = ActorInstance::new_precompiled(
            &engine, &module, id,
            store_shared.clone(), log_shared.clone(),
        ).await.expect("new_precompiled");
        scheduler.register(instance);
    }

    let spawn_elapsed = t_spawn.elapsed();

    // Stabilisation : les run_loops émettent leur Spawned event (write RocksDB).
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let rss_post_spawn = read_rss_kb();
    let overhead_total_kb = rss_post_spawn.saturating_sub(rss_pre_spawn);
    let overhead_per_kb   = overhead_total_kb as f64 / n as f64;
    let spawn_us_per      = spawn_elapsed.as_micros() as f64 / n as f64;

    println!("  Spawn {} agents (tasks Tokio actives, dormantes sur inbox.recv()) :", n);
    println!("    RSS avant spawn     : {} KB", rss_pre_spawn);
    println!("    RSS après +500ms    : {} KB", rss_post_spawn);
    println!("    Delta total         : {} KB", overhead_total_kb);
    println!("    Overhead/agent      : {:.1} KB", overhead_per_kb);
    println!("    Latence spawn       : {:.0} µs/agent\n", spawn_us_per);

    // ── 4. Densité projetée et verdict H-densité-hébergée ──────────────────
    let ram_16gb_kb: f64 = 16.0 * 1024.0 * 1024.0;
    let density_wasmtime = ram_16gb_kb / overhead_per_kb.max(1.0);

    // Baseline L27/L28 : Python 3.11 + deps LLM, mesure hôte (MemAvailable delta)
    let docker_baseline_kb: f64 = 43_314.0;
    let density_docker = ram_16gb_kb / docker_baseline_kb;
    let ratio = density_wasmtime / density_docker;

    println!("  Densité projetée 16 GB (overhead agent seul, infra partagée) :");
    println!("    Wasmtime     : {:.0} agents", density_wasmtime);
    println!("    Docker (L27) : {:.0} containers  ({:.0} KB/container)", density_docker, docker_baseline_kb);
    println!("    Ratio        : {:.0}×", ratio);
    println!("    Cible P1a    : ≥ 5×");

    let verdict = if ratio >= 5.0 { "PASS" } else { "FAIL" };
    println!("    Verdict      : {}\n", verdict);

    // ── 5. JSON structuré → results/T6/phase-a/<timestamp>/ ────────────────
    let ts_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let out_dir = format!("results/T6/phase-a/{}", ts_secs);
    if std::fs::create_dir_all(&out_dir).is_ok() {
        let json = format!(
            concat!(
                "{{\n",
                "  \"benchmark\": \"t6-phase-a\",\n",
                "  \"n_agents\": {},\n",
                "  \"rss_baseline_kb\": {},\n",
                "  \"rss_post_infra_kb\": {},\n",
                "  \"rss_post_module_kb\": {},\n",
                "  \"rss_pre_spawn_kb\": {},\n",
                "  \"rss_post_spawn_kb\": {},\n",
                "  \"overhead_total_kb\": {},\n",
                "  \"overhead_per_agent_kb\": {:.1},\n",
                "  \"spawn_us_per_agent\": {:.0},\n",
                "  \"density_wasmtime\": {:.0},\n",
                "  \"density_docker_l27\": {:.0},\n",
                "  \"docker_baseline_kb\": {:.0},\n",
                "  \"ratio\": {:.1},\n",
                "  \"target\": \">=5\",\n",
                "  \"verdict\": \"{}\",\n",
                "  \"classification\": \"indicatif\",\n",
                "  \"module\": \"AGENT_WAT\",\n",
                "  \"engine\": \"wasmtime+tokio+epoch\",\n",
                "  \"note\": \"agents dormants dans inbox.recv() — P1a densité hébergée\"\n",
                "}}"
            ),
            n,
            rss_0, rss_post_infra, rss_post_module,
            rss_pre_spawn, rss_post_spawn,
            overhead_total_kb, overhead_per_kb,
            spawn_us_per,
            density_wasmtime, density_docker, docker_baseline_kb,
            ratio, verdict,
        );

        let json_path = format!("{}/wasmtime_n{}.json", out_dir, n);
        std::fs::write(&json_path, &json).ok();
        println!("  JSON écrit : {}", json_path);
        println!("{}", json);
        // Ligne compacte pour parsing shell (grep '^T6_JSON:')
        println!(
            "T6_JSON:{{\"benchmark\":\"t6-phase-a\",\"n_agents\":{},\"overhead_per_agent_kb\":{:.1},\"ratio\":{:.1},\"verdict\":\"{}\"}}",
            n, overhead_per_kb, ratio, verdict
        );
    } else {
        println!("  (répertoire results/ inaccessible — JSON non écrit)");
    }

    // scheduler dropped ici → senders droppés → run_loops reçoivent None et terminent.
    drop(scheduler);
}

// ─────────────────────────────────────────────────────────────────────────────
// T6-soak — H-densité-hébergée : absence de fuite mémoire sous charge soutenue
//
// N agents actifs, message par agent toutes les TICK_MS ms, pendant HOURS heures.
// Mesure RSS toutes les 60 s → results/T6/soak/<timestamp>/rss.jsonl + verdict.json
//
// Critère PASS dérivé de H-profil-B (durée minimum 1h) :
//   pente RSS < N × overhead_per_agent / 60  (KB/min)
//   => au taux PASS, la croissance RSS sur H-profil-B min (1h) ≤ 1× overhead initial.
//
// Test non-linéaire :
//   si slope(2ème moitié) > 2× slope(1ère moitié) → warn compaction ou fragmentation.
//
// Périmètre couvert : Wasmtime Store per-actor, RocksDB write amplification, Tokio heap.
// Hors périmètre : cycle evict/wake (scheduler.dormant) — couvert par S11/S12.
//
// Usage : cargo run -p os-poc-benchmarks --release -- t6-soak [N [HOURS]]
// ─────────────────────────────────────────────────────────────────────────────

fn bench_t6_soak(n: usize, soak_hours: f64) {
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(bench_t6_soak_async(n, soak_hours))
}

async fn bench_t6_soak_async(n: usize, soak_hours: f64) {
    let soak_secs       = (soak_hours * 3600.0) as u64;
    let tick_ms         = 1_000u64;
    let sample_secs     = 60u64;
    let warmup_samples  = 5usize;

    println!("=== T6-soak — absence de fuite mémoire ({} agents, {:.1}h) ===\n", n, soak_hours);

    // ── Infrastructure partagée ─────────────────────────────────────────────
    // IMPORTANT : utilise un chemin disque réel (NVMe) et non TempDir (/tmp → tmpfs RAM).
    // Avec tmpfs, RocksDB écrit en RAM → RSS croît linéairement avec le volume d'écriture,
    // masquant toute analyse de fuite réelle. Sur NVMe, les SST flushés libèrent la RAM.
    let engine   = os_poc_runtime::make_engine();
    let ts_secs  = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let soak_data_dir = format!("results/T6/soak/{}/data", ts_secs);
    let store_path    = std::path::Path::new(&soak_data_dir).join("store");
    let log_path      = std::path::Path::new(&soak_data_dir).join("log");
    std::fs::create_dir_all(&store_path).expect("create store dir");
    std::fs::create_dir_all(&log_path).expect("create log dir");
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store_shared = Arc::new(ContentStore::open(&store_path, Some(shared_cache.clone())).unwrap());
    let log_shared   = Arc::new(CausalLog::open(&log_path, Some(shared_cache)).unwrap());

    let t_jit = Instant::now();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    println!("  JIT: {} µs\n", t_jit.elapsed().as_micros());

    // ── Spawn N agents (exercice register + reap au passage) ────────────────
    let mut scheduler = Scheduler::new();
    let rss_pre_spawn = read_rss_kb();

    let mut senders: Vec<tokio::sync::mpsc::Sender<Message>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut id: AgentId = [0u8; 16];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let instance = ActorInstance::new_precompiled(
            &engine, &module, id, store_shared.clone(), log_shared.clone(),
        ).await.expect("new_precompiled");
        senders.push(scheduler.register(instance));
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let rss_post_spawn    = read_rss_kb();
    let overhead_total_kb = rss_post_spawn.saturating_sub(rss_pre_spawn) as f64;
    let overhead_per_kb   = overhead_total_kb / n as f64;

    // Critère PASS ADR-0033 : OLS sur rss_adj (= RSS − memtable_kb).
    // Seuil : 1 % du overhead total par heure → overhead_total_kb × 0.01 / 60 KB/min.
    // Raisonnement : une fuite applicative de 1 %/h de l'overhead initial est détectable
    // et non attribuable au bruit allocateur. En deçà : PASS.
    let pass_slope_kb_per_min = overhead_total_kb * 0.01 / 60.0;

    println!("  Spawn {} agents → {:.1} KB/agent overhead ({:.0} KB total)", n, overhead_per_kb, overhead_total_kb);
    println!("  Critère PASS ADR-0033 (rss_adj, 1%/h) : pente < {:.2} KB/min", pass_slope_kb_per_min);
    println!("  Soak: {:.1}h, tick: {}ms, sample: {}s, warmup: {} points\n",
             soak_hours, tick_ms, sample_secs, warmup_samples);

    // ── Sortie ──────────────────────────────────────────────────────────────
    let out_dir    = format!("results/T6/soak/{}", ts_secs);
    let jsonl_path = format!("{}/rss.jsonl", out_dir);
    std::fs::create_dir_all(&out_dir).ok();

    // ── Soak loop ────────────────────────────────────────────────────────────
    // samples : (t_min, rss_kb, memtable_kb, block_cache_kb)
    // memtable_kb   = ContentStore + CausalLog memtables.
    // block_cache_kb = ContentStore + CausalLog block caches.
    // rss_adj = rss - memtable_kb - block_cache_kb : croissance applicative hors-LSM.
    let mut samples: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut n_messages_sent: u64 = 0;
    let t0 = Instant::now();
    let mut last_sample_elapsed = u64::MAX; // force premier sample immédiat
    let mut last_tick = Instant::now();

    while t0.elapsed().as_secs() < soak_secs {
        // Tick : un Message::data par agent, try_send pour ne pas bloquer
        if last_tick.elapsed().as_millis() as u64 >= tick_ms {
            for tx in &senders {
                if tx.try_send(Message::data(vec![0x42u8; 16])).is_ok() {
                    n_messages_sent += 1;
                }
            }
            last_tick = Instant::now();
        }

        // Sample RSS + memtable à intervalles réguliers
        let elapsed_s = t0.elapsed().as_secs();
        let slot = elapsed_s / sample_secs;
        if slot != last_sample_elapsed {
            last_sample_elapsed = slot;
            let rss_kb         = read_rss_kb() as f64;
            let memtable_kb    = (store_shared.total_memtable_bytes()
                                  + log_shared.total_memtable_bytes()) as f64 / 1024.0;
            let block_cache_kb = (store_shared.block_cache_usage_bytes()
                                  + log_shared.block_cache_usage_bytes()) as f64 / 1024.0;
            let rss_adj_kb     = (rss_kb - memtable_kb - block_cache_kb).max(0.0);
            let t_min          = elapsed_s as f64 / 60.0;
            samples.push((t_min, rss_kb, memtable_kb, block_cache_kb));

            use std::io::Write as _;
            let entry = format!(
                "{{\"elapsed_s\":{},\"rss_kb\":{:.0},\"memtable_kb\":{:.0},\"block_cache_kb\":{:.0},\"rss_adj_kb\":{:.0},\"n_messages_sent\":{},\"n_agents\":{}}}\n",
                elapsed_s, rss_kb, memtable_kb, block_cache_kb, rss_adj_kb, n_messages_sent, n
            );
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&jsonl_path) {
                let _ = f.write_all(entry.as_bytes());
            }
            println!("  t={:4}s  RSS={:7.0} KB  mem={:6.0} KB  cache={:6.0} KB  rss_adj={:7.0} KB  msgs={}",
                     elapsed_s, rss_kb, memtable_kb, block_cache_kb, rss_adj_kb, n_messages_sent);
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    // ── Analyse statistique (ADR-0033 : OLS sur rss_adj = RSS − memtable) ────
    // rss_adj isole la croissance applicative des write buffers LSM.
    let adj_samples: Vec<(f64, f64)> = samples.iter()
        .skip(warmup_samples)
        .map(|(t, rss, mem, cache)| (*t, (*rss - *mem - *cache).max(0.0)))
        .collect();
    let analysis = adj_samples;
    let verdict = compute_soak_verdict(&analysis, pass_slope_kb_per_min);

    let verdict_json = format!(
        concat!(
            "{{\n",
            "  \"benchmark\": \"t6-soak\",\n",
            "  \"criterion\": \"ADR-0033: OLS sur rss_adj = RSS - memtable_kb\",\n",
            "  \"n_agents\": {},\n",
            "  \"soak_hours\": {:.1},\n",
            "  \"n_messages_sent\": {},\n",
            "  \"rss_post_spawn_kb\": {},\n",
            "  \"overhead_per_agent_kb\": {:.1},\n",
            "  \"pass_slope_kb_per_min\": {:.3},\n",
            "  \"measured_slope_rss_adj_kb_per_min\": {:.3},\n",
            "  \"r2\": {:.3},\n",
            "  \"slope_second_half_ratio\": {:.2},\n",
            "  \"verdict\": \"{}\",\n",
            "  \"notes\": \"{}\"\n",
            "}}"
        ),
        n, soak_hours, n_messages_sent, rss_post_spawn, overhead_per_kb,
        pass_slope_kb_per_min,
        verdict.slope_kb_per_min, verdict.r2, verdict.slope_ratio,
        verdict.label, verdict.notes,
    );
    std::fs::write(format!("{}/verdict.json", out_dir), &verdict_json).ok();

    println!("\n  Pente rss_adj (ADR-0033)   : {:.3} KB/min", verdict.slope_kb_per_min);
    println!("  Seuil PASS                 : {:.3} KB/min", pass_slope_kb_per_min);
    println!("  R²                         : {:.3}", verdict.r2);
    println!("  Ratio pente 2ème/1ère moitié : {:.2}×", verdict.slope_ratio);
    println!("  Verdict                    : {}", verdict.label);
    println!("  Notes                      : {}", verdict.notes);
    println!("\n  Résultats : {}", out_dir);
    println!("{}", verdict_json);

    drop(scheduler); // senders droppés → run_loops terminent
}

struct SoakVerdict {
    slope_kb_per_min: f64,
    r2:               f64,
    slope_ratio:      f64,
    label:            &'static str,
    notes:            String,
}

fn compute_soak_verdict(samples: &[(f64, f64)], pass_slope: f64) -> SoakVerdict {
    if samples.len() < 4 {
        return SoakVerdict {
            slope_kb_per_min: 0.0, r2: 0.0, slope_ratio: 1.0,
            label: "INSUFFICIENT_DATA",
            notes: format!("seulement {} points post-warmup (min 4 requis)", samples.len()),
        };
    }

    let slope = linear_slope(samples);
    let r2    = compute_r2(samples, slope);

    let mid          = samples.len() / 2;
    let slope_first  = linear_slope(&samples[..mid]);
    let slope_second = linear_slope(&samples[mid..]);
    let ratio = if slope_first.abs() > 0.01 { slope_second / slope_first } else { 1.0 };

    let pass         = slope < pass_slope && r2 < 0.5;
    let nonlin_warn  = ratio > 2.0 && slope_second > pass_slope * 0.5;

    let label = if pass { "PASS" } else { "FAIL" };
    let notes = if nonlin_warn {
        format!("warn: croissance non-linéaire suspectée (ratio={:.1}×) — compaction RocksDB ou fragmentation tas", ratio)
    } else if !pass {
        format!("pente {:.1} KB/min > seuil {:.1} KB/min (r2={:.2})", slope, pass_slope, r2)
    } else {
        "stable".to_string()
    };

    SoakVerdict { slope_kb_per_min: slope, r2, slope_ratio: ratio, label, notes }
}

// Régression linéaire OLS : retourne la pente b de y = a + b×x.
fn linear_slope(pts: &[(f64, f64)]) -> f64 {
    if pts.len() < 2 { return 0.0; }
    let n   = pts.len() as f64;
    let sx  = pts.iter().map(|(x, _)| x).sum::<f64>();
    let sy  = pts.iter().map(|(_, y)| y).sum::<f64>();
    let sxy = pts.iter().map(|(x, y)| x * y).sum::<f64>();
    let sx2 = pts.iter().map(|(x, _)| x * x).sum::<f64>();
    let den = n * sx2 - sx * sx;
    if den.abs() < 1e-10 { return 0.0; }
    (n * sxy - sx * sy) / den
}

// R² de la régression linéaire de pente `slope`.
fn compute_r2(pts: &[(f64, f64)], slope: f64) -> f64 {
    if pts.len() < 2 { return 0.0; }
    let n       = pts.len() as f64;
    let x_mean  = pts.iter().map(|(x, _)| x).sum::<f64>() / n;
    let y_mean  = pts.iter().map(|(_, y)| y).sum::<f64>() / n;
    let inter   = y_mean - slope * x_mean;
    let ss_res  = pts.iter().map(|(x, y)| (y - (slope * x + inter)).powi(2)).sum::<f64>();
    let ss_tot  = pts.iter().map(|(_, y)| (y - y_mean).powi(2)).sum::<f64>();
    if ss_tot < 1e-10 { return 0.0; }
    1.0 - ss_res / ss_tot
}

// ── T5-ter — isolation p99 vs compaction RocksDB (ADR-0032 §D4) ──────────────
//
// Mode A : disable_auto_compactions + compact_all avant mesure → P3b-intrinsèque.
// Mode B : config normale + poll num-running-compactions à chaque cycle →
//          corrélation spikes p99 > SPIKE_THRESHOLD_US avec compactions actives.
//
// Usage :
//   cargo run -p os-poc-benchmarks --release -- t5-ter a [N]   # Mode A
//   cargo run -p os-poc-benchmarks --release -- t5-ter b [N]   # Mode B
//   N = nombre d'entrées à précharger (défaut 100_000_000 = 10⁸)
//
// Sortie : results/T5-ter/<timestamp>/{verdict.json, events.jsonl}

const SPIKE_THRESHOLD_US: u64 = 5_000; // 5 ms — seuil spike Mode B
const N_MEASURES: usize        = 10_000;
const P99_TARGET_US: u64       = 20_000; // borne P3b

fn bench_t5_ter(mode: &str, n_entries: u64) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let out_dir = format!("results/T5-ter/{}/{}", mode, ts);
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    println!("=== T5-ter Mode {} — isolation p99 vs compaction RocksDB ===", mode.to_uppercase());
    println!("    N_entries = {}  N_measures = {}  out = {}\n", n_entries, N_MEASURES, out_dir);

    // DB dans results/ sur disque réel (évite tmpfs)
    let db_path = format!("{}/db", out_dir);
    std::fs::create_dir_all(&db_path).expect("create db dir");
    let db_path = std::path::Path::new(&db_path);

    // Ouvrir avec la config adaptée au mode
    let log = match mode {
        "a" => CausalLog::open_no_autocompact(db_path, None).expect("open_no_autocompact"),
        _   => CausalLog::open(db_path, None).expect("open"),
    };

    // Population synthétique (même protocole que T5-bis)
    println!("  Population de {} entrées...", n_entries);
    let t_pop = Instant::now();
    log.populate_synthetic(n_entries, 0).expect("populate_synthetic");
    println!("  Population terminée en {:.1}s", t_pop.elapsed().as_secs_f64());

    // Mode A : compaction manuelle avant mesure → état propre, aucun L0 en attente
    if mode == "a" {
        println!("  Compaction manuelle (Mode A)...");
        let t_compact = Instant::now();
        log.compact_all();
        println!("  Compaction terminée en {:.1}s\n", t_compact.elapsed().as_secs_f64());
    } else {
        // Mode B : attendre que les compactions post-population se stabilisent (~10s)
        println!("  Attente stabilisation post-population (10s)...");
        std::thread::sleep(std::time::Duration::from_secs(10));
        let l0 = log.get_rocksdb_int_property("default", "rocksdb.num-files-at-level0")
            .unwrap_or(0);
        println!("  num-files-at-level0 = {} avant mesure\n", l0);
    }

    // ── Boucle de mesure ──────────────────────────────────────────────────────
    // Chaque cycle : append_durable + get, agent_id préfixe 0xBB unique.
    // Mode B : poll num-running-compactions + num-files-at-level0 + is-write-stalled
    //          après chaque cycle.
    struct CycleRecord {
        cycle_idx:          usize,
        latency_us:         u64,
        // Mode B seulement (0 en Mode A)
        running_compactions: u64,
        files_l0:           u64,
        write_stalled:      u64,
    }

    let mut records: Vec<CycleRecord> = Vec::with_capacity(N_MEASURES);
    let t_measure = Instant::now();

    use std::io::Write as _;
    let events_path = format!("{}/events.jsonl", out_dir);
    let mut events_file = std::fs::OpenOptions::new()
        .create(true).write(true).truncate(true)
        .open(&events_path)
        .expect("open events.jsonl");

    for j in 0..N_MEASURES {
        let mut agent_id = [0u8; 16];
        agent_id[0] = 0xBB;
        agent_id[1] = 0xBB;
        agent_id[8..].copy_from_slice(&(j as u64).to_be_bytes());

        let entry = os_poc_causal_log::LogEntry {
            agent_id,
            ts_ms: n_entries + j as u64,
            parent_ids: vec![],
            hash_before: [0xCCu8; 32],
            hash_after:  [0xDDu8; 32],
            emit_payload: None,
        };

        let t0 = Instant::now();
        let id = log.append_durable(&entry).expect("append_durable");
        let _ = log.get(&id).expect("get");
        let latency_us = t0.elapsed().as_micros() as u64;

        let (running, files_l0, stalled) = if mode == "b" {
            (
                log.get_rocksdb_int_property("default", "rocksdb.num-running-compactions").unwrap_or(0),
                log.get_rocksdb_int_property("default", "rocksdb.num-files-at-level0").unwrap_or(0),
                log.get_rocksdb_int_property("default", "rocksdb.is-write-stalled").unwrap_or(0),
            )
        } else {
            (0, 0, 0)
        };

        // Écrire l'event dans le JSONL (tous les cycles pour Mode B, spikes seulement pour Mode A)
        if mode == "b" || latency_us >= SPIKE_THRESHOLD_US {
            let line = format!(
                "{{\"i\":{},\"us\":{},\"running_compact\":{},\"files_l0\":{},\"stalled\":{}}}\n",
                j, latency_us, running, files_l0, stalled
            );
            let _ = events_file.write_all(line.as_bytes());
        }

        records.push(CycleRecord {
            cycle_idx: j, latency_us,
            running_compactions: running, files_l0, write_stalled: stalled,
        });
    }
    let measure_elapsed = t_measure.elapsed();

    // ── Calcul des percentiles ────────────────────────────────────────────────
    let mut latencies: Vec<u64> = records.iter().map(|r| r.latency_us).collect();
    latencies.sort_unstable();
    let p = |per_mille: usize| latencies[(N_MEASURES * per_mille / 1000).min(N_MEASURES - 1)];
    let p50  = p(500);
    let p95  = p(950);
    let p99  = p(990);
    let p999 = p(999);
    let pass = p99 <= P99_TARGET_US;

    // ── Analyse Mode B : corrélation spikes / compactions ────────────────────
    let spikes: Vec<&CycleRecord> = records.iter()
        .filter(|r| r.latency_us >= SPIKE_THRESHOLD_US)
        .collect();
    let n_spikes = spikes.len();

    // Un spike "cooccure" avec une compaction si running_compactions > 0
    // sur le même cycle OU sur l'un des 5 cycles précédents (fenêtre ~5 ms).
    // On étend la fenêtre en arrière car le stall peut commencer avant le cycle mesuré.
    let spikes_with_compaction = if mode == "b" {
        spikes.iter().filter(|spike| {
            let window_start = spike.cycle_idx.saturating_sub(5);
            records[window_start..=spike.cycle_idx]
                .iter()
                // running_compactions > 0 : compaction active
                // write_stalled > 0 : throttle actif
                // files_l0 >= 10 : L0 élevé (précurseur de stall)
                .any(|r| r.running_compactions > 0 || r.write_stalled > 0 || r.files_l0 >= 10)
        }).count()
    } else {
        0
    };

    let correlation_pct = if n_spikes > 0 {
        100.0 * spikes_with_compaction as f64 / n_spikes as f64
    } else {
        100.0 // pas de spikes = pas de corrélation à vérifier
    };

    // ── Verdict ───────────────────────────────────────────────────────────────
    let verdict_mode_a = if mode == "a" {
        if pass { "PASS" } else { "FAIL" }
    } else { "N/A" };

    // Mode B PASS si ≥ 80% des spikes cooccurrent avec une compaction
    let verdict_mode_b = if mode == "b" {
        if n_spikes == 0 { "NO_SPIKES" }
        else if correlation_pct >= 80.0 { "CONFIRMED" }
        else { "UNCONFIRMED" }
    } else { "N/A" };

    // ── Affichage ─────────────────────────────────────────────────────────────
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║  T5-ter Mode {} — P3b isolation compaction", mode.to_uppercase());
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  p50:   {:>7} µs  ({:.3} ms)", p50,  p50  as f64 / 1000.0);
    println!("║  p95:   {:>7} µs  ({:.3} ms)", p95,  p95  as f64 / 1000.0);
    println!("║  p99:   {:>7} µs  ({:.3} ms)  ← P3b ≤ 20ms", p99,  p99  as f64 / 1000.0);
    println!("║  p99.9: {:>7} µs  ({:.3} ms)", p999, p999 as f64 / 1000.0);
    println!("╠══════════════════════════════════════════════════════════╣");
    if mode == "a" {
        println!("║  P3b-intrinsèque (sans compaction) : {}",
            if pass { "CONFORME (p99 ≤ 20ms)" } else { "DÉGRADÉE (p99 > 20ms)" });
    } else {
        println!("║  Spikes (≥ {}ms) : {}", SPIKE_THRESHOLD_US / 1000, n_spikes);
        println!("║  Spikes avec compaction active : {} ({:.1}%)", spikes_with_compaction, correlation_pct);
        println!("║  Hypothèse compaction : {}", verdict_mode_b);
    }
    println!("║  Durée mesure : {:.2}s  ({}/cycle)", measure_elapsed.as_secs_f64(), N_MEASURES);
    println!("╚══════════════════════════════════════════════════════════╝\n");

    // ── Verdict JSON ──────────────────────────────────────────────────────────
    let verdict_json = format!(
        concat!(
            "{{\n",
            "  \"benchmark\": \"t5-ter\",\n",
            "  \"mode\": \"{}\",\n",
            "  \"n_entries\": {},\n",
            "  \"n_measures\": {},\n",
            "  \"p50_us\": {},\n",
            "  \"p95_us\": {},\n",
            "  \"p99_us\": {},\n",
            "  \"p99_9_us\": {},\n",
            "  \"p99_target_us\": {},\n",
            "  \"verdict_mode_a\": \"{}\",\n",
            "  \"n_spikes\": {},\n",
            "  \"spikes_with_compaction\": {},\n",
            "  \"correlation_pct\": {:.1},\n",
            "  \"verdict_mode_b\": \"{}\",\n",
            "  \"measure_elapsed_s\": {:.2}\n",
            "}}"
        ),
        mode, n_entries, N_MEASURES,
        p50, p95, p99, p999, P99_TARGET_US,
        verdict_mode_a,
        n_spikes, spikes_with_compaction, correlation_pct,
        verdict_mode_b,
        measure_elapsed.as_secs_f64(),
    );
    std::fs::write(format!("{}/verdict.json", out_dir), &verdict_json)
        .expect("write verdict.json");

    println!("  Résultats : {}", out_dir);
    println!("{}", verdict_json);
}

// ─────────────────────────────────────────────────────────────────────────────
// T7 — H-wake-latence : latence de réveil d'un agent dormant
//
// Mesure le pipeline C2-acquire + wake_agent (ContentStore restore) + send
// pour N_DORMANT agents évincés successivement, répété K fois.
//
// T_wake = p99 de toutes les mesures deliver — première quantification de
// H-wake-latence (spec/04). Aucun critère pass/fail fixé avant cette mesure.
//
// Usage :
//   cargo run -p os-poc-benchmarks --release -- t7-wake [N_AGENTS [N_DORMANT [CAP_IO [K_RUNS]]]]
//   Défauts : N=50  ND=20  CAP=3  K=3
//
// Sortie : results/T7/wake/<timestamp>/{run_<k>.jsonl, verdict.json}
// ─────────────────────────────────────────────────────────────────────────────

fn bench_t7_wake(n_agents: usize, n_dormant: usize, cap_io: usize, k_runs: u32) {
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(bench_t7_wake_async(n_agents, n_dormant, cap_io, k_runs))
}

async fn bench_t7_wake_async(n_agents: usize, n_dormant: usize, cap_io: usize, k_runs: u32) {
    use std::io::Write as _;

    println!("=== T7 — H-wake-latence : latence de réveil agent dormant ===");
    println!("    N_agents={}  N_dormant={}  CAP_IO={}  K={}\n",
             n_agents, n_dormant, cap_io, k_runs);

    // ── Infrastructure (disque réel — NVMe, pas tmpfs) ──────────────────────
    let engine = os_poc_runtime::make_engine();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let data_dir   = format!("results/T7/wake/{}/data", ts);
    let store_path = std::path::Path::new(&data_dir).join("store");
    let log_path   = std::path::Path::new(&data_dir).join("log");
    std::fs::create_dir_all(&store_path).expect("create store dir");
    std::fs::create_dir_all(&log_path).expect("create log dir");

    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&store_path, Some(shared_cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&log_path, Some(shared_cache)).unwrap());

    let t_jit = Instant::now();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    println!("  JIT: {} µs", t_jit.elapsed().as_micros());

    // IoAdmissionQueue : cap_actif = cap_io, queue_capacity généreuse pour éviter NoSlot
    let io_queue = IoAdmissionQueue::new(cap_io, n_dormant * 4);

    // ── Spawn N agents dans le scheduler ───────────────────────────────────
    let mut scheduler = Scheduler::new();
    let mut agent_ids: Vec<AgentId> = Vec::with_capacity(n_agents);

    for i in 0..n_agents {
        let mut id: AgentId = [0u8; 16];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        id[15] = 0x77; // T7 namespace marker
        agent_ids.push(id);
        let instance = ActorInstance::new_precompiled(
            &engine, &module, id, store.clone(), log.clone(),
        ).await.expect("new_precompiled");
        scheduler.register(instance);
    }

    // Stabilisation : les run_loops émettent leur Spawned event
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Warmup : un message par agent pour forcer un commit ContentStore
    for id in &agent_ids {
        let _ = scheduler.send(id, Message::data(vec![0x42u8; 16])).await;
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Les N_DORMANT agents cibles : les derniers dans la liste
    let dormant_ids: Vec<AgentId> = agent_ids[n_agents - n_dormant..].to_vec();

    // ── K runs ──────────────────────────────────────────────────────────────
    let out_dir = format!("results/T7/wake/{}", ts);
    std::fs::create_dir_all(&out_dir).ok();

    let mut all_latencies: Vec<u64> = Vec::with_capacity(k_runs as usize * n_dormant);

    for k in 0..k_runs {
        // Éviction des agents dormants avant chaque run.
        // Run 0 : agents actifs après warmup. Run 1+ : réactivés par deliver du run précédent.
        for id in &dormant_ids {
            if let Err(e) = scheduler.evict_agent(id).await {
                eprintln!("  warn: evict_agent {:?}: {}", &id[..4], e);
            }
        }

        println!("  --- run {} --- ({} agents dormants évincés)", k, n_dormant);

        let run_path = format!("{}/run_{}.jsonl", out_dir, k);
        let mut run_file = std::fs::OpenOptions::new()
            .create(true).write(true).truncate(true)
            .open(&run_path)
            .expect("open run jsonl");

        let mut run_latencies: Vec<u64> = Vec::with_capacity(n_dormant);

        for (i, id) in dormant_ids.iter().enumerate() {
            let t0 = Instant::now();
            let result = scheduler.deliver(
                id,
                Message::data(vec![0x42u8; 16]),
                &io_queue,
                PriorityClass::Foreground,
                &engine,
                &module,
                store.clone(),
                log.clone(),
            ).await;
            let latency_us = t0.elapsed().as_micros() as u64;
            let ok = result.is_ok();

            if !ok {
                eprintln!("  warn: deliver run={} i={}: {:?}", k, i, result.err());
            }

            let line = format!(
                "{{\"run\":{},\"i\":{},\"latency_us\":{},\"ok\":{}}}\n",
                k, i, latency_us, ok
            );
            let _ = run_file.write_all(line.as_bytes());

            run_latencies.push(latency_us);
            all_latencies.push(latency_us);

            println!("    i={:2}  {} µs  ({})", i, latency_us, if ok { "ok" } else { "ERR" });
        }

        // p50/p95/p99 du run
        let mut sorted = run_latencies.clone();
        sorted.sort_unstable();
        let nr = sorted.len();
        let rp = |pm: usize| sorted[(nr * pm / 1000).min(nr - 1)];
        println!("  → run {} : p50={} µs  p95={} µs  p99={} µs\n",
                 k, rp(500), rp(950), rp(990));
    }

    // ── Verdict agrégé (T_wake) ──────────────────────────────────────────────
    all_latencies.sort_unstable();
    let n_total = all_latencies.len();
    let p = |pm: usize| -> u64 { all_latencies[(n_total * pm / 1000).min(n_total - 1)] };
    let p50  = p(500);
    let p95  = p(950);
    let p99  = p(990);
    let p999 = p(999);

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║  T7 — H-wake-latence  (N_dormant={}, K={}, total={})",
             n_dormant, k_runs, n_total);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  p50:   {:>7} µs  ({:.3} ms)", p50,  p50  as f64 / 1000.0);
    println!("║  p95:   {:>7} µs  ({:.3} ms)", p95,  p95  as f64 / 1000.0);
    println!("║  p99:   {:>7} µs  ({:.3} ms)  ← T_wake", p99, p99 as f64 / 1000.0);
    println!("║  p99.9: {:>7} µs  ({:.3} ms)", p999, p999 as f64 / 1000.0);
    println!("╚══════════════════════════════════════════════════════════╝");
    println!("  T_wake = {} µs = {:.3} ms  (p99 deliver dormant → actif)", p99, p99 as f64 / 1000.0);

    let verdict_json = format!(
        concat!(
            "{{\n",
            "  \"benchmark\": \"t7-wake\",\n",
            "  \"n_agents\": {},\n",
            "  \"n_dormant\": {},\n",
            "  \"cap_io\": {},\n",
            "  \"k_runs\": {},\n",
            "  \"n_samples\": {},\n",
            "  \"t_wake_us_p50\": {},\n",
            "  \"t_wake_us_p95\": {},\n",
            "  \"t_wake_us_p99\": {},\n",
            "  \"t_wake_us_p999\": {},\n",
            "  \"module\": \"AGENT_WAT\",\n",
            "  \"note\": \"première mesure T_wake — critère pass/fail à fixer après résultat\"\n",
            "}}"
        ),
        n_agents, n_dormant, cap_io, k_runs, n_total,
        p50, p95, p99, p999,
    );
    std::fs::write(format!("{}/verdict.json", out_dir), &verdict_json)
        .expect("write verdict.json");

    println!("\n  Résultats : {}", out_dir);
    println!("{}", verdict_json);

    drop(scheduler);
}

// ─────────────────────────────────────────────────────────────────────────────
// T8 — Saturation 70 agents actifs : T_wake sous pression cache + compaction
//
// Ce que T7 ne mesure pas : cache block disputé par N_ACTIVE agents actifs,
// compaction L0→L1 déclenchée en arrière-plan, contention C2-acquire.
//
// Structure :
//   - N_ACTIVE agents reçoivent un message tous les TICK_MS (pression log + cache).
//   - N_DORMANT agents cyclent : evict → deliver (mesuré) → evict → ... toutes CYCLE_MS.
//   - Entre chaque deliver : snapshot RocksDB (L0 files, compaction running).
//
// Verdict :
//   - T_wake p99 sous charge vs T7 baseline (311 µs, cache chaud, sans contention).
//   - Fraction de cycles avec compaction active au moment du deliver.
//   - Breach budget 10 ms (critère H-wake-latence).
//
// Usage :
//   cargo run -p os-poc-benchmarks --release -- t8-sat [NA [ND [CAP [MINS [CYCLE_MS]]]]]
//   Défauts : NA=50 ND=20 CAP=3 MINS=5 CYCLE_MS=5000
//
// Sortie : results/T8/sat/<timestamp>/{events.jsonl, verdict.json}
// ─────────────────────────────────────────────────────────────────────────────

fn bench_t8_sat(n_active: usize, n_dormant: usize, cap_io: usize, dur_mins: f64, cycle_ms: u64, prepop_n: u64) {
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(bench_t8_sat_async(n_active, n_dormant, cap_io, dur_mins, cycle_ms, prepop_n))
}

async fn bench_t8_sat_async(
    n_active:  usize,
    n_dormant: usize,
    cap_io:    usize,
    dur_mins:  f64,
    cycle_ms:  u64,
    prepop_n:  u64,
) {
    use std::io::Write as _;

    const TICK_MS:        u64 = 100;   // message vers agents actifs toutes les 100 ms
    const T_WAKE_BUDGET:  u64 = 10_000; // 10 ms — critère H-wake-latence

    let n_total    = n_active + n_dormant;
    let dur_secs   = (dur_mins * 60.0) as u64;

    println!("=== T8 — Saturation : T_wake sous charge nominale ===");
    println!("    N_active={}  N_dormant={}  CAP_IO={}  {:.1} min  cycle={}ms  prepop={}\n",
             n_active, n_dormant, cap_io, dur_mins, cycle_ms, prepop_n);

    // ── Infrastructure (NVMe réel) ──────────────────────────────────────────
    let engine = os_poc_runtime::make_engine();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let data_dir   = format!("results/T8/sat/{}/data", ts);
    let store_path = std::path::Path::new(&data_dir).join("store");
    let log_path   = std::path::Path::new(&data_dir).join("log");
    std::fs::create_dir_all(&store_path).expect("create store dir");
    std::fs::create_dir_all(&log_path).expect("create log dir");

    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&store_path, Some(shared_cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&log_path, Some(shared_cache)).unwrap());

    // ── Pré-population CausalLog (compaction stress) ────────────────────────
    // Quand prepop_n > 0 : injecte N entrées synthétiques sans autocompact pour
    // laisser des fichiers L0 en attente au démarrage du test → déclenche des
    // compactions en arrière-plan pendant les premiers cycles de deliver.
    if prepop_n > 0 {
        println!("  Pré-population CausalLog sans autocompact ({} entrées)...", prepop_n);
        let t_pop = Instant::now();
        log.populate_synthetic(prepop_n, 0).expect("populate_synthetic");
        let l0_after = log.get_rocksdb_int_property("default", "rocksdb.num-files-at-level0")
            .unwrap_or(0);
        println!("  Pré-population terminée en {:.1}s  (L0={} files)\n",
                 t_pop.elapsed().as_secs_f64(), l0_after);
        // Ne pas attendre la stabilisation — on veut mesurer pendant la compaction.
    }

    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let io_queue = IoAdmissionQueue::new(cap_io, n_dormant * 4);

    // ── Spawn N agents ──────────────────────────────────────────────────────
    let mut scheduler = Scheduler::new();
    let mut agent_ids: Vec<AgentId> = Vec::with_capacity(n_total);
    let mut active_senders: Vec<tokio::sync::mpsc::Sender<Message>> = Vec::with_capacity(n_active);

    for i in 0..n_total {
        let mut id: AgentId = [0u8; 16];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        id[15] = 0x88; // T8 namespace
        agent_ids.push(id);
        let tx = scheduler.register(
            ActorInstance::new_precompiled(&engine, &module, id, store.clone(), log.clone())
                .await.expect("new_precompiled"),
        );
        if i < n_active {
            active_senders.push(tx);
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Warmup : un message par agent → commit initial dans ContentStore
    for tx in &active_senders {
        let _ = tx.try_send(Message::data(vec![0x42u8; 16]));
    }
    for id in &agent_ids[n_active..] {
        let _ = scheduler.send(id, Message::data(vec![0x42u8; 16])).await;
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let dormant_ids: Vec<AgentId> = agent_ids[n_active..].to_vec();

    // ── Sortie ──────────────────────────────────────────────────────────────
    let out_dir    = format!("results/T8/sat/{}", ts);
    std::fs::create_dir_all(&out_dir).ok();
    let ev_path    = format!("{}/events.jsonl", out_dir);
    let mut ev_file = std::fs::OpenOptions::new()
        .create(true).write(true).truncate(true)
        .open(&ev_path).expect("open events.jsonl");

    // ── Boucle principale ────────────────────────────────────────────────────
    // Interleave : ticks (active agents) + cycles evict/deliver (dormant agents).
    let mut all_wake_us:    Vec<u64> = Vec::new();
    let mut n_breach:       u64 = 0;
    let mut n_errors:       u64 = 0;
    let mut n_compact_hits: u64 = 0; // delivers où running_compactions > 0
    let mut n_cycles:       u64 = 0;
    let mut n_ticks:        u64 = 0;
    let t0         = Instant::now();
    let mut last_tick  = Instant::now();
    let mut last_cycle = Instant::now();

    println!("  Démarrage — ticks actifs toutes les {}ms, cycles dormants toutes les {}ms",
             TICK_MS, cycle_ms);
    println!("  {:>6}  {:>8}  {:>8}  {:>8}  {:>6}  {:>6}  {:>8}",
             "t(s)", "wake_p50", "wake_p99", "budget", "breaches", "errors", "compact%");
    println!("  {}", "-".repeat(65));

    let mut last_report = Instant::now();
    let report_secs = 30u64;

    while t0.elapsed().as_secs() < dur_secs {
        // ── Tick : messages vers agents actifs ────────────────────────────
        if last_tick.elapsed().as_millis() as u64 >= TICK_MS {
            for tx in &active_senders {
                let _ = tx.try_send(Message::data(vec![0x42u8; 16]));
            }
            n_ticks += 1;
            last_tick = Instant::now();
        }

        // ── Cycle : evict + deliver sur agents dormants ───────────────────
        if last_cycle.elapsed().as_millis() as u64 >= cycle_ms {
            n_cycles += 1;

            // Éviction
            for id in &dormant_ids {
                if let Err(e) = scheduler.evict_agent(id).await {
                    eprintln!("  evict_agent warn: {}", e);
                }
            }

            // Delivers mesurés
            for id in &dormant_ids {
                // Snapshot RocksDB avant deliver (pression compaction)
                let l0_log   = log.get_rocksdb_int_property("default", "rocksdb.num-files-at-level0")
                    .unwrap_or(0);
                let running  = log.get_rocksdb_int_property("default", "rocksdb.num-running-compactions")
                    .unwrap_or(0);
                let stalled  = log.get_rocksdb_int_property("default", "rocksdb.is-write-stalled")
                    .unwrap_or(0);

                if running > 0 { n_compact_hits += 1; }

                let t_start = Instant::now();
                let result  = scheduler.deliver(
                    id,
                    Message::data(vec![0x42u8; 16]),
                    &io_queue,
                    PriorityClass::Foreground,
                    &engine,
                    &module,
                    store.clone(),
                    log.clone(),
                ).await;
                let wake_us = t_start.elapsed().as_micros() as u64;
                let ok      = result.is_ok();

                if !ok  { n_errors  += 1; }
                if wake_us >= T_WAKE_BUDGET { n_breach += 1; }
                all_wake_us.push(wake_us);

                let line = format!(
                    "{{\"cycle\":{},\"wake_us\":{},\"ok\":{},\"l0_log\":{},\"running_compact\":{},\"stalled\":{}}}\n",
                    n_cycles, wake_us, ok, l0_log, running, stalled
                );
                let _ = ev_file.write_all(line.as_bytes());
            }

            last_cycle = Instant::now();
        }

        // ── Rapport périodique ────────────────────────────────────────────
        if last_report.elapsed().as_secs() >= report_secs && !all_wake_us.is_empty() {
            let mut sorted = all_wake_us.clone();
            sorted.sort_unstable();
            let n = sorted.len();
            let rp = |pm: usize| sorted[(n * pm / 1000).min(n - 1)];
            let total_delivers = all_wake_us.len() as u64;
            let compact_pct = 100.0 * n_compact_hits as f64 / total_delivers as f64;
            println!("  {:>6.0}  {:>7}µ  {:>7}µ  {:>7}µ  {:>6}  {:>6}  {:>7.1}%",
                     t0.elapsed().as_secs_f64(),
                     rp(500), rp(990), T_WAKE_BUDGET,
                     n_breach, n_errors, compact_pct);
            last_report = Instant::now();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // ── Verdict agrégé ───────────────────────────────────────────────────────
    let n_samples = all_wake_us.len();
    all_wake_us.sort_unstable();
    let p = |pm: usize| -> u64 {
        if n_samples == 0 { return 0; }
        all_wake_us[(n_samples * pm / 1000).min(n_samples - 1)]
    };
    let p50  = p(500);
    let p95  = p(950);
    let p99  = p(990);
    let p999 = p(999);
    let total_delivers = n_samples as u64;
    let compact_pct = if total_delivers > 0 {
        100.0 * n_compact_hits as f64 / total_delivers as f64
    } else { 0.0 };
    let breach_pct = if total_delivers > 0 {
        100.0 * n_breach as f64 / total_delivers as f64
    } else { 0.0 };
    let budget_ok = n_breach == 0;

    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!("║  T8 — Saturation  (N_active={}, N_dormant={}, {:.1} min)", n_active, n_dormant, dur_mins);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Delivers total   : {}  ({} cycles × {})", total_delivers, n_cycles, n_dormant);
    println!("║  Ticks actifs     : {} × {} agents = {} messages",
             n_ticks, n_active, n_ticks * n_active as u64);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  T_wake p50 :   {:>7} µs  ({:.3} ms)", p50,  p50  as f64 / 1000.0);
    println!("║  T_wake p95 :   {:>7} µs  ({:.3} ms)", p95,  p95  as f64 / 1000.0);
    println!("║  T_wake p99 :   {:>7} µs  ({:.3} ms)  ← sous charge", p99, p99 as f64 / 1000.0);
    println!("║  T_wake p99.9:  {:>7} µs  ({:.3} ms)", p999, p999 as f64 / 1000.0);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  T7 baseline (sans charge) : p99 = 311 µs");
    println!("║  Ratio sous charge         : {:.1}×", p99 as f64 / 311.0_f64.max(1.0));
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Compaction active lors du deliver : {:.1}%", compact_pct);
    println!("║  Breaches budget 10 ms             : {} ({:.2}%)", n_breach, breach_pct);
    println!("║  Erreurs deliver                   : {}", n_errors);
    println!("║  Verdict budget H-wake-latence     : {}", if budget_ok { "PASS" } else { "FAIL" });
    println!("╚══════════════════════════════════════════════════════════╝\n");

    let verdict_json = format!(
        concat!(
            "{{\n",
            "  \"benchmark\": \"t8-sat\",\n",
            "  \"n_active\": {},\n",
            "  \"n_dormant\": {},\n",
            "  \"cap_io\": {},\n",
            "  \"dur_mins\": {:.1},\n",
            "  \"cycle_ms\": {},\n",
            "  \"tick_ms\": {},\n",
            "  \"prepop_n\": {},\n",
            "  \"n_cycles\": {},\n",
            "  \"n_delivers\": {},\n",
            "  \"t_wake_us_p50\": {},\n",
            "  \"t_wake_us_p95\": {},\n",
            "  \"t_wake_us_p99\": {},\n",
            "  \"t_wake_us_p999\": {},\n",
            "  \"t7_baseline_p99_us\": 311,\n",
            "  \"ratio_vs_t7\": {:.2},\n",
            "  \"compact_active_pct\": {:.1},\n",
            "  \"n_breach_10ms\": {},\n",
            "  \"breach_pct\": {:.2},\n",
            "  \"n_errors\": {},\n",
            "  \"budget_us\": {},\n",
            "  \"verdict\": \"{}\"\n",
            "}}"
        ),
        n_active, n_dormant, cap_io, dur_mins, cycle_ms, TICK_MS, prepop_n,
        n_cycles, total_delivers,
        p50, p95, p99, p999,
        p99 as f64 / 311.0_f64.max(1.0),
        compact_pct, n_breach, breach_pct, n_errors,
        T_WAKE_BUDGET,
        if budget_ok { "PASS" } else { "FAIL" },
    );
    std::fs::write(format!("{}/verdict.json", out_dir), &verdict_json)
        .expect("write verdict.json");

    println!("  Résultats : {}", out_dir);
    println!("{}", verdict_json);

    drop(scheduler);
}

// ─────────────────────────────────────────────────────────────────────────────
// compare-sandbox — Wasmtime/Tokio vs processus Linux (proxy seccomp)
//
// Question : quelle propriété WASM achète-t-elle que fork+seccomp n'achète pas,
// et à quel coût ?
//
// Mesures comparées sur N instances simultanées :
//   A. Wasmtime ActorInstance (AGENT_WAT, 1 page = 64 KiB WASM), dormant sur inbox.
//   B. Processus Linux minimal (`/usr/bin/sleep 3600`) — proxy pour un agent
//      process-based + seccomp. Représente le coût infra d'isolation par processus.
//
// Métriques :
//   - RSS delta par instance (mémoire overhead)
//   - Temps de spawn par instance (cold start)
//   - Ratio densité projetée sur 16 GB
//
// Limites documentées :
//   - `sleep` n'inclut pas seccomp (seccomp ≈ 0 overhead RSS, <5% syscall latency).
//   - Un "agent process" réel chargerait libc + son propre runtime → RSS ≥ sleep.
//   - Wasmtime partage le code JIT via CoW ; `sleep` partage aussi ses pages texte.
//   - Le coût inter-process est ignoré ici (IPC vs message inbox Tokio).
//
// Usage : cargo run -p os-poc-benchmarks --release -- compare-sandbox [N]
//         Défaut : N=200
// ─────────────────────────────────────────────────────────────────────────────

fn bench_compare_sandbox(n: usize) {
    println!("=== compare-sandbox — Wasmtime/Tokio vs processus Linux (N={}) ===\n", n);
    let proc_rss_per = bench_compare_sandbox_processes(n);
    println!();
    tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(bench_compare_sandbox_wasmtime(n, proc_rss_per));
}

/// Lit le VmRSS d'un processus enfant depuis /proc/<pid>/status (Ko).
fn read_child_rss_kb(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{}/status", pid))
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn bench_compare_sandbox_processes(n: usize) -> f64 {
    println!("  --- B. Processus Linux (/usr/bin/sleep 3600) ---");
    println!("  (proxy pour agent process-based + seccomp ; seccomp ≈ 0 overhead RSS)\n");

    let t0 = Instant::now();
    let mut children: Vec<std::process::Child> = Vec::with_capacity(n);
    for _ in 0..n {
        let child = std::process::Command::new("/usr/bin/sleep")
            .arg("3600")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        children.push(child);
    }
    let spawn_elapsed = t0.elapsed();

    // Laisser le noyau allouer les structures de chaque enfant
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Lire le RSS de chaque enfant directement via /proc/<pid>/status
    let total_rss_kb: u64 = children.iter()
        .map(|c| read_child_rss_kb(c.id()))
        .sum();
    let delta_per   = total_rss_kb as f64 / n as f64;
    let spawn_us    = spawn_elapsed.as_micros() as f64 / n as f64;
    let density_16g = 16.0 * 1024.0 * 1024.0 / delta_per.max(1.0);

    println!("  {:>5}  {:>10}  {:>14}  {:>12}  {:>14}",
             "N", "RSS total", "overhead/proc", "spawn/proc", "densité 16 GB");
    println!("  {:>5}  {:>9} KB  {:>11.0} KB  {:>9.0} µs  {:>11.0} inst",
             n, total_rss_kb, delta_per, spawn_us, density_16g);

    for mut c in children { let _ = c.kill(); let _ = c.wait(); }

    println!("\n  Note : RSS = somme de /proc/<pid>/status VmRSS de chaque enfant.");
    println!("  `sleep` est le proxy le plus léger ; agent réel (Python LLM) : ≥ 10×.");
    delta_per
}

async fn bench_compare_sandbox_wasmtime(n: usize, proc_rss_per: f64) {
    println!("  --- A. Wasmtime ActorInstance (AGENT_WAT, 1 page = 64 KiB WASM) ---");
    println!("  (run_loop Tokio actif, dormant sur inbox.recv())\n");

    let rss_0 = read_rss_kb();
    let engine = os_poc_runtime::make_engine();

    let store_dir  = TempDir::new().unwrap();
    let log_dir    = TempDir::new().unwrap();
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(store_dir.path(), Some(shared_cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(log_dir.path(), Some(shared_cache)).unwrap());

    let t_jit = Instant::now();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let jit_us = t_jit.elapsed().as_micros();

    let rss_post_infra = read_rss_kb();

    let mut scheduler = Scheduler::new();
    let rss_pre_spawn = read_rss_kb();
    let t0 = Instant::now();

    for i in 0..n {
        let mut id: AgentId = [0u8; 16];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        id[15] = 0xC5;
        let inst = ActorInstance::new_precompiled(
            &engine, &module, id, store.clone(), log.clone(),
        ).await.expect("new_precompiled");
        scheduler.register(inst);
    }

    let spawn_elapsed = t0.elapsed();
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    let rss_post_spawn  = read_rss_kb();
    let delta_total     = rss_post_spawn.saturating_sub(rss_pre_spawn) as f64;
    let delta_per       = delta_total / n as f64;
    let spawn_us        = spawn_elapsed.as_micros() as f64 / n as f64;
    let density_16g     = 16.0 * 1024.0 * 1024.0 / delta_per.max(1.0);

    println!("  Infrastructure partagée (amortie sur {} instances) :", n);
    println!("    Engine+RocksDB : {:+} KB  |  JIT AGENT_WAT : {} µs",
             rss_post_infra as i64 - rss_0 as i64, jit_us);
    println!();
    println!("  {:>5}  {:>10}  {:>14}  {:>12}  {:>14}",
             "N", "RSS total", "overhead/agent", "spawn/agent", "densité 16 GB");
    println!("  {:>5}  {:>9} KB  {:>11.0} KB  {:>9.0} µs  {:>11.0} inst",
             n, rss_post_spawn, delta_per, spawn_us, density_16g);

    drop(scheduler);

    // ── Tableau comparatif récapitulatif ─────────────────────────────────────
    let ratio_ram = proc_rss_per / delta_per.max(1.0);
    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!("║  compare-sandbox — récapitulatif (N={:3})                ║", n);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Métrique             Wasmtime/Tokio   Process (`sleep`) ║");
    println!("║  ─────────────────    ──────────────   ──────────────── ║");
    println!("║  Overhead/instance  {:>5.0} KB        {:>6.0} KB  ({:.0}×)  ║",
             delta_per, proc_rss_per, ratio_ram);
    println!("║  Spawn/instance     {:>5.0} µs        ≥ 1 000 µs*       ║", spawn_us);
    println!("║  Code JIT partagé     Oui (CoW)        Non (fork CoW)   ║");
    println!("║  Déterminisme bytew.  Oui (SEF-6)      Non              ║");
    println!("║  Capabilities ABI     Oui (P4)         Non (seccomp =   ║");
    println!("║                                        filter syscalls  ║");
    println!("║                                        pas ressources)  ║");
    println!("║  IPC overhead         inbox Tokio      pipe/socket      ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!("  * borne inférieure `sleep` — agent réel (Python/Node) : ≥ 10×");
    println!("  * spawn process inclut fork+exec ; Wasmtime : instantiation Store seule");
    println!("\n  Propriété WASM non reproductible par fork+seccomp :");
    println!("    (a) Déterminisme bytewise — deux instances, mêmes inputs → hash identiques.");
    println!("    (b) Capabilities sémantiques — gate sur ressource nommée, pas sur syscall.");
    println!("    (c) Rollback transactionnel atomique — snapshot Merkle hors WASM.");
}

// T5-P3c — latence log.get() sous writes concurrents (P3c, ADR-0036 B-light).
//
// Mesure la latence de CausalLog::get() (chemin B-light agent_add_cause) pendant que
// N_WRITERS threads font des append() concurrents. Vérifie l'hypothèse
// "coût trivial du check B-light" : p99 < 200 µs même sous charge d'écriture.
//
// Usage :
//   cargo run -p os-poc-benchmarks --release -- t5-p3c [N_PREPOP [N_WRITERS [N_READS]]]
//   N_PREPOP  : entrées à précharger avant la mesure (défaut 1_000_000)
//   N_WRITERS : threads d'écriture concurrents (défaut 4)
//   N_READS   : lectures à mesurer (défaut 10_000)
//
// Sortie : résultats JSON dans results/T5-p3c/<timestamp>/verdict.json
fn bench_t5_p3c(n_prepop: u64, n_writers: usize, n_reads: usize) {
    use os_poc_causal_log::LogEntry;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::SystemTime;

    let ts = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let out_dir = format!("results/T5-p3c/{ts}");
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let p99_target_us: u64 = 200;

    println!("=== T5-P3c — log.get() sous writes concurrents (B-light check ADR-0036) ===");
    println!("    N_prepop={n_prepop}  N_writers={n_writers}  N_reads={n_reads}");
    println!("    cible p99 < {p99_target_us} µs\n");

    let db_path = format!("{out_dir}/db");
    std::fs::create_dir_all(&db_path).expect("create db dir");
    let db_path = std::path::Path::new(&db_path);
    let log = Arc::new(CausalLog::open(db_path, None).expect("open log"));

    // Population initiale
    println!("  Population de {n_prepop} entrées...");
    let t_pop = Instant::now();
    log.populate_synthetic(n_prepop, 0).expect("populate_synthetic");
    println!("  Population en {:.1}s\n", t_pop.elapsed().as_secs_f64());

    // Collecter N_READS action_ids parmi les entrées existantes pour les lookups
    // populate_synthetic utilise agent_id [0xAA;16]
    let sample_ids: Vec<[u8; 32]> = {
        let probe_agent = [0xAAu8; 16];
        let ids = log.query_by_agent_range(&probe_agent, None, None).unwrap_or_default();
        if ids.is_empty() {
            // populate_synthetic utilise agent 0xBB ; si vide, fallback entrée récente
            eprintln!("WARN: agent 0xBB vide, sample réduit");
            Vec::new()
        } else {
            let step = (ids.len() / n_reads.min(ids.len())).max(1);
            ids.into_iter().step_by(step).take(n_reads).collect()
        }
    };
    let n_sample = sample_ids.len();
    if n_sample == 0 {
        eprintln!("Aucun action_id disponible pour le benchmark — abandon");
        return;
    }
    println!("  {n_sample} action_ids échantillonnés pour la mesure de get()");

    // Lancer N_WRITERS threads d'écriture en arrière-plan
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut writer_handles = Vec::new();
    for w in 0..n_writers {
        let log_w = log.clone();
        let stop_w = stop_flag.clone();
        let h = thread::spawn(move || {
            let agent_id = {
                let mut id = [0u8; 16];
                id[0] = 0xC0 + w as u8;
                id
            };
            let mut prev: Option<[u8; 32]> = None;
            while !stop_w.load(Ordering::Relaxed) {
                let entry = LogEntry {
                    agent_id,
                    ts_ms: 0,
                    parent_ids: prev.into_iter().collect(),
                    hash_before: [0u8; 32],
                    hash_after: [0u8; 32],
                    emit_payload: None,
                };
                if let Ok(id) = log_w.append(&entry) {
                    prev = Some(id);
                }
            }
        });
        writer_handles.push(h);
    }

    // Laisser les writers démarrer
    thread::sleep(std::time::Duration::from_millis(200));

    // Mesure des latences get()
    let mut latencies_us: Vec<u64> = Vec::with_capacity(n_sample);
    for id in &sample_ids {
        let t0 = Instant::now();
        let _ = log.get(id);
        latencies_us.push(t0.elapsed().as_micros() as u64);
    }

    // Arrêter les writers
    stop_flag.store(true, Ordering::Relaxed);
    for h in writer_handles { let _ = h.join(); }

    // Percentiles
    latencies_us.sort_unstable();
    let n = latencies_us.len();
    let p50  = latencies_us[n * 50  / 100];
    let p95  = latencies_us[n * 95  / 100];
    let p99  = latencies_us[n * 99  / 100];
    let p999 = latencies_us[n * 999 / 1000];
    let pass = p99 < p99_target_us;

    println!("\n  Résultats ({n} lectures, {n_writers} writers concurrents) :");
    println!("    p50  = {p50} µs");
    println!("    p95  = {p95} µs");
    println!("    p99  = {p99} µs   (cible < {p99_target_us} µs) → {}", if pass { "PASS ✓" } else { "FAIL ✗" });
    println!("    p99.9 = {p999} µs");

    let verdict = format!(
        r#"{{"bench":"t5-p3c","n_prepop":{n_prepop},"n_writers":{n_writers},"n_reads":{n},"p50_us":{p50},"p95_us":{p95},"p99_us":{p99},"p999_us":{p999},"p99_target_us":{p99_target_us},"pass":{pass}}}"#
    );
    std::fs::write(format!("{out_dir}/verdict.json"), &verdict).expect("write verdict");
    println!("  Résultats → {out_dir}/verdict.json");
}
