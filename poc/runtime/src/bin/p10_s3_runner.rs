// p10_s3_runner — Phase 10 : borne dure d'inférence sous backend réel (OllamaBackend).
//
// Exercice de S3 avec OllamaBackend à la place de SleepyBackend.
//
// GARDE-FOU (ADR-0052 §D2) :
//   Les mesures sont non-transférables au hardware cible (GPU 24 GB, spec/07 §2).
//   Ce verdict caractérise le scheduler sous OllamaBackend sur ce poste de dev.
//   k=4–8 et t≈2,5 s de spec/07 §2 restent des hypothèses non validées.
//
// ASSERTIONS :
//   P-α (no-famine)    : tous les N workers complètent avant le budget.
//   P-β (traceability) : N InferenceRequest + N InferenceResponse dans le log.
//   P-γ (pool vide)    : active_count == 0 à la fin.
//
// OBSERVATIONS (non bloquantes) :
//   queue_stats() : total_admitted, total_promoted, overhead scheduler.
//   t_infer_ms    : temps d'inférence par worker (médiane, p99).
//
// EXIT CODES :
//   0 — PASS (toutes assertions)
//   1 — FAIL (au moins une assertion)
//   2 — erreur config / Ollama injoignable

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_poc_causal_log::CausalLog;
use os_poc_capabilities::CapabilityStore;
use os_poc_runtime::actor::{ActorInstance, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend};
use os_poc_runtime::make_engine;
use os_poc_store::{ContentStore, Cache};

// WAT avec timeout 120 000 ms (2 min) pour backend réel.
const INFER_WAT_REAL: &str = r#"(module
  (import "env" "commit_barrier" (func $cb))
  (import "env" "emit"           (func $emit (param i32 i32 i32)))
  (import "env" "agent_infer"    (func $infer (param i32 i32 i32 i32 i32 i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 7))
      (then
        (i32.store8 (i32.const 256)
          (call $infer
            local.get $ptr
            local.get $len
            (i32.const 512)
            (i32.const 1024)
            (i32.const 260)
            (i32.const 120000)
          )
        )
        call $cb
        i32.const 1
        (i32.const 256)
        (i32.const 1)
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

// ── Args ──────────────────────────────────────────────────────────────────────

struct Args {
    n_workers:    usize,
    pool_cap:     usize,
    budget_secs:  u64,
    endpoint:     String,
    model:        String,
    out_report:   String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            n_workers:   6,
            pool_cap:    2,
            budget_secs: 600,
            endpoint:    "http://localhost:11434".into(),
            model:       "qwen2.5:3b".into(),
            out_report:  "p10_s3_report.json".into(),
        }
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let raw: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--n-workers"   => { i += 1; a.n_workers   = raw[i].parse().expect("--n-workers"); }
            "--pool-cap"    => { i += 1; a.pool_cap     = raw[i].parse().expect("--pool-cap"); }
            "--budget-secs" => { i += 1; a.budget_secs  = raw[i].parse().expect("--budget-secs"); }
            "--endpoint"    => { i += 1; a.endpoint     = raw[i].clone(); }
            "--model"       => { i += 1; a.model        = raw[i].clone(); }
            "--out-report"  => { i += 1; a.out_report   = raw[i].clone(); }
            other => { eprintln!("arg inconnu: {other}"); std::process::exit(2); }
        }
        i += 1;
    }
    a
}

fn check_ollama(endpoint: &str) -> bool {
    let url = format!("{endpoint}/api/tags");
    let output = std::process::Command::new("curl")
        .args(["--silent", "--max-time", "5", "--output", "/dev/null",
               "--write-out", "%{http_code}", &url])
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "200",
        Err(_) => false,
    }
}

// Lit les EmitEnvelope d'un agent depuis le log via l'index secondaire.
fn read_agent_envelopes(log: &CausalLog, id: &[u8; 16]) -> Vec<os_poc_causal_log::EmitEnvelope> {
    let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    let mut envs = Vec::new();
    for action_id in ids {
        let Ok(Some(entry)) = log.get(&action_id) else { continue };
        let Some(payload) = entry.emit_payload else { continue };
        let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(&payload) else { continue };
        envs.push(env);
    }
    envs
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = parse_args();

    eprintln!("=== P10-S3 : borne dure + no-famine sous backend réel ===");
    eprintln!("backend   : OllamaBackend / {} @ {}", args.model, args.endpoint);
    eprintln!("n_workers : {}  pool_cap : {}  budget : {}s", args.n_workers, args.pool_cap, args.budget_secs);
    eprintln!("GARDE-FOU : verdicts non transférables au hardware cible (ADR-0052 §D2)");

    eprintln!("\n[1/4] Vérification connectivité Ollama...");
    if !check_ollama(&args.endpoint) {
        eprintln!("ERREUR : Ollama injoignable à {}. Lancer Ollama d'abord.", args.endpoint);
        std::process::exit(2);
    }
    eprintln!("OK — Ollama répond.");

    eprintln!("[2/4] Initialisation runtime...");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp_dir = std::path::PathBuf::from(format!("/tmp/p10-s3-{ts}"));
    std::fs::create_dir_all(&tmp_dir).expect("create tmp_dir");

    let cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp_dir.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp_dir.join("log"),   Some(cache)).unwrap());
    let engine = make_engine();
    let module = wasmtime::Module::new(&engine, INFER_WAT_REAL).expect("Module INFER_WAT_REAL");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        args.pool_cap,
        args.n_workers + 4,
        30_000,
        OllamaBackend { model: args.model.clone(), endpoint: args.endpoint.clone() },
    ));

    let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
    let mut senders = vec![];
    let mut handles = vec![];
    let mut agent_ids: Vec<[u8; 16]> = vec![];

    eprintln!("[3/4] Spawn de {} workers...", args.n_workers);
    for i in 0..args.n_workers {
        let mut agent_id = [0x50u8; 16];
        agent_id[15] = i as u8;
        agent_ids.push(agent_id);

        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));
        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id,
            Arc::clone(&store), Arc::clone(&log),
            Arc::clone(&cap_store), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("ActorInstance");

        let (tx, rx) = tokio::sync::mpsc::channel(4);
        handles.push(tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx)));
        tx.send(os_poc_runtime::actor::Message::data(vec![0x00])).await.unwrap();
        senders.push(tx);
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Déclencher les inférences simultanément
    let t_start = Instant::now();
    for tx in &senders {
        tx.send(os_poc_runtime::actor::Message::data(vec![0x07])).await.unwrap();
    }

    eprintln!("[4/4] Attente complétion (budget: {}s)...", args.budget_secs);
    let budget   = Duration::from_secs(args.budget_secs);
    let deadline = tokio::time::Instant::now() + budget;

    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let done = agent_ids.iter().all(|id| {
            read_agent_envelopes(&log, id)
                .iter()
                .any(|env| env.emit_type == os_poc_causal_log::EmitType::InferenceResponse as u8)
        });

        let elapsed = t_start.elapsed();
        let stats = pool.queue_stats();
        eprintln!("  {:.0}s elapsed | admitted={} promoted={} active={}",
            elapsed.as_secs_f64(), stats.total_admitted, stats.total_promoted, pool.active_count());

        if done || tokio::time::Instant::now() >= deadline {
            break;
        }
    }

    let elapsed_ms = t_start.elapsed().as_millis() as u64;
    // Shutdown ordonné : fermer les channels PUIS joindre les run_loops (le sleep
    // 300 ms ne garantissait pas leur fin — les Arc<ContentStore>/Arc<CausalLog>
    // restaient vivants au remove_dir_all, course RocksDB/process::exit, abort
    // selon la glibc). Même principe que sef1/s12_runner.
    drop(senders);
    for h in handles {
        let _ = h.await;
    }

    // ── Évaluation des assertions ─────────────────────────────────────────────

    let mut total_req  = 0usize;
    let mut total_resp = 0usize;
    let mut t_infer_ms_samples: Vec<u64> = vec![];

    for id in &agent_ids {
        let envs = read_agent_envelopes(&log, id);
        let mut req_ts: Option<u64> = None;
        for env in envs {
            match env.emit_type {
                t if t == os_poc_causal_log::EmitType::InferenceRequest as u8 => {
                    total_req += 1;
                    req_ts = Some(env.ts_us);
                }
                t if t == os_poc_causal_log::EmitType::InferenceResponse as u8 => {
                    total_resp += 1;
                    if let Some(rts) = req_ts {
                        t_infer_ms_samples.push(env.ts_us.saturating_sub(rts) / 1000);
                    }
                }
                _ => {}
            }
        }
    }

    let final_stats  = pool.queue_stats();
    let active_count = pool.active_count();

    let p_alpha = total_resp == args.n_workers;
    let p_beta  = total_req == args.n_workers && total_resp == args.n_workers;
    let p_gamma = active_count == 0;

    let verdict = if p_alpha && p_beta && p_gamma { "PASS" } else { "FAIL" };

    t_infer_ms_samples.sort_unstable();
    let t_median = t_infer_ms_samples.get(t_infer_ms_samples.len() / 2).copied().unwrap_or(0);
    let t_p99    = t_infer_ms_samples.get(t_infer_ms_samples.len() * 99 / 100).copied().unwrap_or(0);

    let report = serde_json::json!({
        "scenario": "P10-S3",
        "phase": 10,
        "date_unix": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0),
        "backend": "OllamaBackend",
        "model": args.model,
        "endpoint": args.endpoint,
        "hardware_note": "non-representative — see ADR-0052 §D2",
        "config": {
            "n_workers": args.n_workers,
            "pool_cap": args.pool_cap,
            "budget_secs": args.budget_secs,
        },
        "assertions": {
            "p_alpha_no_famine": {
                "pass": p_alpha,
                "n_completed": total_resp,
                "n_expected": args.n_workers,
            },
            "p_beta_traceability": {
                "pass": p_beta,
                "n_req": total_req,
                "n_resp": total_resp,
            },
            "p_gamma_pool_empty": {
                "pass": p_gamma,
                "active_count": active_count,
            },
        },
        "observations": {
            "elapsed_ms": elapsed_ms,
            "t_infer_ms_median": t_median,
            "t_infer_ms_p99": t_p99,
            "queue_stats": {
                "total_admitted": final_stats.total_admitted,
                "total_rejected": final_stats.total_rejected,
                "total_promoted": final_stats.total_promoted,
            },
        },
        "verdict": verdict,
    });

    let report_str = serde_json::to_string_pretty(&report).unwrap();
    std::fs::write(&args.out_report, &report_str).expect("write report");

    eprintln!("\n=== VERDICT: {verdict} ===");
    eprintln!("  P-α no-famine    : {} ({}/{} completed)", if p_alpha { "PASS" } else { "FAIL" }, total_resp, args.n_workers);
    eprintln!("  P-β traceability : {} (req={} resp={})", if p_beta { "PASS" } else { "FAIL" }, total_req, total_resp);
    eprintln!("  P-γ pool vide    : {} (active={})", if p_gamma { "PASS" } else { "FAIL" }, active_count);
    eprintln!("  t_infer médiane  : {}ms  p99: {}ms", t_median, t_p99);
    eprintln!("  elapsed          : {}ms", elapsed_ms);
    eprintln!("rapport → {}", args.out_report);

    // Fermer les DB avant de supprimer leurs fichiers (cf. commentaire shutdown plus haut).
    drop(store);
    drop(log);

    std::fs::remove_dir_all(&tmp_dir).ok();
    std::process::exit(if verdict == "PASS" { 0 } else { 1 });
}
