// s15-verifier -- oracle P6 valid-prefix pour S15-crash-machine-concurrent.
//
// ADR-0050 D4 / ADR-0027 D3 / spec/02 P6.
//
// EXIT CODES : 0=P6 ok  1=P6 viole  2=erreur I/O

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use os_poc_causal_log::CausalLog;
use os_poc_runtime::durability::{
    verify_p6_concurrent, ConcurrentWitness, P6ViolationKind,
};
use os_poc_store::{Cache, ContentStore};

struct Args {
    db_store:   PathBuf,
    db_log:     PathBuf,
    witness:    PathBuf,
    out_report: PathBuf,
}

fn print_usage() -> ! {
    eprintln!("s15-verifier -- oracle P6 valid-prefix S15");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store   = None::<PathBuf>;
    let mut db_log     = None::<PathBuf>;
    let mut witness    = None::<PathBuf>;
    let mut out_report = PathBuf::from("s15_report.json");
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store"   => { i += 1; db_store   = Some(PathBuf::from(&raw[i])); }
            "--db-log"     => { i += 1; db_log     = Some(PathBuf::from(&raw[i])); }
            "--witness"    => { i += 1; witness    = Some(PathBuf::from(&raw[i])); }
            "--out-report" => { i += 1; out_report = PathBuf::from(&raw[i]); }
            "-h" | "--help" => print_usage(),
            other => { eprintln!("Argument inconnu: {}", other); print_usage(); }
        }
        i += 1;
    }
    Args {
        db_store:   db_store.unwrap_or_else(|| print_usage()),
        db_log:     db_log.unwrap_or_else(||   print_usage()),
        witness:    witness.unwrap_or_else(||   print_usage()),
        out_report,
    }
}

fn main() {
    let args = parse_args();
    let witness = ConcurrentWitness::load(&args.witness).unwrap_or_else(|e| {
        eprintln!("witness load: {}", e); std::process::exit(2);
    });
    let total_acked: usize = witness.agents.iter().map(|a| a.acked_commits.len()).sum();
    eprintln!("[s15-verifier] temoin : {} agents, {} acks, kill_threshold={}",
        witness.n_agents, total_acked, witness.kill_threshold);
    let shared_cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(&args.db_store, Some(shared_cache.clone())).unwrap_or_else(|e| {
            eprintln!("ContentStore::open: {}", e); std::process::exit(2);
        }),
    );
    let log = Arc::new(
        CausalLog::open(&args.db_log, Some(shared_cache)).unwrap_or_else(|e| {
            eprintln!("CausalLog::open: {}", e); std::process::exit(2);
        }),
    );
    eprintln!("[s15-verifier] store + log reouverts (disque froid)");
    let result = verify_p6_concurrent(&store, &log, &witness);
    println!("=== P6 concurrent verify ===");
    println!("n_agents             : {}", result.n_agents);
    println!("total_acked          : {}", result.total_acked);
    println!("total_visible        : {}", result.total_visible);
    println!("total_gaps           : {} (VIOLATION si > 0)", result.total_gaps);
    println!("total_icsr_violations: {} (VIOLATION si > 0)", result.total_icsr_violations);
    println!("total_parent_viol.   : {} (VIOLATION si > 0)", result.total_parent_violations);
    println!("p6_ok                : {}", result.p6_ok);
    println!("verdict              : {}", if result.p6_ok { "PASS" } else { "FAIL" });
    if !result.violations.is_empty() {
        println!("violations ({}):", result.violations.len());
        for v in &result.violations {
            let kind_str = match &v.kind {
                P6ViolationKind::Gap { first_missing_seq, later_present_seq } =>
                    format!("Gap(missing={},later={})", first_missing_seq, later_present_seq),
                P6ViolationKind::SnapshotMissing => "SnapshotMissing".to_string(),
                P6ViolationKind::DataBlockMissing => "DataBlockMissing".to_string(),
                P6ViolationKind::ParentIdMissing { parent_hex } =>
                    format!("ParentIdMissing({})", &parent_hex[..12]),
            };
            println!("  agent={} seq={} action={} kind={}",
                &v.agent_id_hex[..8], v.seq, &v.action_id_hex[..8], kind_str);
        }
    }
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    // Build JSON report (manual string concat to avoid serde in this binary).
    let violations_json = result.violations.iter().map(|v| {
        let kind_json = match &v.kind {
            P6ViolationKind::Gap { first_missing_seq, later_present_seq } =>
                format!(r#"{{"type":"Gap","fms":{},"lps":{}}}"#, first_missing_seq, later_present_seq),
            P6ViolationKind::SnapshotMissing =>
                r#"{"type":"SnapshotMissing"}"#.to_string(),
            P6ViolationKind::DataBlockMissing =>
                r#"{"type":"DataBlockMissing"}"#.to_string(),
            P6ViolationKind::ParentIdMissing { parent_hex } =>
                format!(r#"{{"type":"ParentIdMissing","parent":"{}"}}"#, parent_hex),
        };
        format!(r#"    {{"agent":"{}","seq":{},"action_id":"{}","kind":{}}}"#,
            v.agent_id_hex, v.seq, v.action_id_hex, kind_json)
    }).collect::<Vec<_>>().join(",
");
    let ok_str = if result.p6_ok { "true" } else { "false" };
    let verdict = if result.p6_ok { "PASS" } else { "FAIL" };
    let report = format!(
        "{{
  \"timestamp\": \"epoch:{ts}\",
  \"harness\": \"S15-crash-machine-concurrent\",
  \"substrat\": \"Linux\",
  \"regime\": \"R1\",
  \"n_agents\": {na},
  \"kill_threshold\": {kt},
  \"total_acked\": {ta},
  \"total_visible\": {tv},
  \"total_gaps\": {tg},
  \"total_icsr\": {ti},
  \"total_parent\": {tp},
  \"p6_ok\": {ok},
  \"violations\": [
{viol}
  ],
  \"verdict\": \"{v}\"\n}}
",
        ts=ts, na=result.n_agents, kt=witness.kill_threshold,
        ta=result.total_acked, tv=result.total_visible,
        tg=result.total_gaps, ti=result.total_icsr_violations,
        tp=result.total_parent_violations,
        ok=ok_str, viol=violations_json, v=verdict,
    );
    std::fs::write(&args.out_report, &report).unwrap_or_else(|e| {
        eprintln!("write report: {}", e); std::process::exit(2);
    });
    eprintln!("[s15-verifier] rapport : {}", args.out_report.display());
    std::process::exit(if result.p6_ok { 0 } else { 1 });
}
