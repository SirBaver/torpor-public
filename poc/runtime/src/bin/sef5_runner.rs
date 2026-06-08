// sef5-runner — binaire de test SEF-5 (traçabilité causale — P3 / P3a).
//
// CONTRAT (spec/benchmarks/equivalence-scenarios.md §SEF-5) :
//   « Pour tout action_id retourné précédemment par append, get(action_id) produit
//     l'entrée correspondante en p99 ≤ 10 ms sur un log de 10⁸ actions.
//     La complétude sémantique du contenu (parents causaux, hashes avant/après,
//     payload d'émission attendu) est vérifiée par comparaison avec le ground truth. »
//
// MÉTHODE :
//   1. (Population) populate_synthetic(N, N_SAMPLES) peuple le log avec N entrées
//      et retourne N_SAMPLES action_ids échantillonnés uniformément.
//      Ground truth de chaque entrée — déterministe par construction :
//        agent_id   = [0xAA; 16]
//        hash_before= [0xAA; 32]
//        hash_after = [0xBB; 32]
//        emit_payload = None
//        parent_ids ≤ 1 (chaîne linéaire)
//        entry.action_id() == id  (intégrité content-addressed)
//
//   2. (Complétude — P-β) Pour chacun des N_SAMPLES action_ids, on appelle
//      log.get(id) et on vérifie 5 sous-propriétés :
//        c1  retourne Some(entry)
//        c2  entry.action_id() == id  (intégrité SHA-256)
//        c3  entry.agent_id == [0xAA;16]
//        c4  entry.hash_before == [0xAA;32]  &&  entry.hash_after == [0xBB;32]
//        c5  entry.emit_payload.is_none()
//
//   3. (Performance — P-α) N_READS appels log.get() sur les action_ids
//      échantillonnés. Distribution de latences → p99 ≤ P99_TARGET_US (10 ms).
//
// EXIT CODES :
//   0 — pass (P-α ET P-β tiennent)
//   1 — fail (au moins une propriété viole SEF-5)
//   2 — erreur arguments / I/O

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use os_poc_causal_log::CausalLog;
use os_poc_store::Cache;

const P99_TARGET_US: u64 = 10_000; // 10 ms — P3a spec/02 §P3

// ── Arguments ────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Args {
    db_dir:          PathBuf,
    n_entries:       u64,
    n_samples:       usize,
    n_reads:         usize,
    out_report:      PathBuf,
    /// Si Some(path) : sauvegarder les action_ids échantillonnés après population.
    save_samples:    Option<PathBuf>,
    /// Si Some(path) : charger les ids depuis ce fichier et sauter la population.
    load_samples:    Option<PathBuf>,
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "sef5-runner — SEF-5 traçabilité causale (P3a — spec/02 §P3)\n\
\n\
USAGE:\n\
    sef5-runner --db-dir <PATH>\n\
                [--n-entries <N>]        (défaut : 100000000 = 10⁸)\n\
                [--n-samples <K>]        (complétude — défaut : 1000)\n\
                [--n-reads   <R>]        (latence — défaut : 10000)\n\
                [--save-samples <PATH>]  (sauvegarder les ids après population)\n\
                [--load-samples <PATH>]  (charger les ids, sauter la population)\n\
                [--out-report <PATH>]\n\
\n\
EXIT CODES:\n\
    0 = pass (P-α p99 ≤ 10 ms  ET  P-β complétude 100%)\n\
    1 = fail\n\
    2 = erreur arguments / I/O\n"
    );
    std::process::exit(code);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db_dir:       Option<PathBuf> = None;
    let mut n_entries:    u64   = 100_000_000;
    let mut n_samples:    usize = 1_000;
    let mut n_reads:      usize = 10_000;
    let mut out_report    = PathBuf::from("report.json");
    let mut save_samples: Option<PathBuf> = None;
    let mut load_samples: Option<PathBuf> = None;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db-dir"       => { i += 1; db_dir = Some(PathBuf::from(&raw[i])); }
            "--n-entries"    => {
                i += 1;
                n_entries = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-entries doit être u64"); std::process::exit(2);
                });
            }
            "--n-samples"    => {
                i += 1;
                n_samples = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-samples doit être usize"); std::process::exit(2);
                });
            }
            "--n-reads"      => {
                i += 1;
                n_reads = raw[i].parse().unwrap_or_else(|_| {
                    eprintln!("--n-reads doit être usize"); std::process::exit(2);
                });
            }
            "--save-samples" => { i += 1; save_samples = Some(PathBuf::from(&raw[i])); }
            "--load-samples" => { i += 1; load_samples = Some(PathBuf::from(&raw[i])); }
            "--out-report"   => { i += 1; out_report = PathBuf::from(&raw[i]); }
            "-h" | "--help" => print_usage_and_exit(0),
            other => {
                eprintln!("Argument inconnu: {other}");
                print_usage_and_exit(2);
            }
        }
        i += 1;
    }

    Args {
        db_dir:       db_dir.unwrap_or_else(|| print_usage_and_exit(2)),
        n_entries,
        n_samples,
        n_reads,
        out_report,
        save_samples,
        load_samples,
    }
}

/// Sérialise des action_ids en JSON (tableau de hex strings).
fn save_ids_json(path: &PathBuf, ids: &[[u8; 32]]) {
    let items: Vec<String> = ids.iter().map(|id| format!("\"{}\"", hex_encode(id))).collect();
    let content = format!("[{}]", items.join(","));
    std::fs::write(path, content).unwrap_or_else(|e| {
        eprintln!("save_samples write {}: {e}", path.display()); std::process::exit(2);
    });
}

/// Charge des action_ids depuis un JSON produit par save_ids_json.
fn load_ids_json(path: &PathBuf) -> Vec<[u8; 32]> {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("load_samples read {}: {e}", path.display()); std::process::exit(2);
    });
    // Parsing minimaliste sans dépendance serde : le format est ["hex","hex",...].
    content
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .filter_map(|tok| {
            let hex = tok.trim().trim_matches('"');
            if hex.len() != 64 { return None; }
            let mut out = [0u8; 32];
            for (j, pair) in hex.as_bytes().chunks(2).enumerate() {
                let s = std::str::from_utf8(pair).ok()?;
                out[j] = u8::from_str_radix(s, 16).ok()?;
            }
            Some(out)
        })
        .collect()
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, byte| {
        s.push_str(&format!("{:02x}", byte));
        s
    })
}

fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() { return 0; }
    sorted[(sorted.len() * p / 1000).min(sorted.len() - 1)]
}

fn main() {
    let args = parse_args();

    std::fs::create_dir_all(&args.db_dir).unwrap_or_else(|e| {
        eprintln!("mkdir {}: {e}", args.db_dir.display());
        std::process::exit(2);
    });

    eprintln!(
        "[sef5-runner] db={} n_entries={} n_samples={} n_reads={}",
        args.db_dir.display(), args.n_entries, args.n_samples, args.n_reads
    );

    // Partage du block cache avec le log (256 MB).
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let log = Arc::new(
        CausalLog::open(&args.db_dir, Some(shared_cache))
            .unwrap_or_else(|e| { eprintln!("CausalLog::open: {e}"); std::process::exit(2); })
    );

    // ── Étape 1 : population (ou chargement ids depuis fichier) ──────────────
    let (sample_ids, pop_secs): (Vec<[u8; 32]>, f64) = if let Some(ref path) = args.load_samples {
        // Mode réutilisation : DB déjà peuplée, ids chargés depuis fichier.
        eprintln!("[sef5] chargement des ids depuis {}", path.display());
        let ids = load_ids_json(path);
        eprintln!("[sef5] {} ids chargés (population ignorée)", ids.len());
        (ids, 0.0)
    } else {
        let n_samples_clamped = args.n_samples.max(args.n_reads);
        eprintln!("[sef5] population de {} entrées (sample_size={})...", args.n_entries, n_samples_clamped);
        let t_pop = Instant::now();
        let ids = log.populate_synthetic(args.n_entries, n_samples_clamped)
            .unwrap_or_else(|e| { eprintln!("populate_synthetic: {e}"); std::process::exit(2); });
        let secs = t_pop.elapsed().as_secs_f64();
        eprintln!("[sef5] population OK en {secs:.1}s — {} ids capturés", ids.len());
        if let Some(ref save_path) = args.save_samples {
            save_ids_json(save_path, &ids);
            eprintln!("[sef5] ids sauvegardés dans {}", save_path.display());
        }
        (ids, secs)
    };

    if sample_ids.is_empty() {
        eprintln!("[sef5] FATAL : aucun action_id disponible");
        std::process::exit(2);
    }

    // ── Étape 2 : vérification de complétude (P-β) ───────────────────────────
    // Ground truth de populate_synthetic (agent_id fixe, champs constants).
    let expected_agent_id:   [u8; 16] = [0xAAu8; 16];
    let expected_hash_before:[u8; 32] = [0xAAu8; 32];
    let expected_hash_after: [u8; 32] = [0xBBu8; 32];

    let completeness_ids: Vec<[u8; 32]> = sample_ids.iter().take(args.n_samples).copied().collect();
    let n_completeness = completeness_ids.len();

    eprintln!("[sef5] vérification complétude sur {} entrées...", n_completeness);
    let mut n_pass_completeness: usize = 0;
    let mut first_fail: Option<String> = None;

    for id in &completeness_ids {
        let entry = match log.get(id) {
            Ok(Some(e)) => e,
            Ok(None) => {
                if first_fail.is_none() {
                    first_fail = Some(format!("c1 : get({}) → None", hex_encode(id)));
                }
                continue;
            }
            Err(e) => {
                if first_fail.is_none() {
                    first_fail = Some(format!("get({}) → Err({e})", hex_encode(id)));
                }
                continue;
            }
        };

        // c2 : intégrité content-addressed
        let computed_id = entry.action_id();
        if computed_id != *id {
            if first_fail.is_none() {
                first_fail = Some(format!(
                    "c2 : action_id calculé {} ≠ clé {}",
                    hex_encode(&computed_id), hex_encode(id)
                ));
            }
            continue;
        }

        // c3 : agent_id
        if entry.agent_id != expected_agent_id {
            if first_fail.is_none() {
                first_fail = Some(format!("c3 : agent_id {:?} ≠ attendu", entry.agent_id));
            }
            continue;
        }

        // c4 : hashes
        if entry.hash_before != expected_hash_before || entry.hash_after != expected_hash_after {
            if first_fail.is_none() {
                first_fail = Some(format!(
                    "c4 : hash_before={} hash_after={} (attendu AA/BB)",
                    hex_encode(&entry.hash_before), hex_encode(&entry.hash_after)
                ));
            }
            continue;
        }

        // c5 : emit_payload absent
        if entry.emit_payload.is_some() {
            if first_fail.is_none() {
                first_fail = Some("c5 : emit_payload non-None inattendu".to_string());
            }
            continue;
        }

        n_pass_completeness += 1;
    }

    let p_beta = n_pass_completeness == n_completeness;
    eprintln!(
        "[sef5] complétude : {}/{} pass{}",
        n_pass_completeness, n_completeness,
        first_fail.as_deref().map(|s| format!(" — premier échec : {s}")).unwrap_or_default()
    );

    // ── Étape 3 : mesure de latence (P-α) ────────────────────────────────────
    // On utilise les n_reads premiers ids des échantillons (ou tous si < n_reads).
    let latency_ids: Vec<[u8; 32]> = sample_ids.iter().take(args.n_reads).copied().collect();
    let n_lat = latency_ids.len();

    eprintln!("[sef5] mesure de latence sur {} get()...", n_lat);
    let mut latencies_us: Vec<u64> = Vec::with_capacity(n_lat);
    for id in &latency_ids {
        let t0 = Instant::now();
        let _ = log.get(id);
        latencies_us.push(t0.elapsed().as_micros() as u64);
    }
    latencies_us.sort_unstable();

    let p50  = percentile(&latencies_us, 500);
    let p95  = percentile(&latencies_us, 950);
    let p99  = percentile(&latencies_us, 990);
    let p999 = percentile(&latencies_us, 999);
    let p_alpha = p99 <= P99_TARGET_US;

    let all_pass = p_alpha && p_beta;

    // ── Rapport console ───────────────────────────────────────────────────────
    println!("=== SEF-5 verify ===");
    println!("n_entries         : {}", args.n_entries);
    println!("n_completeness    : {n_completeness}");
    println!("n_reads           : {n_lat}");
    println!("population (s)    : {pop_secs:.1}");
    println!("--- latences get() ---");
    println!("  p50  : {:>7} µs  ({:.2} ms)", p50,  p50  as f64 / 1000.0);
    println!("  p95  : {:>7} µs  ({:.2} ms)", p95,  p95  as f64 / 1000.0);
    println!("  p99  : {:>7} µs  ({:.2} ms)  ← cible ≤ {} µs", p99, p99 as f64 / 1000.0, P99_TARGET_US);
    println!("  p99.9: {:>7} µs  ({:.2} ms)", p999, p999 as f64 / 1000.0);
    println!("--- propriétés ---");
    println!(
        "  P-α  p99 ≤ {} µs (10 ms)    : {}  ({} µs)",
        P99_TARGET_US,
        if p_alpha { "pass" } else { "FAIL" },
        p99
    );
    println!(
        "  P-β  complétude {}/{} entrées : {}",
        n_pass_completeness, n_completeness,
        if p_beta  { "pass" } else { "FAIL" }
    );
    if let Some(ref msg) = first_fail {
        println!("       premier échec : {msg}");
    }
    println!("verdict           : {}", if all_pass { "pass" } else { "fail" });

    // ── Rapport JSON ─────────────────────────────────────────────────────────
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let fail_msg_json = match &first_fail {
        Some(m) => format!("\"{}\"", m.replace('\"', "\\\"")),
        None    => "null".to_string(),
    };
    let report = format!(
        "{{\n\
  \"timestamp\": \"epoch:{ts}\",\n\
  \"scenario\": \"S14-causal-lookup\",\n\
  \"property\": \"P3a\",\n\
  \"sef\": \"SEF-5\",\n\
  \"n_entries\": {n},\n\
  \"n_completeness_checked\": {nc},\n\
  \"n_reads\": {nr},\n\
  \"population_s\": {pop:.2},\n\
  \"p50_us\": {p50},\n\
  \"p95_us\": {p95},\n\
  \"p99_us\": {p99},\n\
  \"p999_us\": {p999},\n\
  \"p99_target_us\": {tgt},\n\
  \"completeness_pass\": {cp},\n\
  \"completeness_total\": {ct},\n\
  \"first_completeness_fail\": {ff},\n\
  \"properties\": {{\n\
    \"P_alpha_p99_le_10ms\": {a},\n\
    \"P_beta_completeness\": {b}\n\
  }},\n\
  \"verdict\": \"{v}\"\n\
}}\n",
        ts = ts,
        n  = args.n_entries,
        nc = n_completeness,
        nr = n_lat,
        pop = pop_secs,
        p50 = p50, p95 = p95, p99 = p99, p999 = p999,
        tgt = P99_TARGET_US,
        cp = n_pass_completeness,
        ct = n_completeness,
        ff = fail_msg_json,
        a  = p_alpha,
        b  = p_beta,
        v  = if all_pass { "pass" } else { "fail" },
    );
    std::fs::write(&args.out_report, report).expect("write --out-report");

    // Sur une DB de 10⁸ entrées, les threads de compaction RocksDB peuvent encore
    // tourner quand le process se termine. process::exit() exécute les handlers
    // atexit() C++ enregistrés par RocksDB, ce qui provoque un SIGSEGV.
    // _exit() bypasse tous les handlers atexit et destructeurs, et laisse l'OS
    // libérer les file handles (y compris les verrous RocksDB).
    // SAFETY : appel POSIX standard, aucun état Rust n'est accédé après ce point.
    extern "C" { fn _exit(status: i32) -> !; }
    unsafe { _exit(if all_pass { 0 } else { 1 }) }
}
