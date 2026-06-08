// icsr-verifier — phase de vérification du harness de durabilité I-CSR (spec/10 §4).
//
// Rouvre ContentStore + CausalLog sur les mêmes chemins qu'icsr-writer,
// charge le témoin JSON, et vérifie l'invariant I-CSR :
//   ∀ log_entry ∈ journal : log_entry.snapshot_hash ∈ store
//
// Asymétrie (spec/10 §4.2) :
//   LogEntryMissing  — perte d'écriture admise (régime no-force, ADR-0027 §D1).
//   SnapshotMissing  — VIOLATION I-CSR (référence pendante — non admis).
//   DataBlockMissing — violation complémentaire (bloc absent malgré log présent).
//
// EXIT CODES :
//   0 — I-CSR satisfait (snapshot_missing == 0 && data_block_missing == 0)
//   1 — I-CSR violé (au moins une SnapshotMissing ou DataBlockMissing)
//   2 — erreur arguments / I/O

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use os_poc_causal_log::CausalLog;
use os_poc_runtime::durability::{verify_icsr, IcsrWitness};
use os_poc_store::{Cache, ContentStore};

#[derive(Debug)]
struct Args {
    db_store:   PathBuf,
    db_log:     PathBuf,
    witness:    PathBuf,
    out_report: PathBuf,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "icsr-verifier — phase de vérification harness I-CSR\n\
\n\
USAGE:\n\
    icsr-verifier --db-store <PATH> --db-log <PATH> --witness <PATH>\n\
                  [--out-report <PATH>]\n\
\n\
ARGS:\n\
    --db-store   <PATH>  Répertoire ContentStore (doit exister).\n\
    --db-log     <PATH>  Répertoire CausalLog (doit exister).\n\
    --witness    <PATH>  Fichier témoin JSON produit par icsr-writer.\n\
    --out-report <PATH>  Rapport JSON (défaut : icsr_report.json dans cwd).\n\
\n\
EXIT CODES:\n\
    0 = I-CSR satisfait\n\
    1 = I-CSR violé\n\
    2 = erreur arguments / I/O\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_store   = None::<PathBuf>;
    let mut db_log     = None::<PathBuf>;
    let mut witness    = None::<PathBuf>;
    let mut out_report = PathBuf::from("icsr_report.json");

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-store"   => { i += 1; db_store   = Some(PathBuf::from(&raw[i])); }
            "--db-log"     => { i += 1; db_log     = Some(PathBuf::from(&raw[i])); }
            "--witness"    => { i += 1; witness    = Some(PathBuf::from(&raw[i])); }
            "--out-report" => { i += 1; out_report = PathBuf::from(&raw[i]); }
            "-h" | "--help" => print_usage_and_exit(0),
            other => {
                eprintln!("Argument inconnu: {other}");
                print_usage_and_exit(2);
            }
        }
        i += 1;
    }

    Args {
        db_store:   db_store.unwrap_or_else(|| print_usage_and_exit(2)),
        db_log:     db_log.unwrap_or_else(||   print_usage_and_exit(2)),
        witness:    witness.unwrap_or_else(||   print_usage_and_exit(2)),
        out_report,
    }
}

fn main() {
    let args = parse_args();

    // Charger le témoin.
    let witness = IcsrWitness::load(&args.witness).unwrap_or_else(|e| {
        eprintln!("witness load ({}): {e}", args.witness.display());
        std::process::exit(2);
    });
    eprintln!(
        "[icsr-verifier] témoin chargé : {} commits, cut_mode={}",
        witness.n_commits, witness.cut_mode
    );

    // Rouvrir store + log.
    let shared_cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(&args.db_store, Some(shared_cache.clone())).unwrap_or_else(|e| {
            eprintln!("ContentStore::open: {e}"); std::process::exit(2);
        }),
    );
    let log = Arc::new(
        CausalLog::open(&args.db_log, Some(shared_cache)).unwrap_or_else(|e| {
            eprintln!("CausalLog::open: {e}"); std::process::exit(2);
        }),
    );
    eprintln!("[icsr-verifier] store + log réouverts");

    // Vérifier I-CSR.
    let result = verify_icsr(&store, &log, &witness);

    // Affichage.
    println!("=== I-CSR verify ===");
    println!("checked           : {}", result.checked);
    println!("log_missing       : {} (perte d'écriture admise — régime no-force)", result.log_missing);
    println!("snapshot_missing  : {} (VIOLATION I-CSR — référence pendante)", result.snapshot_missing);
    println!("data_block_missing: {} (bloc absent malgré log présent)", result.data_block_missing);
    println!("icsr_ok           : {}", result.icsr_ok);
    println!("verdict           : {}", if result.icsr_ok { "PASS" } else { "FAIL" });

    if !result.violations.is_empty() {
        println!("violations ({}) :", result.violations.len());
        for v in &result.violations {
            let kind = match v.kind {
                os_poc_runtime::durability::IcsrViolationKind::LogEntryMissing   => "LogEntryMissing",
                os_poc_runtime::durability::IcsrViolationKind::SnapshotMissing   => "SnapshotMissing",
                os_poc_runtime::durability::IcsrViolationKind::DataBlockMissing  => "DataBlockMissing",
            };
            println!("  seq={} kind={} action={}", v.seq, kind, &v.action_id_hex[..8]);
        }
    }

    // Rapport JSON.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let violations_json: String = result
        .violations
        .iter()
        .map(|v| {
            let kind = match v.kind {
                os_poc_runtime::durability::IcsrViolationKind::LogEntryMissing  => "LogEntryMissing",
                os_poc_runtime::durability::IcsrViolationKind::SnapshotMissing  => "SnapshotMissing",
                os_poc_runtime::durability::IcsrViolationKind::DataBlockMissing => "DataBlockMissing",
            };
            format!(
                "    {{\"seq\":{},\"kind\":\"{}\",\"action_id\":\"{}\"}}",
                v.seq, kind, v.action_id_hex
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    let report = format!(
        "{{\n\
  \"timestamp\": \"epoch:{ts}\",\n\
  \"harness\": \"icsr\",\n\
  \"invariant\": \"I-CSR\",\n\
  \"spec_ref\": \"spec/10-modele-durabilite.md §4\",\n\
  \"adr_ref\": \"ADR-0051 §Amendement\",\n\
  \"cut_mode\": \"{cut}\",\n\
  \"agent_id\": \"{aid}\",\n\
  \"n_commits_witness\": {nc},\n\
  \"checked\": {chk},\n\
  \"log_missing\": {lm},\n\
  \"snapshot_missing\": {sm},\n\
  \"data_block_missing\": {dbm},\n\
  \"icsr_ok\": {ok},\n\
  \"violations\": [\n{viol}\n  ],\n\
  \"verdict\": \"{v}\"\n\
}}\n",
        ts = ts,
        cut = witness.cut_mode,
        aid = witness.agent_id_hex,
        nc  = witness.n_commits,
        chk = result.checked,
        lm  = result.log_missing,
        sm  = result.snapshot_missing,
        dbm = result.data_block_missing,
        ok  = result.icsr_ok,
        viol = violations_json,
        v   = if result.icsr_ok { "PASS" } else { "FAIL" },
    );

    std::fs::write(&args.out_report, &report).unwrap_or_else(|e| {
        eprintln!("write report: {e}"); std::process::exit(2);
    });
    eprintln!("[icsr-verifier] rapport écrit : {}", args.out_report.display());

    std::process::exit(if result.icsr_ok { 0 } else { 1 });
}
