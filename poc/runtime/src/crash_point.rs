// CrashPoint — point d'injection de panne (failpoint) feature-gated.
//
// ADR-0024 : tester l'atomicité du journal de compensation (0x11 / 0x12).
// SEF-4 (TODO Axe 3) : tester l'atomicité crash de la commit barrier (ADR-0027 régime SIGKILL).
//
// Règle de sécurité : ce code ne doit JAMAIS être présent dans le binaire release
// (sans `--features crash-injection`). CI valide sans le feature.
//
// Usage dans les tests / binaires SEF-4 :
//   cargo build --features crash-injection --bin sef4-victim
//
// Le process::exit(1) simule un crash OS (comme SIGKILL / OOM) qui ne laisse pas
// au processus le temps de finaliser ses écritures applicatives. Sous ce régime,
// le page cache OS (et donc le WAL RocksDB OS-buffered) survit — c'est exactement
// le modèle de menace SEF-4 (ADR-0027 D3, régime SIGKILL/panic).

/// Points d'injection disponibles dans les chemins critiques runtime.
///
/// ADR-0024 (rollback scheduler) : `AfterCancel`, `AfterRollbackApplied`.
/// SEF-4 (commit barrier — ADR-0027 régime SIGKILL) : les 4 variantes `CommitBarrier*`.
#[cfg(feature = "crash-injection")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrashPoint {
    // ── ADR-0024 — Journal de compensation rollback scheduler ───────────────
    /// Entre l'émission 0x0E (InferenceCancelled) et l'envoi de Message::Rollback.
    AfterCancel = 1,
    /// Après l'application du snapshot (Message::Rollback traité par l'agent),
    /// mais avant l'émission de CompensationClose (0x12).
    AfterRollbackApplied = 2,

    // ── SEF-4 — commit barrier (host fn `commit_barrier` + `emit`) ──────────
    /// Dans la host fn `commit_barrier`, avant l'appel `ContentStore::put_block`.
    /// Effet : aucune écriture ContentStore ni log pour l'action en cours.
    /// État attendu post-recovery : `last_snapshot = hash_ref_pre[k]`.
    CommitBarrierPrePutBlock = 10,
    /// Dans la host fn `commit_barrier`, entre `put_block` et `put_snapshot`.
    /// Effet : block orphelin écrit, mais pas de SnapshotHeader pour le référencer.
    /// État attendu post-recovery : `last_snapshot = hash_ref_pre[k]` (l'orphelin
    /// existe mais n'est pas dans la chaîne).
    CommitBarrierBetweenPutBlockAndPutSnapshot = 11,
    /// Dans la host fn `commit_barrier`, après `put_snapshot` mais avant le retour
    /// vers WASM (donc avant que `emit` puisse appeler `CausalLog::append`).
    /// Effet : ContentStore avancé à `hash_ref_pre[k+1]`, log encore à `hash_ref_pre[k]`.
    /// État attendu post-recovery via log : `hash_ref_pre[k]`. Via ContentStore (scan
    /// par parent depuis le tip orphelin) : `hash_ref_pre[k+1]`. Asymétrie
    /// documentée ADR-0027 §Coût (alternative A2).
    CommitBarrierPostPutSnapshotPreLogAppend = 12,
    /// Dans la host fn `emit`, après `CausalLog::append` réussi.
    /// Effet : toute la transaction de l'action `k` est durable (ContentStore +
    /// log alignés). État attendu post-recovery : `hash_ref_pre[k+1]`.
    CommitBarrierPostLogAppend = 13,
}

#[cfg(feature = "crash-injection")]
impl CrashPoint {
    pub fn as_u8(self) -> u8 { self as u8 }

    /// Parse depuis un nom CLI (`--kill-at <nom>`). Retourne None si inconnu.
    pub fn from_cli(name: &str) -> Option<Self> {
        match name {
            "after_cancel" => Some(Self::AfterCancel),
            "after_rollback_applied" => Some(Self::AfterRollbackApplied),
            "pre_put_block" => Some(Self::CommitBarrierPrePutBlock),
            "between_put_block_and_put_snapshot" => {
                Some(Self::CommitBarrierBetweenPutBlockAndPutSnapshot)
            }
            "post_put_snapshot_pre_log_append" => {
                Some(Self::CommitBarrierPostPutSnapshotPreLogAppend)
            }
            "post_log_append" => Some(Self::CommitBarrierPostLogAppend),
            _ => None,
        }
    }
}

/// Configuration globale du kill — armée avant `process_one`, désarmée si l'action
/// cible est dépassée. Conçue pour le binaire `sef4-victim` qui exécute une seule
/// séquence déterministe puis tue le processus à un point précis.
#[cfg(feature = "crash-injection")]
pub mod armed {
    use super::CrashPoint;
    use std::sync::atomic::{AtomicU8, Ordering};

    /// 0 = désarmé ; sinon `CrashPoint as u8`. AtomicU8 plutôt que Mutex parce que
    /// `fire()` est appelé depuis les host functions Wasmtime qui ne peuvent pas
    /// acquérir de locks (FnMut sans Send sur certaines branches `func_wrap`).
    static ARMED: AtomicU8 = AtomicU8::new(0);

    /// Arme un point d'injection. À appeler une seule fois après initialisation.
    pub fn arm(cp: CrashPoint) {
        ARMED.store(cp.as_u8(), Ordering::SeqCst);
    }

    /// Désarme (utilisé par les tests sans kill).
    pub fn disarm() {
        ARMED.store(0, Ordering::SeqCst);
    }

    /// Retourne le point armé courant, ou None si rien n'est armé.
    pub fn current() -> Option<CrashPoint> {
        match ARMED.load(Ordering::SeqCst) {
            0 => None,
            1 => Some(CrashPoint::AfterCancel),
            2 => Some(CrashPoint::AfterRollbackApplied),
            10 => Some(CrashPoint::CommitBarrierPrePutBlock),
            11 => Some(CrashPoint::CommitBarrierBetweenPutBlockAndPutSnapshot),
            12 => Some(CrashPoint::CommitBarrierPostPutSnapshotPreLogAppend),
            13 => Some(CrashPoint::CommitBarrierPostLogAppend),
            _ => None,
        }
    }
}

/// Déclenche le point de panne si la feature est activée ET le point armé.
///
/// Sémantique : appelle `std::process::exit(1)` (équivalent SIGKILL côté processus —
/// pas de destructeur, pas de flush utilisateur, page cache OS préservé).
///
/// En production (`not(crash-injection)`) : no-op absolu — le compilateur élide l'appel.
#[cfg(feature = "crash-injection")]
pub fn fire(cp: CrashPoint) {
    if armed::current() == Some(cp) {
        tracing::warn!("CrashPoint::fire({:?}) — exit(1) (simulation SIGKILL)", cp);
        // Note : on ne flushe RIEN volontairement. Le WAL OS-buffered (page cache) survit
        // (ADR-0027 D3). C'est l'invariant testé par SEF-4.
        std::process::exit(1);
    }
}

/// Version no-op pour la production (feature non activée).
///
/// Le paramètre `()` garantit que l'appel est syntaxiquement valide dans les deux cas.
#[cfg(not(feature = "crash-injection"))]
#[inline(always)]
pub fn fire(_cp: ()) {}
