//! log-verify — vérificateur d'intégrité **tiers** du log causal.
//!
//! Rouvre le log sur disque dans un process DISTINCT de l'écrivain (cold reopen, zéro état
//! mémoire partagé) et recalcule, pour chaque entrée, `clé == SHA256(valeur)`. Le vérificateur
//! ignore tout de ce qui a pu être modifié : il le découvre seul. C'est le cœur du scénario
//! « moins téléphoné » — juge ≠ partie, jusque dans deux exécutables séparés.
//!
//! Usage :
//!   log-verify [--db <path>]      (défaut : demo-work/log)
//!
//! EXIT CODES : 0 = intègre · 1 = violation d'intégrité détectée · 2 = erreur I/O / args
//!
//! Portée : détecte la mutation locale d'une valeur (clé ≠ SHA256(valeur)) et les références
//! parent pendantes. HORS portée : réécriture cohérente d'un sous-arbre (re-keying), bit-rot
//! du stockage (checksums RocksDB), troncature du log. Voir `os_poc_runtime::integrity`.

use os_poc_causal_log::CausalLog;
use os_poc_runtime::integrity::verify_content_addressing;
use std::path::PathBuf;
use std::process;

fn hx(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db = PathBuf::from("demo-work/log");
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db" => {
                i += 1;
                match raw.get(i) {
                    Some(p) => db = PathBuf::from(p),
                    None => {
                        eprintln!("--db nécessite un argument");
                        process::exit(2);
                    }
                }
            }
            "-h" | "--help" => {
                eprintln!("Usage: log-verify [--db <path>]   (défaut: demo-work/log)");
                eprintln!("  Exit: 0=intègre  1=violation détectée  2=erreur I/O/args");
                process::exit(2);
            }
            other => {
                eprintln!("argument inconnu : {other}");
                process::exit(2);
            }
        }
        i += 1;
    }

    if !db.exists() {
        eprintln!("log-verify: chemin introuvable : {}", db.display());
        eprintln!("  (refus d'ouvrir/créer une DB vide — ce serait un faux négatif)");
        process::exit(2);
    }

    // Cold reopen, process distinct : create_if_missing(false) → échoue si la DB n'est pas là.
    let log = match CausalLog::open_existing(&db, None) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("log-verify: ouverture impossible ({}): {e}", db.display());
            process::exit(2);
        }
    };

    let report = match verify_content_addressing(&log) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("log-verify: erreur d'itération (I/O) : {e}");
            process::exit(2);
        }
    };

    println!("== log-verify — audit content-addressing ==");
    println!("  source        : {}", db.display());
    println!("  entrées        : {}", report.entries_checked);
    println!("  hash mismatch  : {}", report.hash_mismatches.len());
    println!("  parents pendants: {}", report.dangling_parents.len());

    if report.is_clean() {
        println!("\n  ✓ INTÈGRE — chaque clé == SHA256(valeur), aucun parent pendant.");
        process::exit(0);
    }

    println!("\n  ✗ FALSIFICATION DÉTECTÉE");
    for m in &report.hash_mismatches {
        println!(
            "    [hash] clé   {}…  stockée sous ce hash",
            &hx(&m.stored_key)[..16]
        );
        println!(
            "           recalc {}…  ≠ clé → valeur modifiée après écriture",
            &hx(&m.recomputed)[..16]
        );
    }
    for d in &report.dangling_parents {
        println!(
            "    [dag]  enfant {}… référence un parent absent {}…",
            &hx(&d.child_key)[..16],
            &hx(&d.missing_parent)[..16]
        );
    }
    process::exit(1);
}
