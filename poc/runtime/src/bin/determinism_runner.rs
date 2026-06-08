// determinism_runner -- K : P5 determinisme de transition d'etat.
//
// Cas d'usage : verification de reproductibilite d'un agent deploye.
// Scénario : rejouer la meme sequence d'inputs sur deux instances isolees
// (stores/logs separes) avec la meme horloge logique -- les action_ids doivent
// etre bit-a-bit identiques.
//
// Methode : echo.wasm (introspect + barrier + emit, sans infer).
//   P-alpha : hash final ContentStore identique entre A et B.
//   P-beta  : sequence ordonnee d'action_ids identique.
//   P-gamma : SHA-256 de la concatenation des action_ids identique.
//
// Propriete demontree : P5 (determinisme conditionnel, ADR-0028).

use std::sync::Arc;
use std::time::{Duration, Instant};

use os_poc_causal_log::CausalLog;
use os_poc_runtime::actor::{ActorInstance, Message};
use os_poc_runtime::clock::{Clock, LogicalClock};
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{ContentStore, Cache};
use sha2::{Digest, Sha256};

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}
fn hex_short(b: &[u8]) -> String {
    b.iter().take(16).map(|x| format!("{x:02x}")).collect()
}

async fn run_instance(
    label: &str,
    base: &std::path::Path,
    agent_id: [u8; 16],
    n_msgs: u64,
    clock_start: u64,
    payloads: &[Vec<u8>],
) -> Result<(String, Vec<[u8; 32]>), String> {
    let dir_store = base.join("store");
    let dir_log   = base.join("log");
    std::fs::create_dir_all(&dir_store).map_err(|e| format!("mkdir ({label}): {e}"))?;
    std::fs::create_dir_all(&dir_log).map_err(|e| format!("mkdir ({label}): {e}"))?;
    let cache = Cache::new_lru_cache(32 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&dir_store, Some(cache.clone()))
        .map_err(|e| format!("store ({label}): {e}"))?);
    let log = Arc::new(CausalLog::open(&dir_log, Some(cache))
        .map_err(|e| format!("log ({label}): {e}"))?);

    let engine = make_engine();
    let module = load_module_from_file(&engine,
        std::path::Path::new("target/wasm32-unknown-unknown/release/examples/echo.wasm"))
        .map_err(|e| format!("echo.wasm ({label}): {e}"))?;

    let clock: Arc<dyn Clock> = Arc::new(LogicalClock::new(clock_start));
    let actor = ActorInstance::new_precompiled_with_clock(
        &engine, &module, agent_id,
        Arc::clone(&store), Arc::clone(&log), clock,
    ).await.map_err(|e| format!("actor ({label}): {e}"))?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(32);
    tokio::spawn(os_poc_runtime::actor::run_loop(actor, rx));

    for p in payloads {
        tx.send(Message::data(p.clone())).await
            .map_err(|e| format!("send ({label}): {e}"))?;
    }

    // echo.wasm n'appelle pas terminate() : Spawned(1) + N*Active + N*Emit = 2N+1
    let min_entries = n_msgs * 2 + 1;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ids = log.query_by_agent_range(&agent_id, None, None).unwrap_or_default();
        if ids.len() as u64 >= min_entries {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let ids2 = log.query_by_agent_range(&agent_id, None, None).unwrap_or_default();
            if ids2.len() == ids.len() {
                drop(tx);
                let mut last_hash = "none".to_string();
                for aid in ids2.iter().rev() {
                    if let Ok(Some(e)) = log.get(aid) {
                        if e.hash_after != [0u8; 32] {
                            last_hash = hex_short(&e.hash_after);
                            break;
                        }
                    }
                }
                return Ok((last_hash, ids2));
            }
        }
        if Instant::now() > deadline {
            return Err(format!("{label}: drain timeout"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::main]
async fn main() {
    const N: u64 = 10;
    const CLOCK_START: u64 = 1_700_000_000_000;
    let agent_id: [u8; 16] = *b"determ-agent-K00";

    eprintln!("=== determinism-runner -- K : P5 determinisme de transition d'etat ===");
    eprintln!("echo.wasm + LogicalClock({}), N={N} messages", CLOCK_START);
    eprintln!();

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let base = std::path::PathBuf::from(format!("/tmp/determinism-{ts}"));

    let payloads: Vec<Vec<u8>> = (0..N)
        .map(|i| format!("determ-msg-{i:04}").into_bytes())
        .collect();

    eprintln!("Instance A...");
    let (hash_a, ids_a) = run_instance("A", &base.join("a"), agent_id, N, CLOCK_START, &payloads)
        .await.expect("instance A");

    eprintln!("Instance B (conditions identiques)...");
    let (hash_b, ids_b) = run_instance("B", &base.join("b"), agent_id, N, CLOCK_START, &payloads)
        .await.expect("instance B");

    let pa = hash_a == hash_b && hash_a != "none";
    let pb = ids_a == ids_b;
    let digest_a: [u8; 32] = {
        let mut h = Sha256::new();
        for id in &ids_a { h.update(id); }
        h.finalize().into()
    };
    let digest_b: [u8; 32] = {
        let mut h = Sha256::new();
        for id in &ids_b { h.update(id); }
        h.finalize().into()
    };
    let pg = digest_a == digest_b;

    eprintln!();
    eprintln!("=== RESULTATS ===");
    eprintln!("  Instance A : {} action_ids | hash_after: {}...", ids_a.len(), &hash_a[..hash_a.len().min(16)]);
    eprintln!("  Instance B : {} action_ids | hash_after: {}...", ids_b.len(), &hash_b[..hash_b.len().min(16)]);
    eprintln!("  P-alpha hash final identique  : {}", if pa { "PASS" } else { "FAIL" });
    eprintln!("  P-beta  sequence action_ids   : {}", if pb { "PASS" } else { "FAIL" });
    eprintln!("  P-gamma SHA-256 log digest    : {}", if pg { "PASS" } else { "FAIL" });
    eprintln!();
    eprintln!("  Premiers action_ids instance A :");
    for (i, aid) in ids_a.iter().take(5).enumerate() {
        let label = if ids_b.get(i) == Some(aid) { "= B" } else { "!= B" };
        eprintln!("    [{}] {} ({})", i, hex8(aid), label);
    }

    let all_pass = pa && pb && pg;
    eprintln!();
    if all_pass {
        eprintln!("PASS -- P5 determinisme (3 proprietes)");
    } else {
        eprintln!("FAIL -- P5 determinisme");
        std::process::exit(1);
    }
}
