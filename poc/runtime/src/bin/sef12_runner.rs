// sef12-runner — SEF-12 rollback adversarial (campagne P2, ADR-0053 §D-P2).
//
// TROIS VARIANTES (un seul binaire) :
//
//   V2.2 — rollback² : après rollback à k1, nouvelles actions, puis rollback à k2 < k1.
//     rollback_path doit naviguer la "jonction" (nouvelle branche → chaîne originale).
//     Oracle non-trivial : P-δ₂ = l'action suivant le 2ᵉ rollback a hash_before = hash_at_k2.
//
//   V2.3 — rollback + flood immédiat : rollback injecté suivi immédiatement de N actions.
//     Oracle : toutes les actions post-rollback forment une chaîne cohérente depuis hash_at_k.
//
//   V2.4 — liveness sous charge : rollback injecté, inbox saturé de messages avant lui.
//     Oracle : rollback complète (liveness) ; P-δ tient sur l'action post-rollback.
//
// NOTE seq ≠ action_number : seq (dans AgentState) est un compteur monotone jamais
// réinitialisé par rollback. Après rollback au snapshot de seq=49, les nouvelles actions
// ont seq=101, 102, … rollback_path traverse correctement la jonction (seq 110→49→29).

use std::sync::Arc;
use std::time::{Duration, Instant};

use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType, LogEntry};
use os_poc_runtime::actor::{ActorInstance, Message, AGENT_WAT};
use os_poc_runtime::make_engine;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_store::{Cache, ContentStore};
use wasmtime::Module;

// ── helpers (adaptés de sef2_runner) ──────────────────────────────────────

fn hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn all_action_results(log: &CausalLog, agent_id: &[u8; 16]) -> Vec<(LogEntry, EmitEnvelope)> {
    let ids = log.query_by_agent_range(agent_id, None, None).unwrap_or_default();
    let mut out = Vec::new();
    for id in &ids {
        if let Ok(Some(entry)) = log.get(id) {
            if let Some(payload) = entry.emit_payload.as_ref() {
                if let Ok(env) = EmitEnvelope::from_msgpack(payload) {
                    if env.emit_type == EmitType::ActionResult as u8 {
                        out.push((entry, env));
                    }
                }
            }
        }
    }
    out
}

/// Attend que le log contienne au moins `min_count` ActionResult, avec timeout.
async fn wait_action_results(
    log: &CausalLog,
    agent_id: &[u8; 16],
    min_count: usize,
    timeout: Duration,
) -> Vec<(LogEntry, EmitEnvelope)> {
    let deadline = Instant::now() + timeout;
    loop {
        let results = all_action_results(log, agent_id);
        if results.len() >= min_count {
            return results;
        }
        if Instant::now() > deadline {
            eprintln!("[sef12] TIMEOUT : attendu {} ActionResult, vu {}", min_count, results.len());
            std::process::exit(2);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Attend le Nᵉ SchedulerRollback (count 1-indexé), avec timeout.
async fn wait_nth_rollback(
    log: &CausalLog,
    agent_id: &[u8; 16],
    n: usize,
    timeout: Duration,
) -> (LogEntry, u64) {
    let deadline = Instant::now() + timeout;
    loop {
        let ids = log.query_by_agent_range(agent_id, None, None).unwrap_or_default();
        let mut rollbacks = Vec::new();
        for id in &ids {
            if let Ok(Some(entry)) = log.get(id) {
                if let Some(payload) = entry.emit_payload.as_ref() {
                    if let Ok(env) = EmitEnvelope::from_msgpack(payload) {
                        if env.emit_type == EmitType::SchedulerRollback as u8 && env.payload.len() >= 10 {
                            let mut buf = [0u8; 8];
                            buf.copy_from_slice(&env.payload[1..9]);
                            rollbacks.push((entry, u64::from_le_bytes(buf)));
                        }
                    }
                }
            }
        }
        if rollbacks.len() >= n {
            return rollbacks.into_iter().nth(n - 1).unwrap();
        }
        if Instant::now() > deadline {
            eprintln!("[sef12] TIMEOUT : attendu rollback n°{n}, vu {}", rollbacks.len());
            std::process::exit(2);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ── V2.2 rollback² ────────────────────────────────────────────────────────

async fn test_v22(
    tx: &tokio::sync::mpsc::Sender<Message>,
    scheduler: &mut Scheduler,
    agent_id: &[u8; 16],
    store: &ContentStore,
    log: &CausalLog,
) -> bool {
    eprintln!("[sef12/V2.2] Phase 1 : 100 actions");
    for i in 0u64..100 {
        tx.send(Message::data(format!("v22-p1-{i}").into_bytes())).await.expect("send");
    }
    let results = wait_action_results(log, agent_id, 100, Duration::from_secs(30)).await;

    // Capture hash_at_50 (action 50, seq=49 en 0-indexé) et hash_at_30 (seq=29).
    let hash_at_50 = results.iter().find(|(_, e)| e.seq == 49).map(|(le, _)| le.hash_after);
    let hash_at_30 = results.iter().find(|(_, e)| e.seq == 29).map(|(le, _)| le.hash_after);
    let (hash_at_50, hash_at_30) = match (hash_at_50, hash_at_30) {
        (Some(a), Some(b)) => (a, b),
        _ => { eprintln!("[sef12/V2.2] FATAL : hash_at_50 ou hash_at_30 introuvable"); return false; }
    };
    eprintln!("[sef12/V2.2] hash_at_50={} hash_at_30={}", hex(&hash_at_50), hex(&hash_at_30));

    // Rollback-1 : cible seq=49 (action 50).
    eprintln!("[sef12/V2.2] Rollback-1 → target_seq=49");
    scheduler.rollback(agent_id, 49).await.expect("rollback-1");
    let (rb1_entry, rb1_target) = wait_nth_rollback(log, agent_id, 1, Duration::from_secs(5)).await;
    eprintln!("[sef12/V2.2] Rollback-1 observé : hash_after={} target_seq={}", hex(&rb1_entry.hash_after), rb1_target);

    // P-δ₁ : l'action suivant rollback-1 a hash_before = hash_at_50.
    tx.send(Message::data(b"v22-post-rb1".to_vec())).await.expect("send post-rb1");
    let results_after_rb1 = wait_action_results(log, agent_id, 101, Duration::from_secs(10)).await;
    let post_rb1 = results_after_rb1.last().map(|(le, _)| le.clone());
    let p_delta1 = post_rb1.as_ref().map(|e| e.hash_before == hash_at_50).unwrap_or(false);
    eprintln!("[sef12/V2.2] P-δ₁ (post-rb1.hash_before == hash_at_50) : {}", if p_delta1 { "PASS" } else { "FAIL" });

    // Phase 2 : 10 nouvelles actions après rollback-1.
    // seq dans l'agent = 100 + 1 (post-rb1) + 10 = 111 ; snapshots sur nouvelle branche.
    eprintln!("[sef12/V2.2] Phase 2 : 10 nouvelles actions (nouvelle branche)");
    for i in 0u64..10 {
        tx.send(Message::data(format!("v22-p2-{i}").into_bytes())).await.expect("send");
    }
    // 100 (Phase1) + 1 (post-rb1) + 10 (Phase2) = 111
    wait_action_results(log, agent_id, 111, Duration::from_secs(15)).await;

    // Rollback-2 : cible seq=29 (action 30), avant la jonction.
    // rollback_path doit traverser : seq_max→...→101(post-rb1)→49→48→...→29.
    eprintln!("[sef12/V2.2] Rollback-2 → target_seq=29 (traversée jonction nouvelle-branche→chaîne-originale)");
    scheduler.rollback(agent_id, 29).await.expect("rollback-2");
    let (rb2_entry, rb2_target) = wait_nth_rollback(log, agent_id, 2, Duration::from_secs(10)).await;
    eprintln!("[sef12/V2.2] Rollback-2 observé : hash_after={} target_seq={}", hex(&rb2_entry.hash_after), rb2_target);

    // P-α₂ : SchedulerRollback#2 .hash_after = hash_at_30.
    let p_alpha2 = rb2_entry.hash_after == hash_at_30;

    // P-β₂ : header du snapshot cible a seq=29.
    let p_beta2 = store.get_header(&hash_at_30).ok().flatten()
        .map(|h| h.seq == 29).unwrap_or(false);

    // P-γ₂ : payload SchedulerRollback#2 a target_seq=29.
    let p_gamma2 = rb2_target == 29;

    // P-δ₂ : l'action suivant rollback-2 a hash_before = hash_at_30.
    tx.send(Message::data(b"v22-post-rb2".to_vec())).await.expect("send post-rb2");
    // 111 + 1 (post-rb2) = 112
    let results_after_rb2 = wait_action_results(log, agent_id, 112, Duration::from_secs(10)).await;
    let post_rb2 = results_after_rb2.last().map(|(le, _)| le.clone());
    let p_delta2 = post_rb2.as_ref().map(|e| e.hash_before == hash_at_30).unwrap_or(false);

    println!("  P-α₂ SchedulerRollback#2.hash_after == hash_at_30  : {}", if p_alpha2 { "pass" } else { "FAIL" });
    println!("  P-β₂ target header.seq == 29                       : {}", if p_beta2  { "pass" } else { "FAIL" });
    println!("  P-γ₂ payload.target_seq == 29                      : {}", if p_gamma2 { "pass" } else { "FAIL" });
    println!("  P-δ₁ post-rb1.hash_before == hash_at_50            : {}", if p_delta1 { "pass" } else { "FAIL" });
    println!("  P-δ₂ post-rb2.hash_before == hash_at_30            : {}", if p_delta2 { "pass" } else { "FAIL" });

    p_alpha2 && p_beta2 && p_gamma2 && p_delta1 && p_delta2
}

// ── V2.3 rollback + flood immédiat ────────────────────────────────────────
// Rollback injecté, puis N actions envoyées immédiatement après.
// Oracle : chaîne post-rollback cohérente (hash_before de chaque action =
// hash_after de la précédente, depuis hash_at_k).

async fn test_v23(
    tx: &tokio::sync::mpsc::Sender<Message>,
    scheduler: &mut Scheduler,
    agent_id: &[u8; 16],
    log: &CausalLog,
    action_offset: usize,  // nb d'ActionResult déjà présents dans le log (V2.2)
) -> bool {
    let n_build = 50usize;
    eprintln!("[sef12/V2.3] Phase 1 : {n_build} actions");
    for i in 0..n_build {
        tx.send(Message::data(format!("v23-p1-{i}").into_bytes())).await.expect("send");
    }
    let results = wait_action_results(log, agent_id, action_offset + n_build, Duration::from_secs(30)).await;

    // hash_at_25 = action 25 dans cette variante (seq relatif = 25e depuis offset).
    // On prend le 25e résultat depuis l'offset.
    let all_v23 = &results[action_offset..];
    let hash_at_25 = all_v23.get(24).map(|(le, _)| le.hash_after);
    let hash_at_25 = match hash_at_25 {
        Some(h) => h,
        None => { eprintln!("[sef12/V2.3] FATAL : hash_at_25 introuvable"); return false; }
    };
    // target_seq = seq du 25e résultat v23.
    let target_seq_v23 = all_v23.get(24).map(|(_, e)| e.seq).unwrap();
    eprintln!("[sef12/V2.3] hash_at_25={} target_seq={}", hex(&hash_at_25), target_seq_v23);

    // Rollback vers target_seq_v23.
    scheduler.rollback(agent_id, target_seq_v23).await.expect("rollback-v23");

    // Flood immédiat : 30 actions envoyées sans attendre que le rollback soit traité.
    let n_flood = 30usize;
    for i in 0..n_flood {
        tx.send(Message::data(format!("v23-flood-{i}").into_bytes())).await.expect("send flood");
    }

    // Attendre que tous les messages soient drainés.
    let total_expected = action_offset + n_build + n_flood;
    let results_final = wait_action_results(log, agent_id, total_expected, Duration::from_secs(30)).await;

    // Trouver le SchedulerRollback v23 (le 3e rollback, après 2 de V2.2).
    let (rb_entry, _) = wait_nth_rollback(log, agent_id, 3, Duration::from_secs(5)).await;

    // Les ActionResult APRÈS le SchedulerRollback v23 doivent former une chaîne cohérente.
    // On identifie les ActionResult postérieurs au rollback par leur position dans le log.
    // Simplification : on prend les n_flood derniers ActionResult et vérifie la chaîne.
    let post_rb_results: Vec<_> = results_final[action_offset + n_build..].to_vec();

    // Trier par seq (query_by_agent_range trie par ts_ms+action_id, pas seq).
    let mut post_rb_results = post_rb_results;
    post_rb_results.sort_by_key(|(_, e)| e.seq);

    // P-δ : le premier post-rb (par seq) a hash_before = hash_at_25.
    let p_delta = post_rb_results.first().map(|(le, _)| le.hash_before == hash_at_25).unwrap_or(false);

    // P-ordering : chaque post-rb[i].hash_before == post-rb[i-1].hash_after.
    let mut p_ordering = true;
    for i in 1..post_rb_results.len() {
        let prev_hash_after = post_rb_results[i - 1].0.hash_after;
        let curr_hash_before = post_rb_results[i].0.hash_before;
        if prev_hash_after != curr_hash_before {
            p_ordering = false;
            eprintln!("[sef12/V2.3] FAIL chaîne brisée à i={i}: hash_after[i-1]={} ≠ hash_before[i]={}",
                hex(&prev_hash_after), hex(&curr_hash_before));
            break;
        }
    }

    // P-rb-hash : hash_after du rollback = hash_at_25.
    let p_rb_hash = rb_entry.hash_after == hash_at_25;

    println!("  P-δ   premier post-rb.hash_before == hash_at_25    : {}", if p_delta    { "pass" } else { "FAIL" });
    println!("  P-ord chaîne post-rollback cohérente ({n_flood} actions): {}", if p_ordering { "pass" } else { "FAIL" });
    println!("  P-rb  SchedulerRollback.hash_after == hash_at_25    : {}", if p_rb_hash  { "pass" } else { "FAIL" });

    p_delta && p_ordering && p_rb_hash
}

// ── V2.4 liveness sous charge ──────────────────────────────────────────────
// Rollback injecté dans une inbox chargée de messages.
// Oracle : rollback complète (liveness) ; P-δ tient.

async fn test_v24(
    tx: &tokio::sync::mpsc::Sender<Message>,
    scheduler: &mut Scheduler,
    agent_id: &[u8; 16],
    log: &CausalLog,
    action_offset: usize,
) -> bool {
    let n_build = 50usize;
    eprintln!("[sef12/V2.4] Phase 1 : {n_build} actions");
    for i in 0..n_build {
        tx.send(Message::data(format!("v24-p1-{i}").into_bytes())).await.expect("send");
    }
    let results = wait_action_results(log, agent_id, action_offset + n_build, Duration::from_secs(30)).await;

    let all_v24 = &results[action_offset..];
    let hash_at_25 = all_v24.get(24).map(|(le, _)| le.hash_after);
    let hash_at_25 = match hash_at_25 {
        Some(h) => h,
        None => { eprintln!("[sef12/V2.4] FATAL : hash_at_25 introuvable"); return false; }
    };
    let target_seq_v24 = all_v24.get(24).map(|(_, e)| e.seq).unwrap();
    eprintln!("[sef12/V2.4] hash_at_25={} target_seq={}", hex(&hash_at_25), target_seq_v24);

    // Injecter le rollback, puis 80 messages (charge dans l'inbox).
    let rb_inject = Instant::now();
    scheduler.rollback(agent_id, target_seq_v24).await.expect("rollback-v24");
    for i in 0..80usize {
        tx.send(Message::data(format!("v24-load-{i}").into_bytes())).await.expect("send load");
    }

    // Attendre que le rollback apparaisse dans le log (liveness).
    let (rb_entry, _) = wait_nth_rollback(log, agent_id, 4, Duration::from_secs(60)).await;
    let rb_latency_ms = rb_inject.elapsed().as_millis();
    eprintln!("[sef12/V2.4] rollback observé après {} ms (liveness)", rb_latency_ms);

    // Attendre le drain complet.
    let total_expected = action_offset + n_build + 80;
    let results_final = wait_action_results(log, agent_id, total_expected, Duration::from_secs(60)).await;

    // P-liveness : rollback complété (rb_entry existe, déjà vérifié).
    let p_liveness = true;

    // P-rb-hash : hash_after du rollback = hash_at_25.
    let p_rb_hash = rb_entry.hash_after == hash_at_25;

    // P-δ : le premier ActionResult post-rollback (par seq) a hash_before = hash_at_25.
    let mut post_rb_results: Vec<_> = results_final[action_offset + n_build..].to_vec();
    post_rb_results.sort_by_key(|(_, e)| e.seq);
    let p_delta = post_rb_results.first().map(|(le, _)| le.hash_before == hash_at_25).unwrap_or(false);

    println!("  P-liveness rollback complète sous 80 msgs en vol     : {}", if p_liveness { "pass" } else { "FAIL" });
    println!("  P-rb       SchedulerRollback.hash_after == hash_at_25: {}", if p_rb_hash  { "pass" } else { "FAIL" });
    println!("  P-δ        premier post-rb.hash_before == hash_at_25  : {}", if p_delta    { "pass" } else { "FAIL" });
    println!("  (info)     latence rollback observée : {} ms", rb_latency_ms);

    p_liveness && p_rb_hash && p_delta
}

// ── main ───────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let run_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    let base = std::env::temp_dir().join(format!("sef12-{run_id}"));
    let store_path = base.join("store");
    let log_path = base.join("log");
    std::fs::create_dir_all(&store_path).unwrap();
    std::fs::create_dir_all(&log_path).unwrap();

    let agent_id = [0x12u8; 16];
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&store_path, Some(shared_cache.clone())).expect("store"));
    let log   = Arc::new(CausalLog::open(&log_path, Some(shared_cache)).expect("log"));

    let engine = make_engine();
    let module = Module::new(&engine, AGENT_WAT).expect("compile AGENT_WAT");
    let actor  = ActorInstance::new_precompiled(
        &engine, &module, agent_id, Arc::clone(&store), Arc::clone(&log),
    ).await.expect("actor");

    let mut scheduler = Scheduler::new();
    scheduler.set_log_ref(Arc::clone(&log));
    let tx = scheduler.register(actor);

    println!("=== SEF-12 — Rollback adversarial (P2, ADR-0053 §D-P2) ===");

    // V2.2
    println!("\n--- V2.2 rollback² (naviguation jonction nouvelle-branche→chaîne-originale) ---");
    let pass_v22 = test_v22(&tx, &mut scheduler, &agent_id, &store, &log).await;
    println!("verdict V2.2 : {}", if pass_v22 { "PASS" } else { "FAIL" });

    // Compte des ActionResult produits jusqu'ici (100 + 1 + 10 + 1 = 112).
    let offset_v23 = all_action_results(&log, &agent_id).len();

    // V2.3
    println!("\n--- V2.3 rollback + flood immédiat (FIFO ordering) ---");
    let pass_v23 = test_v23(&tx, &mut scheduler, &agent_id, &log, offset_v23).await;
    println!("verdict V2.3 : {}", if pass_v23 { "PASS" } else { "FAIL" });

    let offset_v24 = all_action_results(&log, &agent_id).len();

    // V2.4
    println!("\n--- V2.4 liveness sous charge (80 msgs en vol) ---");
    let pass_v24 = test_v24(&tx, &mut scheduler, &agent_id, &log, offset_v24).await;
    println!("verdict V2.4 : {}", if pass_v24 { "PASS" } else { "FAIL" });

    let all_pass = pass_v22 && pass_v23 && pass_v24;
    println!("\n=== verdict global SEF-12 : {} ===", if all_pass { "PASS" } else { "FAIL" });

    drop(tx);
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::process::exit(if all_pass { 0 } else { 1 });
}
