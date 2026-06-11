// p10_s5_runner — Phase 10 : fairness + priorité sous backend réel (OllamaBackend).
//
// Exercice de S5 avec OllamaBackend à la place de SleepyBackend(100ms).
//
// GARDE-FOU (ADR-0052 §D2) :
//   Verdicts non transférables au hardware cible. Nomme backend + hardware.
//
// ASSERTIONS :
//   A-priorité (ADR-0022 D1) : les supervisors terminent avant que tous les foreground
//     aient fini — sv_max_ts < fg_max_ts dans le log causal.
//   A-E3 (ADR-0023 D2) : tous complètent avant le budget.
//   A-E1 (ADR-0023 §D3) : parmi les Foreground, l'ordre des slot_acquired_instant
//     suit l'ordre des admission_seq (FIFO intra-classe).
//
// OBSERVATIONS :
//   queue_stats(), t_infer_ms médiane/p99.
//
// EXIT CODES :
//   0 — PASS
//   1 — FAIL
//   2 — erreur config / Ollama injoignable

use std::sync::Arc;
use std::time::{Duration, Instant};

use os_poc_causal_log::CausalLog;
use os_poc_runtime::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
use os_poc_runtime::inference::{InferencePool, OllamaBackend, PriorityClass};
use os_poc_runtime::make_engine;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_store::{ContentStore, Cache};

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
    n_foreground: usize,
    n_supervisor: usize,
    pool_cap:     usize,
    budget_secs:  u64,
    endpoint:     String,
    model:        String,
    out_report:   String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            n_foreground: 3,
            n_supervisor: 1,
            pool_cap:     1,
            budget_secs:  900,
            endpoint:     "http://localhost:11434".into(),
            model:        "qwen2.5:3b".into(),
            out_report:   "p10_s5_report.json".into(),
        }
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let raw: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--n-foreground" => { i += 1; a.n_foreground = raw[i].parse().expect("--n-foreground"); }
            "--n-supervisor" => { i += 1; a.n_supervisor = raw[i].parse().expect("--n-supervisor"); }
            "--pool-cap"     => { i += 1; a.pool_cap     = raw[i].parse().expect("--pool-cap"); }
            "--budget-secs"  => { i += 1; a.budget_secs  = raw[i].parse().expect("--budget-secs"); }
            "--endpoint"     => { i += 1; a.endpoint     = raw[i].clone(); }
            "--model"        => { i += 1; a.model        = raw[i].clone(); }
            "--out-report"   => { i += 1; a.out_report   = raw[i].clone(); }
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

fn has_infer_response(log: &CausalLog, id: &[u8; 16]) -> bool {
    read_agent_envelopes(log, id)
        .iter()
        .any(|env| env.emit_type == os_poc_causal_log::EmitType::InferenceResponse as u8)
}

fn get_resp_ts_us(log: &CausalLog, id: &[u8; 16]) -> Option<u64> {
    read_agent_envelopes(log, id)
        .into_iter()
        .find(|env| env.emit_type == os_poc_causal_log::EmitType::InferenceResponse as u8)
        .map(|env| env.ts_us)
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = parse_args();
    let n_total = args.n_foreground + args.n_supervisor;

    eprintln!("=== P10-S5 : fairness + priorité sous backend réel ===");
    eprintln!("backend     : OllamaBackend / {} @ {}", args.model, args.endpoint);
    eprintln!("foreground  : {}  supervisor : {}  pool_cap : {}  budget : {}s",
        args.n_foreground, args.n_supervisor, args.pool_cap, args.budget_secs);
    eprintln!("GARDE-FOU : verdicts non transférables au hardware cible (ADR-0052 §D2)");

    eprintln!("\n[1/5] Vérification connectivité Ollama...");
    if !check_ollama(&args.endpoint) {
        eprintln!("ERREUR : Ollama injoignable à {}.", args.endpoint);
        std::process::exit(2);
    }
    eprintln!("OK.");

    eprintln!("[2/5] Initialisation runtime...");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let tmp_dir = std::path::PathBuf::from(format!("/tmp/p10-s5-{ts}"));
    std::fs::create_dir_all(&tmp_dir).expect("create tmp_dir");

    let cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&tmp_dir.join("store"), Some(cache.clone())).unwrap());
    let log   = Arc::new(CausalLog::open(&tmp_dir.join("log"),   Some(cache)).unwrap());
    let engine = make_engine();
    let module = wasmtime::Module::new(&engine, INFER_WAT_REAL).expect("Module");

    let pool = Arc::new(InferencePool::new_with_queue_params(
        args.pool_cap,
        n_total + 4,
        30_000,
        OllamaBackend { model: args.model.clone(), endpoint: args.endpoint.clone() },
    ));
    let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

    let fg_infer_fn = InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground);
    let sv_infer_fn = InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Supervisor);

    let mut scheduler = Scheduler::new();
    scheduler.set_cancel_fn(cancel_fn);
    let cap_store = scheduler.cap_store.clone();

    eprintln!("[3/5] Spawn agents...");

    let mut fg_txs = Vec::new();
    let mut fg_ids: Vec<[u8; 16]> = Vec::new();
    for i in 0..args.n_foreground {
        let mut id = [0u8; 16];
        id[0] = 0xF0 | (i as u8);
        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, id,
            Arc::clone(&store), Arc::clone(&log),
            Arc::clone(&cap_store), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            fg_infer_fn.clone(),
        ).await.unwrap();
        let tx = scheduler.register(actor);
        fg_txs.push(tx);
        fg_ids.push(id);
    }

    let mut sv_txs = Vec::new();
    let mut sv_ids: Vec<[u8; 16]> = Vec::new();
    for i in 0..args.n_supervisor {
        let mut id = [0u8; 16];
        id[0] = 0x5A | (i as u8);
        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, id,
            Arc::clone(&store), Arc::clone(&log),
            Arc::clone(&cap_store), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            sv_infer_fn.clone(),
        ).await.unwrap();
        let tx = scheduler.register(actor);
        sv_txs.push(tx);
        sv_ids.push(id);
    }

    // Build history
    for tx in fg_txs.iter().chain(sv_txs.iter()) {
        tx.send(Message::data(vec![0x00])).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Foreground d'abord pour que les Supervisor les dépassent (test A-priorité)
    eprintln!("[4/5] Déclenchement inférences simultanées...");
    let t_start = Instant::now();
    for tx in fg_txs.iter() {
        tx.send(Message::data(vec![0x07])).await.unwrap();
    }
    tokio::task::yield_now().await;
    for tx in sv_txs.iter() {
        tx.send(Message::data(vec![0x07])).await.unwrap();
    }

    eprintln!("[5/5] Attente complétion (budget: {}s)...", args.budget_secs);
    let budget   = Duration::from_secs(args.budget_secs);
    let deadline = tokio::time::Instant::now() + budget;

    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;

        let all_done = fg_ids.iter().chain(sv_ids.iter())
            .all(|id| has_infer_response(&log, id));
        let elapsed = t_start.elapsed();
        let stats   = pool.queue_stats();
        eprintln!("  {:.0}s | admitted={} promoted={} active={}",
            elapsed.as_secs_f64(), stats.total_admitted, stats.total_promoted, pool.active_count());

        if all_done || tokio::time::Instant::now() >= deadline {
            break;
        }
    }

    let elapsed_ms = t_start.elapsed().as_millis() as u64;
    drop(fg_txs);
    drop(sv_txs);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Évaluation des assertions ─────────────────────────────────────────────

    let fg_done = fg_ids.iter().filter(|id| has_infer_response(&log, id)).count();
    let sv_done = sv_ids.iter().filter(|id| has_infer_response(&log, id)).count();

    let fg_ts: Vec<u64> = fg_ids.iter().filter_map(|id| get_resp_ts_us(&log, id)).collect();
    let sv_ts: Vec<u64> = sv_ids.iter().filter_map(|id| get_resp_ts_us(&log, id)).collect();

    let sv_max_ts = sv_ts.iter().max().copied().unwrap_or(u64::MAX);
    let fg_max_ts = fg_ts.iter().max().copied().unwrap_or(0);

    let a_priority_applicable = !sv_ts.is_empty() && !fg_ts.is_empty();
    let a_priority = a_priority_applicable && sv_max_ts < fg_max_ts;

    let a_e3 = fg_done == args.n_foreground && sv_done == args.n_supervisor;

    // A-E1 : FIFO intra-classe via QueueTrace.
    // Trier par slot_acquired_instant (ordre réel d'acquisition) puis vérifier que
    // admission_seq est non-décroissant — l'agent admis en premier doit obtenir le
    // slot en premier. Comparer admission_seq (ordre logique) évite les faux FAIL
    // causés par l'ordonnancement OS quand pool_cap > 1 (F5, REVIEW-2026-05-30).
    let traces = pool.queue_traces();
    let mut fg_trace_pairs: Vec<(Instant, u64)> = traces.iter()
        .filter(|t| t.priority_class_at_admission == PriorityClass::Foreground
                 || t.promoted_from == Some(PriorityClass::Foreground))
        .filter_map(|t| t.slot_acquired_instant.map(|si| (si, t.admission_seq)))
        .collect();
    fg_trace_pairs.sort_by_key(|(instant, _)| *instant);

    let a_e1 = fg_trace_pairs.len() < 2 || {
        fg_trace_pairs.windows(2).all(|w| w[0].1 <= w[1].1)
    };

    let final_stats = pool.queue_stats();

    // Overhead
    let mut t_infer_samples: Vec<u64> = vec![];
    for id in fg_ids.iter().chain(sv_ids.iter()) {
        let envs = read_agent_envelopes(&log, id);
        let mut req_ts: Option<u64> = None;
        for env in envs {
            match env.emit_type {
                t if t == os_poc_causal_log::EmitType::InferenceRequest as u8  => { req_ts = Some(env.ts_us); }
                t if t == os_poc_causal_log::EmitType::InferenceResponse as u8 => {
                    if let Some(rts) = req_ts {
                        t_infer_samples.push(env.ts_us.saturating_sub(rts) / 1000);
                    }
                }
                _ => {}
            }
        }
    }
    t_infer_samples.sort_unstable();
    let t_median = t_infer_samples.get(t_infer_samples.len() / 2).copied().unwrap_or(0);
    let t_p99    = t_infer_samples.get(t_infer_samples.len() * 99 / 100).copied().unwrap_or(0);

    let verdict = if a_e3 && a_e1 && (a_priority || !a_priority_applicable) { "PASS" } else { "FAIL" };

    let a_priority_note = if a_priority_applicable {
        format!("sv_max={}us fg_max={}us", sv_max_ts, fg_max_ts)
    } else {
        "non applicable (données insuffisantes)".into()
    };

    let report = serde_json::json!({
        "scenario": "P10-S5",
        "phase": 10,
        "date_unix": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0),
        "backend": "OllamaBackend",
        "model": args.model,
        "endpoint": args.endpoint,
        "hardware_note": "non-representative — see ADR-0052 §D2",
        "config": {
            "n_foreground": args.n_foreground,
            "n_supervisor": args.n_supervisor,
            "pool_cap": args.pool_cap,
            "budget_secs": args.budget_secs,
        },
        "assertions": {
            "a_priority": {
                "pass": a_priority || !a_priority_applicable,
                "applicable": a_priority_applicable,
                "note": a_priority_note,
            },
            "a_e3_no_starvation": {
                "pass": a_e3,
                "fg_done": fg_done,
                "sv_done": sv_done,
                "fg_expected": args.n_foreground,
                "sv_expected": args.n_supervisor,
            },
            "a_e1_fifo_intraclass": {
                "pass": a_e1,
                "n_fg_traces": fg_trace_pairs.len(),
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
    eprintln!("  A-priorité  : {} ({})", if a_priority || !a_priority_applicable { "PASS" } else { "FAIL" }, a_priority_note);
    eprintln!("  A-E3 famine : {} (fg={}/{} sv={}/{})", if a_e3 { "PASS" } else { "FAIL" },
        fg_done, args.n_foreground, sv_done, args.n_supervisor);
    eprintln!("  A-E1 FIFO   : {} ({} traces fg)", if a_e1 { "PASS" } else { "FAIL" }, fg_trace_pairs.len());
    eprintln!("  t_infer     : médiane={}ms  p99={}ms", t_median, t_p99);
    eprintln!("  elapsed     : {}ms", elapsed_ms);
    eprintln!("rapport → {}", args.out_report);

    // Shutdown ordonné : drop(fg_txs/sv_txs) plus haut ne suffit pas — le Scheduler
    // garde ses propres clones de senders et ne joint jamais les run_loops. Évincer
    // (= joindre) puis dropper les Arcs store/log AVANT remove_dir_all, sinon
    // suppression d'une RocksDB ouverte + process::exit() course les threads
    // background C++ (abort selon la glibc). Même séquence que sef1/s12_runner.
    for id in fg_ids.iter().chain(sv_ids.iter()) {
        if scheduler.is_dormant(id) {
            continue; // déjà évincé : sa run_loop a déjà été jointe par evict_agent
        }
        // Err = agent déjà terminé/reapé : rien à joindre, on ignore.
        let _ = scheduler.evict_agent(id).await;
    }
    drop(scheduler);
    drop(store);
    drop(log);

    std::fs::remove_dir_all(&tmp_dir).ok();
    std::process::exit(if verdict == "PASS" { 0 } else { 1 });
}
