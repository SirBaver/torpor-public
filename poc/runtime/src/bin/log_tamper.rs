//! log-tamper — l'« attaquant » : corrompt une entrée du log causal sur disque.
//!
//! Binaire DISTINCT de `log-verify` (l'auditeur) : la séparation juge ≠ partie est visible
//! jusque dans le Cargo.toml. Mute la *valeur* d'une entrée sous sa *clé inchangée* (couche
//! logique `db.put` + flush), cassant l'invariant content-addressing `clé == SHA256(valeur)`.
//! L'attaquant ne calcule, n'affiche et ne vérifie rien : il casse, puis quitte. C'est le
//! vérificateur, lancé ensuite dans un autre process, qui détecte seul.
//!
//! Usage :
//!   log-tamper [--db <path>] [--key <hex32>] [--byte <n>]
//!     --db   : chemin du log (défaut : demo-work/log)
//!     --key  : action_id (64 hex) à corrompre ; défaut = une entrée référencée comme parent
//!     --byte : index d'octet à flipper dans la valeur (défaut : 0)
//!
//! EXIT CODES : 0 = corruption écrite · 2 = erreur I/O / args / cible introuvable
//!
//! Compile uniquement avec `--features demo-tamper` (active causal-log/test-utils).

use os_poc_causal_log::{ActionId, CausalLog, LogEntry};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

fn hx(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn parse_hex32(s: &str) -> Option<ActionId> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db = PathBuf::from("demo-work/log");
    let mut key: Option<ActionId> = None;
    let mut byte: usize = 0;
    let mut blind = false;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--blind" => blind = true,
            "--db" => {
                i += 1;
                db = PathBuf::from(raw.get(i).unwrap_or_else(|| bad("--db nécessite un argument")));
            }
            "--key" => {
                i += 1;
                let s = raw.get(i).unwrap_or_else(|| bad("--key nécessite un argument"));
                key = Some(parse_hex32(s).unwrap_or_else(|| bad("--key doit être 64 hex (32 octets)")));
            }
            "--byte" => {
                i += 1;
                let s = raw.get(i).unwrap_or_else(|| bad("--byte nécessite un argument"));
                byte = s.parse().unwrap_or_else(|_| bad("--byte doit être un entier"));
            }
            "-h" | "--help" => bad("Usage: log-tamper [--db <path>] [--key <hex32>] [--byte <n>] [--blind]"),
            other => bad(&format!("argument inconnu : {other}")),
        }
        i += 1;
    }

    if !db.exists() {
        eprintln!("log-tamper: chemin introuvable : {}", db.display());
        process::exit(2);
    }

    let log = match CausalLog::open_existing(&db, None) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("log-tamper: ouverture impossible ({}): {e}", db.display());
            process::exit(2);
        }
    };

    // Sélection de la cible :
    //   --blind : entrée ET octet tirés à l'horloge, CACHÉS à l'opérateur (l'auditeur révèle).
    //   --key   : cible explicite (le public peut dicter).
    //   défaut  : une entrée référencée comme parent (nœud dont d'autres dépendent).
    let (target, byte, hidden) = if blind {
        let keys = all_keys(&log);
        if keys.is_empty() {
            eprintln!("log-tamper: aucune entrée dans {}", db.display());
            process::exit(2);
        }
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let idx = (seed as usize) % keys.len();
        let b = ((seed >> 17) as usize) % 32; // octet dans l'en-tête sérialisé du LogEntry
        (keys[idx], b, true)
    } else {
        let t = match key {
            Some(k) => k,
            None => match pick_referenced_entry(&log) {
                Some(k) => k,
                None => {
                    eprintln!("log-tamper: aucune entrée corruptible trouvée dans {}", db.display());
                    process::exit(2);
                }
            },
        };
        (t, byte, false)
    };

    match log.corrupt_value_at(&target, byte) {
        Ok(true) => {
            if hidden {
                println!("log-tamper: une entrée a été corrompue À L'AVEUGLE (cible cachée).");
                println!("  L'opérateur ne sait PAS laquelle. Lance `log-verify` : il la retrouvera seul.");
            } else {
                println!("log-tamper: entrée corrompue (valeur mutée, clé inchangée)");
                println!("  action_id : {}", hx(&target));
                println!("  octet     : {byte}");
                println!("  → la clé n'est plus le SHA256 de la valeur. Lance `log-verify` pour le constater.");
            }
            process::exit(0);
        }
        Ok(false) => {
            eprintln!("log-tamper: cible introuvable ou valeur vide : {}", hx(&target));
            process::exit(2);
        }
        Err(e) => {
            eprintln!("log-tamper: échec de l'écriture : {e}");
            process::exit(2);
        }
    }
}

/// Toutes les clés (action_id 32B) de la CF default.
fn all_keys(log: &CausalLog) -> Vec<ActionId> {
    let mut keys = Vec::new();
    for item in log.iter_default_raw() {
        let Ok((k, _)) = item else { continue };
        if k.len() != 32 {
            continue;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&k);
        keys.push(key);
    }
    keys
}

/// Retourne la clé d'une entrée référencée comme parent par au moins une autre entrée.
fn pick_referenced_entry(log: &CausalLog) -> Option<ActionId> {
    let mut referenced: HashSet<ActionId> = HashSet::new();
    let mut keys: Vec<ActionId> = Vec::new();
    for item in log.iter_default_raw() {
        let (k, v) = item.ok()?;
        if k.len() != 32 {
            continue;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&k);
        keys.push(key);
        if let Ok(entry) = bincode::deserialize::<LogEntry>(&v) {
            for p in entry.parent_ids {
                referenced.insert(p);
            }
        }
    }
    // Priorité : une entrée qui existe ET est référencée. Sinon, la première venue.
    keys.iter().find(|k| referenced.contains(*k)).copied().or_else(|| keys.first().copied())
}

fn bad(msg: &str) -> ! {
    eprintln!("{msg}");
    process::exit(2);
}
