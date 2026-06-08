// Primitive horloge substituable — exigence S6 (spec/02b-substrate_requirements.md).
//
// Toutes les host functions qui produisent un timestamp inséré dans une structure
// hashée (SnapshotHeader, LogEntry, EmitEnvelope) doivent passer par cette primitive.
// Le timestamp est inclus dans :
//   - SnapshotHeader.ts_us (→ snapshot_id, → last_snapshot, → hash_before/hash_after)
//   - LogEntry.ts_ms       (→ action_id)
//   - EmitEnvelope.ts_us   (→ payload bincode → LogEntry.emit_payload → action_id)
//
// Sans substitution, deux exécutions identiques produisent des chaînes différentes
// et SEF-6 (P5 — déterminisme de transition d'état) est non-vérifiable.
//
// Coût (P5 §Coût connu) : structurel — interface plus stricte, pas de coût runtime
// notable sur le chemin chaud (un appel virtuel via Arc<dyn Clock> par commit_barrier).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Source de timestamp substituable.
///
/// `Send + Sync` requis : `AgentState` doit être `Send` (Tokio task) ; `AgentState`
/// peut être partagé avec des host functions async (`func_wrap_async`) qui clonent
/// l'`Arc<dyn Clock>`.
pub trait Clock: Send + Sync {
    /// Timestamp en millisecondes (epoch Unix ou compteur logique selon implémentation).
    fn now_ms(&self) -> u64;
    /// Timestamp en microsecondes. Implémentation par défaut : `now_ms() * 1000`.
    /// Override pour les horloges qui distinguent les deux résolutions (ex. SystemClock).
    fn now_us(&self) -> u64 { self.now_ms() * 1_000 }
}

/// Horloge réelle adossée à `SystemTime::now()`. Mode production.
///
/// **Non-déterministe par construction** — deux appels successifs produisent
/// des valeurs différentes ; deux runs produisent des séries de valeurs différentes.
/// Utiliser uniquement quand le déterminisme de transition (P5) n'est pas requis.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    fn now_us(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }
}

/// Horloge logique monotone : compteur incrémenté à chaque appel.
///
/// Garantit l'égalité bit-à-bit des timestamps entre deux runs identiques tant que
/// la séquence d'appels est identique. Conçue pour SEF-6 (P5 — déterminisme de
/// transition d'état) et plus largement pour tout test de reproductibilité.
///
/// **Sémantique d'incrément :** chaque appel à `now_ms()` ou `now_us()` retourne
/// la valeur courante et incrémente le compteur de 1. La séparation ms/us est
/// transparente : un seul compteur sous-jacent, exposé identiquement aux deux
/// méthodes (l'unité est purement nominale en mode replay — la structure hashée
/// ne distingue pas).
///
/// **Hypothèse d'usage :** tous les call-sites `clock.now_*()` du runtime sont
/// déterministes une fois la séquence d'inputs fixée (S5 — séquentialité par
/// acteur). Si un site dépend d'un timing wall-clock externe (ex. timeout
/// Tokio), il sort du périmètre de la primitive horloge et de SEF-6.
pub struct LogicalClock {
    counter: AtomicU64,
}

impl LogicalClock {
    /// Démarre à `start` (inclus dans le premier appel).
    pub fn new(start: u64) -> Self {
        Self { counter: AtomicU64::new(start) }
    }

    /// Valeur courante sans incrément (diagnostic).
    pub fn peek(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }
}

impl Default for LogicalClock {
    fn default() -> Self {
        Self::new(1_700_000_000_000) // ~2023-11-15, valeur arbitraire stable
    }
}

impl Clock for LogicalClock {
    fn now_ms(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::SeqCst)
    }
    fn now_us(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Helper : `Arc<SystemClock>` pour les call-sites historiques qui n'ont pas
/// été refactorisés pour accepter une `Clock` explicite.
pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_clock_is_deterministic() {
        let c1 = LogicalClock::new(100);
        let c2 = LogicalClock::new(100);
        let s1: Vec<u64> = (0..10).map(|_| c1.now_ms()).collect();
        let s2: Vec<u64> = (0..10).map(|_| c2.now_ms()).collect();
        assert_eq!(s1, s2);
        assert_eq!(s1, vec![100, 101, 102, 103, 104, 105, 106, 107, 108, 109]);
    }

    #[test]
    fn logical_clock_ms_us_share_counter() {
        let c = LogicalClock::new(10);
        assert_eq!(c.now_ms(), 10);
        assert_eq!(c.now_us(), 11);
        assert_eq!(c.now_ms(), 12);
        assert_eq!(c.peek(), 13);
    }

    #[test]
    fn system_clock_returns_nonzero() {
        let c = SystemClock;
        assert!(c.now_ms() > 1_700_000_000_000);
        assert!(c.now_us() > 1_700_000_000_000_000);
    }
}
