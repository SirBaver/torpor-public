// Tracking de capabilities en mémoire — valide P4 (isolation non-ambiante)
//
// Modèle minimal :
//   - Capability = token opaque avec permissions et portée (deux dimensions d'atténuation)
//   - Arbre de dérivation : chaque délégation A→B crée un enfant dans l'arbre
//   - Révocation récursive : invalide le nœud et tous ses descendants
//
// H-revoke : vérifier que le coût de révocation reste < 5% CPU sous W1.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub type CapabilityId = u64;
pub type AgentId = [u8; 16];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub delegate: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    pub id: CapabilityId,
    pub owner: AgentId,
    /// Permissions atténuables (⊆ permissions du parent).
    pub permissions: Permissions,
    /// Portée : identifiant de ressource (path, service, etc.).
    pub resource: String,
    /// Capability parent (None = racine, octroyée par le runtime).
    pub parent: Option<CapabilityId>,
    /// Wall clock (ms depuis UNIX_EPOCH) au moment du grant/delegate.
    /// ADR-0007 : utilisé pour révoquer les caps émises après un snapshot rollbacké.
    /// Même horloge que `SnapshotHeader.ts_us` (cohérence cross-store).
    pub issued_at_ms: u64,
}

#[derive(Default)]
pub struct CapabilityStore {
    caps: HashMap<CapabilityId, Capability>,
    /// Arbre de dérivation : parent_id → set d'enfants.
    children: HashMap<CapabilityId, HashSet<CapabilityId>>,
    next_id: CapabilityId,
}

impl CapabilityStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Octroie une capability racine (appelé par le runtime lors du spawn d'un agent).
    pub fn grant_root(&mut self, owner: AgentId, permissions: Permissions, resource: String) -> CapabilityId {
        let id = self.alloc_id();
        let issued_at_ms = now_ms();
        self.caps.insert(id, Capability {
            id,
            owner,
            permissions,
            resource,
            parent: None,
            issued_at_ms,
        });
        id
    }

    /// Délègue une capability existante à un autre agent avec atténuation optionnelle.
    /// Échoue si la capability n'existe pas, si l'agent n'en est pas propriétaire,
    /// ou si les permissions déléguées excèdent celles de la source.
    pub fn delegate(
        &mut self,
        source_id: CapabilityId,
        from: &AgentId,
        to: AgentId,
        permissions: Permissions,
        resource: String,
    ) -> Result<CapabilityId, CapError> {
        let source = self.caps.get(&source_id).ok_or(CapError::NotFound(source_id))?.clone();

        if &source.owner != from {
            return Err(CapError::NotOwner);
        }
        if !source.permissions.delegate {
            return Err(CapError::DelegationNotAllowed);
        }
        // Atténuation : les permissions déléguées ne peuvent pas dépasser celles de la source.
        if (permissions.read && !source.permissions.read)
            || (permissions.write && !source.permissions.write)
            || (permissions.execute && !source.permissions.execute)
            || (permissions.delegate && !source.permissions.delegate)
        {
            return Err(CapError::PermissionEscalation);
        }

        let child_id = self.alloc_id();
        let issued_at_ms = now_ms();
        let child = Capability {
            id: child_id,
            owner: to,
            permissions,
            resource,
            parent: Some(source_id),
            issued_at_ms,
        };
        self.caps.insert(child_id, child);
        self.children.entry(source_id).or_default().insert(child_id);
        Ok(child_id)
    }

    /// ADR-0007 / D8 — Révoque toutes les capabilities détenues par `owner`
    /// et émises strictement après `after_ts_ms`. Utilisé par le runtime lors
    /// d'un rollback per-agent : les caps octroyées après le snapshot cible
    /// n'ont plus de référent valide dans l'état restauré.
    ///
    /// Sémantique :
    ///   - Filtre : `cap.owner == owner` ET `cap.issued_at_ms > after_ts_ms` (strict).
    ///   - Ne cascade PAS aux descendants (un cap délégué à un autre agent
    ///     reste valide ; sa révocation est traitée séparément si le superviseur
    ///     choisit d'étendre le rollback).
    ///   - Retourne le nombre de caps effectivement supprimées.
    ///
    /// Complexité : O(N) sur le nombre total de caps du store (scan + collect).
    /// Suffisant pour phase 2 PoC (plafond H-revoke ~10K caps).
    pub fn revoke_owned_after(&mut self, owner: &AgentId, after_ts_ms: u64) -> usize {
        // Pré-collecte (id, parent) avant mutation pour pouvoir nettoyer l'arbre de dérivation.
        let victims: Vec<(CapabilityId, Option<CapabilityId>)> = self.caps
            .iter()
            .filter(|(_, c)| &c.owner == owner && c.issued_at_ms > after_ts_ms)
            .map(|(id, c)| (*id, c.parent))
            .collect();

        let mut count = 0;
        for (id, parent) in victims {
            if let Some(parent_id) = parent {
                if let Some(siblings) = self.children.get_mut(&parent_id) {
                    siblings.remove(&id);
                }
            }
            count += self.revoke(id);
        }
        count
    }

    /// Révoque une capability et toutes ses dérivées (récursif).
    /// Retourne le nombre de capabilities révoquées.
    pub fn revoke(&mut self, id: CapabilityId) -> usize {
        let mut to_revoke = vec![id];
        let mut count = 0;

        while let Some(current) = to_revoke.pop() {
            if self.caps.remove(&current).is_some() {
                count += 1;
            }
            if let Some(children) = self.children.remove(&current) {
                to_revoke.extend(children);
            }
        }

        count
    }

    /// Vérifie si `agent` peut effectuer `perm` sur `resource` avec la capability `cap_id`.
    /// Hot path P4 — O(1) : un seul lookup HashMap.
    ///
    /// P3 (SEF-3) : la portée est vérifiée via `scope_covers` (préfixe de path).
    /// Une cap sur `store/agent-A` couvre `store/agent-A/x` mais pas `store/agent-B/x`.
    pub fn check(&self, agent: &AgentId, cap_id: CapabilityId, resource: &str, perm: &Permissions) -> bool {
        match self.caps.get(&cap_id) {
            None => false,
            Some(cap) => {
                &cap.owner == agent
                    && scope_covers(&cap.resource, resource)
                    && (!perm.read    || cap.permissions.read)
                    && (!perm.write   || cap.permissions.write)
                    && (!perm.execute || cap.permissions.execute)
                    && (!perm.delegate || cap.permissions.delegate)
            }
        }
    }

    /// Retourne la ressource couverte par la capability (pour les tests et le débogage).
    pub fn resource(&self, cap_id: CapabilityId) -> Option<&str> {
        self.caps.get(&cap_id).map(|c| c.resource.as_str())
    }

    pub fn get(&self, id: CapabilityId) -> Option<&Capability> {
        self.caps.get(&id)
    }

    pub fn count(&self) -> usize {
        self.caps.len()
    }

    /// Construit un arbre synthétique de capabilities pour les benchmarks.
    ///
    /// Produit `sum(branching^i, i=0..=depth)` nœuds.
    /// Exemples : branching=10, depth=3 → ~1 111 ; depth=4 → ~11 111 ; depth=5 → ~111 111.
    ///
    /// Retourne (root_id, échantillon de cap_ids répartis dans l'arbre).
    pub fn populate_tree(
        &mut self,
        depth: u32,
        branching: usize,
    ) -> (CapabilityId, Vec<CapabilityId>) {
        let owner = [0xAAu8; 16];
        let perm = Permissions { read: true, write: true, execute: true, delegate: true };
        let root = self.grant_root(owner, perm.clone(), "/res".to_string());

        let mut samples = vec![root];
        let mut current_level = vec![root];

        for _ in 0..depth {
            let mut next_level = Vec::with_capacity(current_level.len() * branching);
            for &parent_id in &current_level {
                for _ in 0..branching {
                    let child = self
                        .delegate(parent_id, &owner, owner, perm.clone(), "/res".to_string())
                        .expect("delegation doit réussir dans populate_tree");
                    next_level.push(child);
                    samples.push(child);
                }
            }
            current_level = next_level;
        }

        (root, samples)
    }

    fn alloc_id(&mut self) -> CapabilityId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// P3 (SEF-3) — Sémantique préfixe de portée.
///
/// `scope_covers(cap, req)` retourne `true` si :
///   - `req == cap` (exact match), OU
///   - `req.starts_with(cap + "/")` (préfixe de path)
///
/// Exemples :
///   - `scope_covers("store/A", "store/A")` → true (exact)
///   - `scope_covers("store/A", "store/A/x")` → true (préfixe)
///   - `scope_covers("store/A", "store/AB")` → false (pas un sous-path)
///   - `scope_covers("store/A", "store/B/x")` → false (chemin différent)
pub fn scope_covers(cap: &str, req: &str) -> bool {
    req == cap || req.starts_with(&format!("{}/", cap))
}

/// Wall clock en millisecondes depuis UNIX_EPOCH.
/// Même horloge que `SnapshotHeader.ts_us / 1000` (ContentStore).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, thiserror::Error)]
pub enum CapError {
    #[error("capability {0} introuvable")]
    NotFound(CapabilityId),
    #[error("l'agent n'est pas propriétaire de cette capability")]
    NotOwner,
    #[error("la délégation n'est pas autorisée sur cette capability")]
    DelegationNotAllowed,
    #[error("les permissions déléguées excèdent celles de la source")]
    PermissionEscalation,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P3 — scope_covers : exact match retourne true.
    #[test]
    fn scope_covers_exact_match() {
        assert!(scope_covers("store/agent-A", "store/agent-A"));
        assert!(scope_covers("/res/A", "/res/A"));
        assert!(scope_covers("r", "r"));
    }

    /// P3 — scope_covers : préfixe de path retourne true.
    #[test]
    fn scope_covers_prefix_match() {
        assert!(scope_covers("store/agent-A", "store/agent-A/x"));
        assert!(scope_covers("store/agent-A", "store/agent-A/tâche-X/fichier.txt"));
        assert!(scope_covers("/res", "/res/subpath"));
    }

    /// P3 — scope_covers : chemin différent retourne false.
    #[test]
    fn scope_covers_different_path() {
        assert!(!scope_covers("store/agent-A", "store/agent-B/x"));
        assert!(!scope_covers("store/agent-A", "store/AB"));   // pas un sous-path
        assert!(!scope_covers("store/agent-A", "store/agent-AA/x")); // pas un préfixe exact
        assert!(!scope_covers("store/agent-A", "other/agent-A"));
    }

    /// P3 — check() utilise scope_covers : une cap sur "store/A" couvre "store/A/x".
    #[test]
    fn check_uses_scope_covers() {
        let mut cs = CapabilityStore::new();
        let owner = [0x01u8; 16];
        let perm_rw = Permissions { read: true, write: true, execute: false, delegate: false };
        // Cap sur le préfixe "store/A"
        let cap_id = cs.grant_root(owner, perm_rw.clone(), "store/A".to_string());

        // Exact match
        assert!(cs.check(&owner, cap_id, "store/A", &perm_rw), "exact match doit passer");
        // Sous-path
        assert!(cs.check(&owner, cap_id, "store/A/x", &perm_rw), "sous-path doit passer");
        // Autre chemin
        assert!(!cs.check(&owner, cap_id, "store/B", &perm_rw), "chemin différent doit échouer");
        assert!(!cs.check(&owner, cap_id, "store/AB", &perm_rw), "préfixe partial doit échouer");
    }
}
