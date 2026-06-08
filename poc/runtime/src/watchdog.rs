// Watchdog calibration (ADR-0025) — constantes par profil AgentProfile.
//
// Le watchdog Wasmtime repose sur epoch_interruption :
//   - Un thread background incrémente l'epoch toutes les EPOCH_TICK_MS_BASE ms.
//   - Avant chaque process_one, run_loop réarme la deadline à `profile.max_ticks()`.
//   - Si WASM dépasse cette deadline, une trappe est levée (pas de retour normal).
//
// ADR-0025 change EPOCH_TICK_MS_BASE de 100 ms (Phase 2) à 10 ms pour permettre
// aux profils Algo (100 ms) et LlmShort (5 s) d'être distingués sans créer un plafond
// minimum de 100 ms pour tous les agents.
//
// Compatibilité : LlmShort était le seul profil avant ADR-0025 (EPOCH_TICK_MS=100,
// MAX_PROCESS_ONE_TICKS=50 → 5 s). Avec ADR-0025, LlmShort garde 5 s mais via
// EPOCH_TICK_MS_BASE=10 et max_ticks=500 (10×500=5000 ms ≡ 5 s identique).

/// Durée d'un tick epoch en millisecondes (ADR-0025).
/// Thread background appelle engine.increment_epoch() toutes les EPOCH_TICK_MS_BASE ms.
pub const EPOCH_TICK_MS_BASE: u64 = 10;

/// Constantes MAX_TICKS par profil (cohérent avec AgentProfile::max_ticks()).
/// Données ici pour référence dans le runtime sans dépendre de agent-sdk.
pub mod profile_ticks {
    /// Algo : boucles déterministes, réponse attendue < 100 ms.
    pub const ALGO: u64 = 10;
    /// LlmShort : inférence standard ≤ 5 s (défaut, rétro-compatible).
    pub const LLM_SHORT: u64 = 500;
    /// LlmLong : inférence longue ≤ 30 s.
    pub const LLM_LONG: u64 = 3_000;
    /// Batch : traitement par lots ≤ 5 min.
    pub const BATCH: u64 = 30_000;
}

/// Défaut utilisé pour les acteurs sans profil explicite = LlmShort (5 s).
/// Doit être identique à l'ancien MAX_PROCESS_ONE_TICKS × EPOCH_TICK_MS = 50 × 100 ms = 5 s.
pub const DEFAULT_MAX_TICKS: u64 = profile_ticks::LLM_SHORT;

/// Profil watchdog d'un agent WASM (ADR-0025) — côté runtime.
///
/// Valeurs identiques à `agent_sdk::AgentProfile` pour cohérence dans le log causal.
/// Un agent WASM déclare son profil via la constante AGENT_PROFILE dans son binaire ou
/// via le paramètre `profile` passé à `ActorInstance::new_precompiled_with_profile`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentProfile {
    /// Agent déterministe pur — boucle courte, réponse < 100 ms attendue.
    Algo      = 0x01,
    /// Agent LLM avec inférence courte (défaut, rétro-compatible avec agents existants).
    #[default]
    LlmShort  = 0x02,
    /// Agent LLM avec inférence longue (jusqu'à ~30 s).
    LlmLong   = 0x03,
    /// Agent de traitement par lots — peut prendre jusqu'à ~5 min.
    Batch     = 0x04,
}

impl AgentProfile {
    /// Retourne le nombre maximum de ticks epoch pour ce profil.
    pub fn max_ticks(self) -> u64 {
        match self {
            AgentProfile::Algo     => profile_ticks::ALGO,
            AgentProfile::LlmShort => profile_ticks::LLM_SHORT,
            AgentProfile::LlmLong  => profile_ticks::LLM_LONG,
            AgentProfile::Batch    => profile_ticks::BATCH,
        }
    }

    /// Construit depuis le discriminant u8.
    /// Retourne `AgentProfile::LlmShort` si inconnu (compatibilité ascendante).
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => AgentProfile::Algo,
            0x02 => AgentProfile::LlmShort,
            0x03 => AgentProfile::LlmLong,
            0x04 => AgentProfile::Batch,
            _    => AgentProfile::LlmShort,
        }
    }
}
