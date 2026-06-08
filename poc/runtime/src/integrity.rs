//! integrity — audit content-addressing **tiers** du log causal.
//!
//! La politique d'intégrité vit ici (runtime), **pas** dans `causal-log` (Layer 0) : un
//! vérificateur tiers ne doit pas réutiliser le code de mécanisme du composant qu'il audite.
//! Le cœur ne fournit que le mécanisme (`iter_default_raw`, `get`). Voir verdict architecte.
//!
//! Invariant vérifié : pour toute entrée du log, la **clé** sous laquelle elle est indexée
//! est bien `SHA256(valeur_sérialisée)` (content-addressing, comme `git fsck`). Toute mutation
//! de la valeur après écriture, sous une clé inchangée, est donc détectable par recalcul.
//!
//! **Portée.** Détecte (a) la mutation locale incohérente d'une valeur (clé ≠ SHA256(valeur))
//! et (b) les références parent pendantes (un parent référencé absent du log).
//!
//! **Hors portée** — à ne jamais sous-entendre :
//!   - réécriture *cohérente* de tout un sous-arbre par un adversaire ayant accès en écriture
//!     (re-keying : exigerait un chaînage Merkle de tête signé) ;
//!   - corruption bit-rot du stockage (couverte par les checksums de bloc RocksDB, pas ici) ;
//!   - troncature du log (suppression de la dernière entrée laisse un log cohérent plus court).

use os_poc_causal_log::{ActionId, CausalLog, LogEntry, LogError};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Une entrée dont la clé ne correspond plus au SHA256 de sa valeur stockée.
#[derive(Debug, Clone)]
pub struct HashMismatch {
    /// Clé sous laquelle l'entrée est indexée (le hash *attendu*).
    pub stored_key: ActionId,
    /// SHA256 recalculé sur les octets bruts de la valeur (le hash *réel*).
    pub recomputed: ActionId,
}

/// Un enfant référençant un parent qui n'existe pas (ou plus) dans le log.
#[derive(Debug, Clone)]
pub struct DanglingParent {
    pub child_key: ActionId,
    pub missing_parent: ActionId,
}

/// Rapport d'audit content-addressing du log causal.
#[derive(Debug, Default)]
pub struct ContentIntegrityReport {
    pub entries_checked: usize,
    pub hash_mismatches: Vec<HashMismatch>,
    pub dangling_parents: Vec<DanglingParent>,
}

impl ContentIntegrityReport {
    /// `true` si aucune violation détectée.
    pub fn is_clean(&self) -> bool {
        self.hash_mismatches.is_empty() && self.dangling_parents.is_empty()
    }
}

/// Re-parcourt **tout** le log et vérifie l'invariant content-addressing.
///
/// Test primaire `SHA256(octets_valeur) == octets_clé` sur les **octets bruts** : ne dépend
/// d'aucune re-sérialisation, donc robuste même si le format évoluait. La désérialisation
/// n'est utilisée que pour le diagnostic causal secondaire (parents pendants) ; une valeur
/// indésérialisable est déjà signalée par le mismatch de hash.
pub fn verify_content_addressing(log: &CausalLog) -> Result<ContentIntegrityReport, LogError> {
    let mut report = ContentIntegrityReport::default();
    let mut keys: HashSet<ActionId> = HashSet::new();
    let mut child_parents: Vec<(ActionId, Vec<ActionId>)> = Vec::new();

    for item in log.iter_default_raw() {
        let (raw_key, raw_value) = item?; // erreur I/O propagée — jamais avalée
        if raw_key.len() != 32 {
            continue; // garde défensif : la CF default ne contient que des clés 32B
        }
        let mut key: ActionId = [0u8; 32];
        key.copy_from_slice(&raw_key);
        report.entries_checked += 1;
        keys.insert(key);

        // Check primaire : content-addressing sur octets bruts.
        let recomputed: ActionId = Sha256::digest(&raw_value).into();
        if recomputed != key {
            report.hash_mismatches.push(HashMismatch {
                stored_key: key,
                recomputed,
            });
        }

        // Collecte des parents pour le check secondaire (références pendantes).
        if let Ok(entry) = bincode::deserialize::<LogEntry>(&raw_value) {
            if !entry.parent_ids.is_empty() {
                child_parents.push((key, entry.parent_ids));
            }
        }
    }

    // Check secondaire : tout parent référencé doit exister dans le log.
    for (child, parents) in child_parents {
        for parent in parents {
            if !keys.contains(&parent) {
                report.dangling_parents.push(DanglingParent {
                    child_key: child,
                    missing_parent: parent,
                });
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_poc_causal_log::CausalLog;

    fn entry(agent: [u8; 16], parents: Vec<ActionId>) -> LogEntry {
        LogEntry {
            agent_id: agent,
            ts_ms: 1,
            parent_ids: parents,
            hash_before: [0u8; 32],
            hash_after: [1u8; 32],
            emit_payload: Some(vec![1, 2, 3, 4]),
        }
    }

    #[test]
    fn round_trip_clean_then_corruption_detected() {
        let dir = tempfile::tempdir().unwrap();
        let log = CausalLog::open(dir.path(), None).unwrap();
        let a = log.append(&entry(*b"agentA0000000000", vec![])).unwrap();
        let _b = log.append(&entry(*b"agentB0000000000", vec![a])).unwrap();

        // Garde-fou round-trip : append puis verify → intègre (clé == SHA256(valeur)).
        // Si bincode n'était pas déterministe, ce test casserait immédiatement.
        let r = verify_content_addressing(&log).unwrap();
        assert_eq!(r.entries_checked, 2);
        assert!(r.is_clean(), "log fraîchement écrit doit être intègre : {r:?}");

        // Corruption même-clé → exactement un hash mismatch, pointant la bonne entrée.
        assert!(log.corrupt_value_at(&a, 0).unwrap());
        let r2 = verify_content_addressing(&log).unwrap();
        assert_eq!(r2.hash_mismatches.len(), 1);
        assert_eq!(r2.hash_mismatches[0].stored_key, a);
        assert_ne!(r2.hash_mismatches[0].recomputed, a);
        assert!(!r2.is_clean());
    }
}
