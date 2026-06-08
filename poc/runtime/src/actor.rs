// Actor = instance Wasmtime isolée + état agent.
// S5 (séquentialité) : run_loop traite les messages un par un.
// S4 (effets externes interposés) : `emit` ne peut être appelé qu'après `commit_barrier`.
// H-cb-correct : debug_assert dans `emit` vérifie que `pending_commit.is_some()` (M4 : source
// unique de l'invariant ; l'ancien drapeau `barrier_fired` redondant a été retiré).

use std::sync::{Arc, Mutex};
use wasmtime::{Caller, Engine, Linker, Memory, Module, Store, TypedFunc};
use os_poc_store::{ContentStore, SnapshotHeader, StoreError};
use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType, LogEntry};
use os_poc_capabilities::{CapabilityStore, CapabilityId, Permissions};
use crate::clock::{Clock, SystemClock};
use crate::error::RuntimeError;
use crate::inference::{InferFn, InferError};
use crate::watchdog::AgentProfile;
use sha2::{Sha256, Digest};

pub type AgentId = [u8; 16];

/// C2 (revue sécurité 2026-06-07) — verrou tolérant à l'empoisonnement.
///
/// Les stores d'autorité (`CapabilityStore`, `CauseHandleStore`) sont des `Arc<Mutex<…>>`
/// **partagés** au-delà d'une frontière de tenant (le `CauseHandleStore` via le registre
/// cross-tenant d'ADR-0060 ; le `CapabilityStore` au sein d'un tenant). Avec `.lock().unwrap()`,
/// un panic d'un porteur antérieur empoisonne le Mutex et fait paniquer **tous** les détenteurs
/// suivants — y compris d'autres tenants → DoS cross-tenant, voire `abort()` si le panic survient
/// dans un `Drop` pendant un unwind. Or les mutations de ces stores sont logiquement atomiques
/// (un appel = mint/contains/revoke/check/delegate) : l'état reste cohérent même après un panic
/// d'un porteur. On récupère donc l'état (`into_inner`) au lieu de propager le poison.
#[inline]
pub(crate) fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Identifiant de tenant (ADR-0057, chantier MT-1).
///
/// Un *tenant* est un principal (opérateur) potentiellement non-confiant vis-à-vis des
/// autres. Le `CausalLog` et le `ContentStore` sont **partagés** entre tenants (c'est la
/// condition de la menace que B-fort fermera, ADR-0057 §D1) ; le `CapabilityStore` est
/// **isolé** par tenant (frontière d'autorité, §D2).
///
/// `DEFAULT` est la sentinelle mono-tenant : tout acteur construit sans `.tenant()` explicite
/// l'obtient, garantissant zéro régression du code existant (PoC mono-tenant).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TenantId(pub u64);

impl TenantId {
    /// Tenant par défaut (mono-tenant). Tout le code antérieur à MT-1 l'utilise implicitement.
    pub const DEFAULT: TenantId = TenantId(0);
}

impl Default for TenantId {
    fn default() -> Self {
        TenantId::DEFAULT
    }
}

/// Registre des `CauseHandle` (ADR-0058, B-fort).
///
/// Un `CauseHandle` autorise un agent `grantee` à citer une action `action_id` (émise par
/// un autre agent) comme parent causal via `agent_add_cause`. Sans handle, une citation
/// cross-agent est refusée (`-3`) — c'est ce qui ferme le *confused deputy* d'ADR-0036 §57
/// en multi-tenant (ADR-0057 §D1).
///
/// Modèle (ADR-0058 R1, amendement post-BF-0) : la clé est `(grantee, action_id)` —
/// l'action_id content-addressed EST l'objet capability-désigné (§D1). Pas d'index `u64`
/// exposé au guest (l'ABI de `agent_add_cause` reste `action_id_ptr`), pas de cache local :
/// **ce store partagé-par-tenant est l'unique source de vérité d'autorisation**, consulté à
/// chaque appel cross-agent — ce qui clôt structurellement le « risque n°1 » de cohérence.
///
/// Isolé par tenant comme le `CapabilityStore` (ADR-0057 §D2) : un `Arc<Mutex<…>>` partagé
/// entre les agents d'un même tenant. L'auto-citation (un agent cite ses propres actions,
/// §D10) ne passe pas par ce store : elle est autorisée intrinsèquement dans `agent_add_cause`.
#[derive(Default)]
pub struct CauseHandleStore {
    next_id: u64,
    handles: std::collections::HashMap<(AgentId, [u8; 32]), CauseHandleEntry>,
}

struct CauseHandleEntry {
    /// Agent émetteur du handle (ADR-0058 §D6/D7 — base de la révocation par émetteur).
    issuer: AgentId,
    /// Horloge logique d'émission (même base que `Capability.issued_at_ms` / `SnapshotHeader.ts_us`).
    issued_at_ms: u64,
    /// Identité d'audit du handle (ADR-0058 §D3) ; non exposée au guest. Exploitée en BF-3.
    #[allow(dead_code)]
    id: u64,
}

impl CauseHandleStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Émet un handle autorisant `grantee` à citer `action_id` (émise par `issuer`).
    /// Retourne l'identité d'audit du handle. Appelé par le code trusted (runner/superviseur).
    pub fn mint(&mut self, action_id: [u8; 32], grantee: AgentId, issuer: AgentId, issued_at_ms: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.handles.insert((grantee, action_id), CauseHandleEntry { issuer, issued_at_ms, id });
        id
    }

    /// Vrai si `grantee` détient un handle pour `action_id`.
    pub fn contains(&self, grantee: &AgentId, action_id: &[u8; 32]) -> bool {
        self.handles.contains_key(&(*grantee, *action_id))
    }

    /// BF-2 (ADR-0058 §D6) — révoque tous les handles émis par `issuer`
    /// (à la terminaison de l'agent émetteur).
    pub fn revoke_issued_by(&mut self, issuer: &AgentId) {
        self.handles.retain(|_, e| &e.issuer != issuer);
    }

    /// BF-2 (ADR-0058 §D7) — révoque les handles émis par `issuer` après `after_ms`
    /// (au rollback de l'agent émetteur ; symétrie « émis » et non « détenu », cf. caps).
    pub fn revoke_issued_after(&mut self, issuer: &AgentId, after_ms: u64) {
        self.handles.retain(|_, e| !(&e.issuer == issuer && e.issued_at_ms > after_ms));
    }
}

/// ADR-0060 — Registre des `CauseHandleStore`, indexé par tenant : `TenantId → store`.
///
/// Issu du constat (ADR-0058 §D6/D7) que la révocation BF-2 ne balayait que le store du
/// tenant de l'émetteur — un handle émis par A (tenant T1) **au profit d'un grantee de T2**
/// vit dans le store de **T2** (le grantee consulte SON store, cf. `bf3_handle_in_wrong_tenant_store_useless`),
/// donc survivait à la terminaison/rollback de A. Ce registre rend **tous** les stores visibles
/// à un point unique, pour que la révocation cross-tenant (XR-1) les balaie tous.
///
/// **Source de vérité unique (risque n°1, ADR-0060/ADR-0058).** Le store local que chaque
/// agent consulte dans `agent_add_cause` **doit** être obtenu via [`get_or_create`](Self::get_or_create) —
/// jamais construit indépendamment. Sinon `agent_add_cause` (store local) et le balayage de
/// révocation (registre) verraient deux états divergents : exactement le bug cache↔store que
/// B-fort prétend fermer. `get_or_create` est l'unique point d'insertion.
pub struct CauseHandleRegistry {
    stores: std::sync::RwLock<std::collections::HashMap<TenantId, Arc<Mutex<CauseHandleStore>>>>,
}

impl CauseHandleRegistry {
    pub fn new() -> Self {
        Self { stores: std::sync::RwLock::new(std::collections::HashMap::new()) }
    }

    /// Store du tenant, créé à la volée si absent. **Unique point d'insertion** (risque n°1).
    /// C2 : RwLock tolérant au poison (`into_inner`) — un panic d'un porteur ne doit pas bloquer
    /// la construction d'agents de TOUS les tenants.
    pub fn get_or_create(&self, tenant: TenantId) -> Arc<Mutex<CauseHandleStore>> {
        if let Some(s) = self.stores.read().unwrap_or_else(|e| e.into_inner()).get(&tenant) {
            return Arc::clone(s);
        }
        let mut w = self.stores.write().unwrap_or_else(|e| e.into_inner());
        Arc::clone(w.entry(tenant).or_insert_with(|| Arc::new(Mutex::new(CauseHandleStore::new()))))
    }

    /// Store du tenant s'il existe (lecture seule, sans création).
    pub fn get(&self, tenant: TenantId) -> Option<Arc<Mutex<CauseHandleStore>>> {
        self.stores.read().unwrap_or_else(|e| e.into_inner()).get(&tenant).map(Arc::clone)
    }

    /// XR-1 (ADR-0060 §D6) — révoque, dans **tous** les stores du registre, les handles émis
    /// par `issuer` (terminaison de l'émetteur, balayage cross-tenant). O(tenants × handles).
    /// C2 : locks tolérants au poison — un store empoisonné par un tenant ne fait pas paniquer
    /// la révocation des autres (ni `abort()` si appelé depuis le Drop guard pendant unwind).
    pub fn revoke_issued_by_all(&self, issuer: &AgentId) {
        for store in self.stores.read().unwrap_or_else(|e| e.into_inner()).values() {
            lock_or_recover(store).revoke_issued_by(issuer);
        }
    }

    /// XR-1 (ADR-0060 §D7) — révoque, dans **tous** les stores du registre, les handles émis
    /// par `issuer` après `after_ms` (rollback de l'émetteur, balayage cross-tenant).
    pub fn revoke_issued_after_all(&self, issuer: &AgentId, after_ms: u64) {
        for store in self.stores.read().unwrap_or_else(|e| e.into_inner()).values() {
            lock_or_recover(store).revoke_issued_after(issuer, after_ms);
        }
    }
}

impl Default for CauseHandleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// États du cycle de vie d'un acteur (A4 — 02c-primitives-agent.md §A4).
/// Chaque transition est enregistrée dans le log causal (EmitType::Lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LifecycleState {
    Spawned           = 0,
    Active            = 1,
    Suspended         = 2,  // A4 : suspension externe (superviseur)
    Checkpointed      = 3,
    Terminated        = 4,
    AwaitingValidation = 5, // A3 : en attente d'un verdict de validation
    WaitingInference   = 6, // ADR-0019 : bloqué dans agent_infer, thread Tokio libéré
}

impl LifecycleState {
    pub fn as_u8(self) -> u8 { self as u8 }
}

/// Verdict retourné par le superviseur sur une demande de validation A3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ValidationVerdict {
    Approved  = 0,
    Rejected  = 1,
    Timeout   = 2,
    /// Demande annulée par un `Message::Rollback` reçu pendant `AwaitingValidation`.
    /// L'agent redevient `Active` et le Rollback est traité normalement (ADR-0014).
    Cancelled = 3,
}

/// ADR-0015 D15.2 — Cause d'un événement `EmitType::AgentCrash` (0x13).
/// Encodé comme premier octet (u8) du payload AgentCrash.
/// `HostPanic = 0x04` est défini mais hors scope de l'implémentation D15.2
/// (requiert `catch_unwind` côté run_loop — décision séparée).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrashCause {
    /// `process_one` (ou `SessionResume::process_one`) a renvoyé `Err`.
    ProcessFailed       = 0x01,
    /// `rollback_path` a renvoyé `Err` — chaîne ContentStore brisée.
    ContentStoreBroken  = 0x02,
    /// Trap epoch_interruption (ADR-0025) — boucle infinie ou cycle dépassant
    /// `profile.max_ticks()` ticks. Diagnostiqué via `wasmtime::Trap::Interrupt`.
    WatchdogTrap        = 0x03,
    /// Host panic capturé par `catch_unwind` — hors scope D15.2.
    HostPanic           = 0x04,
}

impl CrashCause {
    pub fn as_u8(self) -> u8 { self as u8 }
}

/// Longueur fixe du payload `EmitType::AgentCrash` (0x13) : 49 octets.
///   [0]      cause u8
///   [1..17]  parent_agent_id 16B ([0u8;16] si racine)
///   [17..49] last_action_id 32B ([0u8;32] si aucune action)
pub const AGENT_CRASH_PAYLOAD_LEN: usize = 1 + 16 + 32;

pub enum Message {
    /// Données à traiter par l'agent.
    /// `cause` : action_id ([u8;32]) de l'action déclencheuse (cross-agent, ADR-0003).
    /// Si Some, injectée dans `pending_extra_causes` avant `process_one` — le `commit_barrier`
    /// de ce cycle inclura alors la cause dans les `parent_ids` du LogEntry résultant.
    Data { payload: Vec<u8>, cause: Option<[u8; 32]> },
    Suspend,
    Checkpoint,
    Rollback { target_seq: u64 },
    /// A3 : réponse du superviseur à une demande de validation en cours.
    ValidationResponse { verdict: ValidationVerdict },
    /// ADR-0012 : reprise d'une nouvelle session avec injection du résumé causal.
    /// Le payload est délivré comme premier Data de la session.
    SessionResume { summary: Vec<u8> },
    /// Eviction : l'agent se met au propre (Suspended dans le log), renvoie son état
    /// minimal via le canal reply, et se termine. Le scheduler stocke l'état dans sa
    /// table dormant pour un wake ultérieur.
    Evict { reply: tokio::sync::oneshot::Sender<EvictedState> },
}

/// État minimal d'un agent évincé de la mémoire.
///
/// Contient les champs de `AgentState` nécessaires pour reconstituer un
/// `ActorInstance` fonctionnel depuis le ContentStore (ADR-0030 §FutureWork).
#[derive(Debug, Clone)]
pub struct EvictedState {
    pub id:            AgentId,
    pub seq:           u64,
    pub last_snapshot: Option<[u8; 32]>,
    pub last_action:   Option<[u8; 32]>,
    /// Timestamp d'éviction capturé par `Scheduler::evict_agent` (ADR-0031 §D4).
    /// Utilisé pour calculer le `cache_score` lors du réveil via `IoAdmissionQueue::acquire` :
    /// une éviction récente → état probablement encore dans le block cache RocksDB.
    pub evicted_at:    std::time::Instant,
}

impl Message {
    /// Constructeur pratique pour un message sans cause explicite.
    pub fn data(payload: Vec<u8>) -> Self {
        Self::Data { payload, cause: None }
    }
    /// Constructeur pratique pour un message avec cause cross-agent.
    pub fn caused(payload: Vec<u8>, cause: [u8; 32]) -> Self {
        Self::Data { payload, cause: Some(cause) }
    }
}

/// Taille fixe du payload retourné par `agent_introspect` (A1 — 02c-primitives-agent.md).
///
/// Layout (tous les champs en little-endian) :
///   [  0.. 32] last_action_id  : [u8; 32]  — zeros si absent
///   [ 32.. 40] seq             : u64 LE
///   [ 40.. 72] last_snapshot   : [u8; 32]  — zeros si absent
///   [      72] flags           : u8 — bit 0 = action set, bit 1 = snapshot set
///   [      73] lifecycle_state : u8 — LifecycleState discriminant
pub const INTROSPECT_PAYLOAD_LEN: usize = 74;

/// A2 (02c-primitives-agent.md §A2) : profondeur maximale autorisée pour un self-rollback.
pub const MAX_SELF_ROLLBACK_DEPTH: usize = 3;

/// ADR-0012 : nombre maximal d'actions (commit_barriers) par session.
pub const SESSION_DEFAULT_MAX_ACTIONS: u64 = 10_000;
/// ADR-0012 : durée maximale d'une session en millisecondes (24h).
/// Configurable par agent via `new_precompiled_with_caps_timeout_and_session`.
/// Convention : `session_max_duration_ms == 0` désactive la borne durée
/// (seule `session_max_actions` joue alors). Utile pour les tests qui isolent
/// la borne actions, et pour les agents long-courrier dont la durée n'est
/// pas le critère pertinent.
pub const SESSION_DEFAULT_MAX_DURATION_MS: u64 = 86_400_000;

/// ADR-0014 D14.a : durée par défaut au-delà de laquelle un agent en `AwaitingValidation`
/// reçoit automatiquement un `ValidationVerdict::Timeout`. Valeur provisoire et non-mesurée
/// (30 s) — à réviser dès qu'une mesure réelle de latence de réponse superviseur est
/// disponible (Phase 3). Configurable par agent via `new_precompiled_with_caps_and_timeout`.
pub const SESSION_DEFAULT_VALIDATION_TIMEOUT_MS: u64 = 30_000;

/// ADR-0019 §Q4 : timeout hôte maximal pour agent_infer, non négociable par l'agent.
/// `timeout_ms_effective = min(timeout_ms_requested, HOST_MAX_INFERENCE_DURATION_MS)`.
/// 180 s : calibré pour inférence CPU (Ryzen 5 PRO, mistral:7b) avec historique multi-tour.
/// Sur GPU cible (spec/07 §2, 24 GB), la valeur peut être réduite à 30–60 s.
pub const HOST_MAX_INFERENCE_DURATION_MS: u32 = 180_000;

// S6 : tout call-site qui produit un timestamp inséré dans SnapshotHeader, LogEntry
// ou EmitEnvelope doit passer par `state.clock.now_ms()` / `state.clock.now_us()`.
// L'horloge est substituable (SystemClock en prod, LogicalClock pour SEF-6).
// Voir `crate::clock` et ADR-0028.

// Module WASM minimal : commit_barrier avant tout emit (S4 / H-cb-correct).
// ADR-0010 : emit prend (emit_type i32, ptr i32, len i32) — type 1 = ActionResult.
pub const AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier" (func $cb))
  (import "env" "emit"           (func $emit (param i32 i32 i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    call $cb
    i32.const 1
    local.get $ptr
    local.get $len
    call $emit
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM A1 — teste `agent_introspect`.
/// Appelle introspect à l'offset 256, puis emit le résultat pour le rendre visible
/// dans le log causal (prouve que l'agent a lu son propre état).
pub const INTROSPECT_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier"   (func $cb))
  (import "env" "emit"             (func $emit (param i32 i32 i32)))
  (import "env" "agent_introspect" (func $intro (param i32 i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    ;; A1 : lit l'état courant → écrit INTROSPECT_PAYLOAD_LEN bytes à l'offset 256
    (drop (call $intro (i32.const 256) (i32.const 74)))
    ;; commit_barrier + emit du payload introspect (type 4 = Introspect)
    call $cb
    i32.const 6
    i32.const 256
    i32.const 74
    call $emit
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM A2 — teste `agent_self_rollback`.
/// msg[0]=0 → commit_barrier + emit (construit un snapshot dans l'historique)
/// msg[0]=1 → agent_self_rollback(msg[1]) et abandonne le résultat
pub const SELF_ROLLBACK_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier"      (func $cb))
  (import "env" "emit"                (func $emit (param i32 i32 i32)))
  (import "env" "agent_self_rollback" (func $rollback (param i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 1))
      (then
        (drop (call $rollback (i32.load8_u (i32.add (local.get $ptr) (i32.const 1)))))
      )
    )
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM session (ADR-0012) — teste `agent_session_info`.
/// msg[0]=0 → commit_barrier + emit (build history, incrémente action_count)
/// msg[0]=5 → agent_session_info à l'offset 32, commit_barrier + emit les 24 bytes
pub const SESSION_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier"    (func $cb))
  (import "env" "emit"              (func $emit (param i32 i32 i32)))
  (import "env" "agent_session_info"(func $sess_info (param i32 i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    ;; msg[0]=0 → build history
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    ;; msg[0]=5 → session_info + commit_barrier + emit
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 5))
      (then
        (drop (call $sess_info (i32.const 32) (i32.const 24)))
        call $cb
        i32.const 1
        i32.const 32
        i32.const 24
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM ADR-0003 cross-agent — teste `agent_add_cause`.
/// msg[0]=0          → commit_barrier + emit (build history)
/// msg[0]=4, msg[1..33] = 32 bytes action_id → agent_add_cause(ptr+1) + commit_barrier + emit
/// L'agent_add_cause doit être appelé AVANT commit_barrier pour que la cause soit
/// incluse dans les parent_ids du LogEntry créé par commit_barrier.
pub const CROSS_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier"  (func $cb))
  (import "env" "emit"            (func $emit (param i32 i32 i32)))
  (import "env" "agent_add_cause" (func $add_cause (param i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    ;; msg[0]=0 → build history
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    ;; msg[0]=4 → add_cause(ptr+1) + commit_barrier + emit
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 4))
      (then
        (drop (call $add_cause (i32.add (local.get $ptr) (i32.const 1))))
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM A3 — teste `agent_request_validation` et `agent_get_verdict`.
/// msg[0]=0 → commit_barrier + emit (build history)
/// msg[0]=2, msg[1]=risk → agent_request_validation(risk) — lifecycle passe AwaitingValidation
/// msg[0]=3 → agent_get_verdict() stocké à l'offset 8, commit_barrier + emit
pub const VALIDATION_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier"           (func $cb))
  (import "env" "emit"                     (func $emit (param i32 i32 i32)))
  (import "env" "agent_request_validation" (func $req_val (param i32) (result i32)))
  (import "env" "agent_get_verdict"        (func $get_verdict (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    ;; msg[0]=0 → build history
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    ;; msg[0]=2 → request_validation(msg[1])
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 2))
      (then
        (drop (call $req_val (i32.load8_u (i32.add (local.get $ptr) (i32.const 1)))))
      )
    )
    ;; msg[0]=3 → get_verdict, commit_barrier + emit
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 3))
      (then
        (i32.store8 (i32.const 8) (call $get_verdict))
        call $cb
        i32.const 1
        i32.const 8
        i32.const 1
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
// Module WASM W1 : 800 pages = 50 MB état agent simulé (spec §W1).
// Le start fn touche la première word de chaque page (800 page faults).
// Même interface que AGENT_WAT : commit_barrier avant emit.
pub const W1_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier" (func $cb))
  (import "env" "emit"           (func $emit (param i32 i32 i32)))
  (memory (export "memory") 800)
  (func $init
    (local $p i32)
    (local.set $p (i32.const 0))
    (block $exit
      (loop $again
        (i32.store (local.get $p) (local.get $p))
        (local.set $p (i32.add (local.get $p) (i32.const 65536)))
        (br_if $exit (i32.ge_u (local.get $p) (i32.const 52428800)))
        (br $again)
      )
    )
  )
  (func (export "process") (param $ptr i32) (param $len i32)
    call $cb
    i32.const 1
    local.get $ptr
    local.get $len
    call $emit
  )
  (start $init)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM capabilities — teste `agent_check_cap`.
/// msg[0]=0 → build history (commit+emit)
/// msg[0]=6, msg[1..9]=cap_id i64 LE, msg[9..17]=resource 8 bytes, msg[17]=perm_flags
/// → agent_check_cap, résultat (1/0) écrit à l'offset 64, commit+emit
pub const CAP_CHECK_WAT: &str = r#"(module
  (import "env" "commit_barrier"  (func $cb))
  (import "env" "emit"            (func $emit (param i32 i32 i32)))
  (import "env" "agent_check_cap" (func $check_cap (param i64 i32 i32 i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    ;; msg[0]=0 → build history
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    ;; msg[0]=6 → cap check, result at offset 64, commit+emit
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 6))
      (then
        (i32.store8 (i32.const 64)
          (call $check_cap
            (i64.load (i32.add (local.get $ptr) (i32.const 1)))
            (i32.add (local.get $ptr) (i32.const 9))
            (i32.const 8)
            (i32.load8_u (i32.add (local.get $ptr) (i32.const 17)))
          )
        )
        call $cb
        i32.const 1
        i32.const 64
        i32.const 1
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// T2.4 (D9) — agent en boucle infinie pour tester le watchdog epoch_interruption.
/// process() entre dans une boucle WASM sans jamais retourner ni appeler de host function.
/// Doit être trappé par run_loop sous MAX_PROCESS_ONE_TICKS × EPOCH_TICK_MS ≈ 5s.
pub const INFINITE_LOOP_AGENT_WAT: &str = r#"(module
  (func (export "process") (param i32) (param i32)
    (loop $inf (br $inf)))
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// ADR-0015 D15.2-a — agent qui trap immédiatement via `unreachable`.
/// `process_one` retourne `Err(RuntimeError::Wasmtime)` avec trap
/// `UnreachableCodeReached` (pas `Interrupt`) → classifié `ProcessFailed`.
/// Sert à tester l'émission de `AgentCrash{cause=0x01}` sans dépendre du watchdog.
pub const TRAP_AGENT_WAT: &str = r#"(module
  (func (export "process") (param i32) (param i32)
    unreachable)
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// UC-18 / S30 agent OOB : acces memoire hors-bornes (offset 0x10000 = au-dela 1ere page WASM).
/// Wasmtime leve MemoryOutOfBounds -> ProcessFailed (0x01). Sandbox WASM contient lacces.
pub const OOB_TRAP_AGENT_WAT: &str = r#"(module
  (func (export "process") (param i32) (param i32)
    (drop (i32.load (i32.const 0x10000))))
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM T5 (ADR-0019) — teste la host function `agent_infer`.
/// msg[0]=0 → commit_barrier + emit (build history)
/// msg[0]=7 → agent_infer(msg, len, response_buf=512, cap=1024, len_out=260, timeout=5000)
///            commit_barrier + emit(result_code byte à l'offset 256)
pub const INFER_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier" (func $cb))
  (import "env" "emit"           (func $emit (param i32 i32 i32)))
  (import "env" "agent_infer"    (func $infer (param i32 i32 i32 i32 i32 i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 7))
      (then
        (i32.store8 (i32.const 256)
          (call $infer
            local.get $ptr
            local.get $len
            (i32.const 512)
            (i32.const 1024)
            (i32.const 260)
            (i32.const 5000)
          )
        )
        call $cb
        i32.const 1
        (i32.const 256)
        (i32.const 1)
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

#[cfg(any(test, feature = "test-fixtures"))]
/// Module WASM SEF-3 / P1 — teste les host functions `agent_store_get` et `agent_store_put`.
///
/// msg[0]=0 → commit_barrier + emit (build history)
///
/// msg[0]=8 → agent_store_put(resource=msg[2..2+msg[1]], cap_id=msg[2+msg[1]..+8], val=msg[10+msg[1]..+msg[1]])
///            Résultat stocké à l'offset 256, commit_barrier + emit.
///
/// msg[0]=9 → agent_store_get(resource=msg[2..2+msg[1]], cap_id=msg[2+msg[1]..+8], out=offset 300)
///            Résultat stocké à l'offset 256, commit_barrier + emit.
///
/// Pour simplifier les tests : resource et val ont la même longueur (msg[1] bytes).
/// Layout mémoire du message pour op=8 (put) :
///   [0]          : opcode (8)
///   [1]          : resource_len (N bytes)
///   [2..2+N]     : resource string
///   [2+N..2+N+8] : cap_id i64 LE
///   [10+N..10+2N]: value bytes (même len = N)
///
/// Layout mémoire du message pour op=9 (get) :
///   [0]          : opcode (9)
///   [1]          : resource_len (N bytes)
///   [2..2+N]     : resource string
///   [2+N..2+N+8] : cap_id i64 LE
pub const STORE_AGENT_WAT: &str = r#"(module
  (import "env" "commit_barrier"   (func $cb))
  (import "env" "emit"             (func $emit (param i32 i32 i32)))
  (import "env" "agent_store_put"  (func $put  (param i32 i32 i64 i32 i32) (result i32)))
  (import "env" "agent_store_get"  (func $get  (param i32 i32 i64 i32) (result i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    (local $rlen i32)
    (local $cap_lo i64)
    ;; msg[0]=0 → build history
    (if (i32.eqz (i32.load8_u (local.get $ptr)))
      (then
        call $cb
        i32.const 1
        local.get $ptr
        local.get $len
        call $emit
      )
    )
    ;; msg[0]=8 → agent_store_put
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 8))
      (then
        (local.set $rlen (i32.load8_u (i32.add (local.get $ptr) (i32.const 1))))
        ;; cap_id = i64 LE at ptr+2+rlen
        (local.set $cap_lo (i64.load (i32.add (i32.add (local.get $ptr) (i32.const 2)) (local.get $rlen))))
        (i32.store8 (i32.const 256)
          (call $put
            ;; resource_ptr = ptr+2, resource_len = rlen
            (i32.add (local.get $ptr) (i32.const 2))
            (local.get $rlen)
            ;; cap_id
            (local.get $cap_lo)
            ;; val_ptr = ptr+10+rlen, val_len = rlen
            (i32.add (i32.add (local.get $ptr) (i32.const 10)) (local.get $rlen))
            (local.get $rlen)
          )
        )
        call $cb
        i32.const 1
        (i32.const 256)
        (i32.const 1)
        call $emit
      )
    )
    ;; msg[0]=9 → agent_store_get
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 9))
      (then
        (local.set $rlen (i32.load8_u (i32.add (local.get $ptr) (i32.const 1))))
        (local.set $cap_lo (i64.load (i32.add (i32.add (local.get $ptr) (i32.const 2)) (local.get $rlen))))
        (i32.store8 (i32.const 256)
          (call $get
            ;; resource_ptr = ptr+2, resource_len = rlen
            (i32.add (local.get $ptr) (i32.const 2))
            (local.get $rlen)
            ;; cap_id
            (local.get $cap_lo)
            ;; out_ptr = 300
            (i32.const 300)
          )
        )
        call $cb
        i32.const 1
        (i32.const 256)
        (i32.const 1)
        call $emit
      )
    )
  )
  (memory (export "memory") 1)
)"#;

// État intermédiaire entre commit_barrier et emit (ADR-0010 §4).
// Stocké dans AgentState ; complété lors de l'appel emit qui appende le LogEntry.
struct PendingCommit {
    hash_before: [u8; 32],
    hash_after:  [u8; 32],
    parent_ids:  Vec<[u8; 32]>,
    ts_ms:       u64,
    seq:         u64,
}

/// F10 (absorbe F8) : regroupe les cinq champs du rate-limiter CapabilityDenied.
/// Un seul appel `.reset(now_ms)` garantit la co-cohérence de tous les champs —
/// deux sites de reset (fenêtre glissante + constructeur) ne peuvent plus diverger.
/// HashSet<String> remplace BTreeSet (F8) : les opérations sont contains/insert/len/clear,
/// aucune n'exige un ordre trié.
#[derive(Default)]
pub struct CapDeniedLimiter {
    pub count: u32,
    pub window_start_ms: u64,
    pub resources: std::collections::HashSet<String>,
    pub aggregate_emitted: bool,
    pub set_overflow_emitted: bool,
}

impl CapDeniedLimiter {
    fn reset(&mut self, now_ms: u64) {
        self.count = 0;
        self.window_start_ms = now_ms;
        self.resources.clear();
        self.aggregate_emitted = false;
        self.set_overflow_emitted = false;
    }
}

pub struct AgentState {
    pub id: AgentId,
    // ADR-0057 (MT-1) : tenant propriétaire de cet agent. DEFAULT en mono-tenant.
    // Le log/store sont partagés cross-tenant ; le cap_store est isolé par tenant.
    pub tenant: TenantId,
    // ADR-0058 (B-fort) : store des CauseHandle du tenant de cet agent. Source de vérité de
    // l'autorisation de citation cross-agent dans agent_add_cause (consulté à chaque appel).
    // L'auto-citation ne le consulte pas (autorité intrinsèque, §D10).
    // ADR-0060 (XR-0) : ce store est DÉRIVÉ de `cause_handle_registry.get_or_create(tenant)`,
    // jamais construit indépendamment — il pointe le même Mutex que l'entrée du registre pour
    // ce tenant (risque n°1 : éviter la double source de vérité store local ↔ registre).
    pub cause_handle_store: Arc<Mutex<CauseHandleStore>>,
    // ADR-0060 (XR-0) : registre cross-tenant des stores de CauseHandle. Partagé entre tous les
    // tenants servis par le même orchestrateur. Permet à la révocation (terminaison/rollback)
    // de balayer TOUS les stores — un handle émis par cet agent au profit d'un grantee d'un
    // AUTRE tenant vit dans le store de cet autre tenant et doit être révocable d'ici (XR-1).
    pub cause_handle_registry: Arc<CauseHandleRegistry>,
    pub seq: u64,
    pub store_ref: Arc<ContentStore>,
    pub log_ref: Arc<CausalLog>,
    pub last_snapshot: Option<[u8; 32]>,
    pub last_action: Option<[u8; 32]>,
    pub lifecycle: LifecycleState,
    // M2 (revue sécurité) : posé par agent_terminate. Une fois vrai, les host fns mutantes
    // (commit_barrier/emit/add_cause/request_validation/checkpoint/self_rollback) deviennent
    // no-op dans le cycle courant — empêche les effets « post-mortem » qu'un guest pourrait
    // émettre après avoir demandé sa terminaison (log-spoofing, évasion de terminaison).
    pub termination_requested: bool,
    // Commit en attente de payload (ADR-0010) ; flushé avec None si emit absent.
    // M4 : SOURCE UNIQUE de l'invariant H-cb-correct (emit n'est légal que si Some). L'ancien
    // drapeau `barrier_fired` (redondant) a été retiré — il se désynchronisait sur les chemins
    // de fin de cycle sans emit (request_validation/terminate post-barrier).
    pending_commit: Option<PendingCommit>,
    // A3 : dernier verdict de validation reçu du superviseur (None = jamais reçu).
    pub last_verdict: Option<ValidationVerdict>,
    // ADR-0003 (cross-agent) : causes externes en attente d'être incluses dans le prochain
    // commit_barrier. Chaque entrée est un action_id ([u8;32]) d'un autre agent.
    // Vidé après chaque commit_barrier. Populé par agent_add_cause().
    pub pending_extra_causes: Vec<[u8; 32]>,
    // ADR-0012 : identifiant de session courant (commence à 1, incrémenté à chaque frontière).
    pub session_id: u64,
    // seq au début de la session courante — action_count = seq - session_start_seq.
    pub session_start_seq: u64,
    // Timestamp ms du premier commit_barrier de cette session (0 = pas encore démarré).
    pub session_started_at_ms: u64,
    // Borne d'actions par session (configurable, défaut SESSION_DEFAULT_MAX_ACTIONS).
    pub session_max_actions: u64,
    // ADR-0012 : Borne de durée par session en ms (configurable, défaut
    // SESSION_DEFAULT_MAX_DURATION_MS). La valeur `0` désactive la borne durée
    // (cf. doc de SESSION_DEFAULT_MAX_DURATION_MS).
    pub session_max_duration_ms: u64,
    // Capabilities : store partagé (scheduler ou isolé) + caps propres à cet agent.
    pub cap_store: Arc<Mutex<CapabilityStore>>,
    pub own_caps: Vec<CapabilityId>,
    // ADR-0014 D14.a : timeout d'attente de verdict en millisecondes. Si aucun
    // `Message::ValidationResponse` n'arrive dans ce délai depuis l'entrée en
    // `AwaitingValidation`, `run_loop` injecte automatiquement un verdict `Timeout`
    // via `record_validation_response`. Configurable au constructeur.
    pub validation_timeout_ms: u64,
    // ADR-0025 : profil watchdog — détermine le nombre de ticks epoch par process_one.
    // Défaut : AgentProfile::LlmShort (5 s, rétro-compatible).
    pub agent_profile: AgentProfile,
    // ADR-0015 D15.2 : agent_id du parent direct (issu de Scheduler::spawn_child).
    // None pour les agents racine (créés directement par register, hors hiérarchie spawn).
    // Encodé dans le payload AgentCrash (0x13) ; [0u8;16] sentinelle si None.
    pub parent_agent_id: Option<AgentId>,
    // Erreur I/O produite par commit_barrier ou emit (host functions synchrones).
    // func_wrap ne supporte pas Result<(), E> arbitraire en Wasmtime 25 ; l'erreur est
    // stockée ici et lue dans process_one après call_async → RuntimeError::Wasmtime.
    pub host_error: Option<String>,
    // S6 (substrate requirement) : horloge substituable. SystemClock en prod,
    // LogicalClock en mode replay/SEF-6. Tous les call-sites qui insèrent un
    // timestamp dans une structure hashée passent par ce champ.
    // Voir crate::clock et ADR-0028.
    pub clock: Arc<dyn Clock>,
    // P1/P4 (SEF-3) — store clé-valeur des host functions `agent_store_get`/`agent_store_put`.
    // C1 (revue sécurité) : `Arc<Mutex<…>>` PARTAGEABLE (par tenant), namespacé par resource.
    // Auparavant un `HashMap` privé-par-agent : la capability gardait alors un magasin que
    // personne d'autre ne pouvait atteindre → P4 (isolation NON-ambiante d'un référent partagé)
    // était trivialement vraie pour la mauvaise raison. Désormais le référent est réel : deux
    // agents d'un tenant partageant ce store voient les mêmes octets, et l'absence de capability
    // ferme un accès réellement ouvert. Default = store propre à l'agent (mono-agent inchangé) ;
    // partage opt-in via `ActorInstanceBuilder::kv_store` (isolé entre tenants par construction).
    pub kv_store: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    // P2 (SEF-3) : rate-limiting des événements CapabilityDenied (0x14).
    // ADR-0051 §D2 (correctif #6, P4) — voir CapDeniedLimiter.
    pub cap_denied: CapDeniedLimiter,
    // SEF-9 / axe 1b (ADR-0050 §D3) : témoin hors-bande de l'audit des refus.
    // None en production (zéro impact, aucun changement de comportement).
    // Some(sink) en harness SEF-9 : chaque tentative refusée est enregistrée
    // ICI, AVANT le rate-limit de `emit_cap_denied`. L'oracle 1b compare ce
    // témoin (vérité-terrain des tentatives) au log `0x14` (sujet au rate-limit)
    // pour falsifier la fidélité du log d'audit sous flood. `check()` étant pur,
    // un refus ne laisse aucune trace d'état : ce témoin est le seul observable
    // constructible des tentatives refusées hors du log lui-même.
    pub cap_denied_witness: Option<Arc<Mutex<Vec<CapDeniedAttempt>>>>,
}

/// SEF-9 / axe 1b — enregistrement hors-bande d'une tentative d'accès refusée,
/// capturé au point de décision (`emit_cap_denied`) avant tout rate-limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapDeniedAttempt {
    pub resource: String,
    pub perm_flags: u8,
}

// T1.4 — Précondition func_wrap_async (ADR-0019) : AgentState doit être Send.
// Si cette assertion échoue à la compilation, identifier le champ non-Send avant B2.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<AgentState>();
};

impl AgentState {
    /// Enregistre une transition de cycle de vie dans le log causal.
    /// Non enregistrante dans ContentStore — pas de snapshot associé.
    pub fn log_lifecycle_event(&mut self, new_state: LifecycleState) {
        // M2 (revue sécurité) : `Terminated` est ABSORBANT. Aucune transition sortante n'est
        // permise (spec A4 : un agent terminé ne se respawn pas). Empêche qu'un chemin
        // (checkpoint, etc.) ressuscite un agent terminé en réécrivant son lifecycle.
        if self.lifecycle == LifecycleState::Terminated && new_state != LifecycleState::Terminated {
            return;
        }
        self.lifecycle = new_state;
        let now_ms = self.clock.now_ms();
        let now_us = self.clock.now_us();
        let parent_ids: Vec<[u8; 32]> = self.last_action.into_iter().collect();
        let hash = self.last_snapshot.unwrap_or([0u8; 32]);
        // Payload de base : [state_byte, seq LE 8 bytes] = 9 bytes.
        // ADR-0025 : pour Spawned (0x00), on ajoute le byte de profil en 10ème position.
        // Rétro-compatible : les lecteurs qui ne connaissent pas ADR-0025 voient 9 bytes
        // et les ignorent (ils lisent state_byte + seq uniquement).
        let mut payload_bytes = Vec::with_capacity(10);
        payload_bytes.push(new_state.as_u8());
        payload_bytes.extend_from_slice(&self.seq.to_le_bytes());
        if new_state == LifecycleState::Spawned {
            payload_bytes.push(self.agent_profile as u8);
        }
        let envelope = EmitEnvelope::new(
            EmitType::Lifecycle,
            self.id,
            self.seq,
            now_us,
            payload_bytes,
        );
        let entry = LogEntry {
            agent_id: self.id,
            ts_ms: now_ms,
            parent_ids,
            hash_before: hash,
            hash_after: hash,
            emit_payload: Some(envelope.to_msgpack()),
        };
        if let Ok(action_id) = self.log_ref.append(&entry) {
            self.last_action = Some(action_id);
        }
    }

    /// ADR-0015 D15.2-a — Enregistre une terminaison anormale dans le log causal.
    ///
    /// À émettre **avant** la transition `LifecycleState::Terminated` correspondante,
    /// dans la branche de `run_loop` où la cause est identifiée. Pour les terminaisons
    /// normales (`Suspend`, inbox fermée naturellement, `agent_terminate` depuis WASM),
    /// **ne pas** appeler cette méthode — seul `Lifecycle Terminated` est attendu.
    ///
    /// Payload : `AGENT_CRASH_PAYLOAD_LEN` = 49 octets fixes —
    ///   [cause u8 | parent_agent_id 16B | last_action_id 32B].
    /// `parent_agent_id` = sentinelle `[0u8;16]` si l'agent n'a pas de parent enregistré.
    /// `last_action_id`  = sentinelle `[0u8;32]` si aucune action n'a encore été émise.
    pub fn log_agent_crash(&mut self, cause: CrashCause) {
        let now_ms = self.clock.now_ms();
        let now_us = self.clock.now_us();
        let parent_ids: Vec<[u8; 32]> = self.last_action.into_iter().collect();
        let hash = self.last_snapshot.unwrap_or([0u8; 32]);

        let mut payload = [0u8; AGENT_CRASH_PAYLOAD_LEN];
        payload[0] = cause.as_u8();
        payload[1..17].copy_from_slice(&self.parent_agent_id.unwrap_or([0u8; 16]));
        payload[17..49].copy_from_slice(&self.last_action.unwrap_or([0u8; 32]));

        let envelope = EmitEnvelope::new(
            EmitType::AgentCrash,
            self.id,
            self.seq,
            now_us,
            payload.to_vec(),
        );
        let entry = LogEntry {
            agent_id: self.id,
            ts_ms: now_ms,
            parent_ids,
            hash_before: hash,
            hash_after: hash,
            emit_payload: Some(envelope.to_msgpack()),
        };
        // ADR-0027 : append() non-durable suffit pour 0x13 AgentCrash. L'événement est émis
        // depuis run_loop *avant* la mort du process (le process continue à tourner après).
        // Le WAL OS-buffered absorbe l'écriture ; un SIGKILL ultérieur ne perd rien (RocksDB
        // rejouera au redémarrage). Sous power-loss (hors scope Phase 6) la cause de
        // terminaison peut être perdue — limite acceptée, documentée dans ADR-0015 §Cas E
        // d'ADR-0027. Le ContentStore reflète le dernier état committé de l'agent : P6 tient.
        if let Ok(action_id) = self.log_ref.append(&entry) {
            // On met à jour `last_action` pour que le `Terminated` qui suit
            // référence ce `AgentCrash` dans ses `parent_ids` (chaînage causal).
            self.last_action = Some(action_id);
        }
        // ADR-0015 D-Q-V2.2 : AgentCrash implique Terminated — un seul append RocksDB
        // garantit l'atomicité. Ne pas appeler log_lifecycle_event(Terminated) après.
        self.lifecycle = LifecycleState::Terminated;
    }

    /// A3 — Enregistre la réponse de validation dans le log et restaure l'état Active.
    pub fn record_validation_response(&mut self, verdict: ValidationVerdict) {
        // M2 : un agent terminé ne peut pas être ramené à Active par un verdict tardif.
        if self.lifecycle == LifecycleState::Terminated {
            return;
        }
        self.last_verdict = Some(verdict);
        let now_ms = self.clock.now_ms();
        let now_us = self.clock.now_us();
        let parent_ids: Vec<[u8; 32]> = self.last_action.into_iter().collect();
        let hash = self.last_snapshot.unwrap_or([0u8; 32]);
        let envelope = EmitEnvelope::new(
            EmitType::ValidationResponse,
            self.id,
            self.seq,
            now_us,
            vec![verdict as u8],
        );
        let entry = LogEntry {
            agent_id: self.id,
            ts_ms: now_ms,
            parent_ids,
            hash_before: hash,
            hash_after: hash,
            emit_payload: Some(envelope.to_msgpack()),
        };
        if let Ok(action_id) = self.log_ref.append(&entry) {
            self.last_action = Some(action_id);
        }
        self.log_lifecycle_event(LifecycleState::Active);
    }

    /// ADR-0012 — Enregistre la frontière de session dans le log et démarre une nouvelle session.
    pub fn log_session_boundary(&mut self) {
        let action_count = self.seq - self.session_start_seq;
        let now_ms = self.clock.now_ms();
        let now_us = self.clock.now_us();
        let parent_ids: Vec<[u8; 32]> = self.last_action.into_iter().collect();
        let hash = self.last_snapshot.unwrap_or([0u8; 32]);
        let mut payload = [0u8; 16];
        payload[..8].copy_from_slice(&self.session_id.to_le_bytes());
        payload[8..].copy_from_slice(&action_count.to_le_bytes());
        let envelope = EmitEnvelope::new(
            EmitType::SessionBoundary,
            self.id,
            self.seq,
            now_us,
            payload.to_vec(),
        );
        let entry = LogEntry {
            agent_id: self.id,
            ts_ms: now_ms,
            parent_ids,
            hash_before: hash,
            hash_after: hash,
            emit_payload: Some(envelope.to_msgpack()),
        };
        if let Ok(action_id) = self.log_ref.append(&entry) {
            self.last_action = Some(action_id);
        }
        // Démarre la nouvelle session
        self.session_id += 1;
        self.session_start_seq = self.seq;
        self.session_started_at_ms = 0;
        self.log_lifecycle_event(LifecycleState::Checkpointed);
    }
}

/// P2 (SEF-3) — Émet un événement `CapabilityDenied` (0x14) dans le log causal.
///
/// Appelé directement depuis les host functions `agent_store_get` / `agent_store_put`
/// en cas de refus d'accès. Hors cycle commit_barrier/emit de l'agent (précédent :
/// `SchedulerRollback` émis depuis le scheduler, `AgentCrash` depuis run_loop).
///
/// Rate-limit : au plus 100 événements par fenêtre glissante de 1 seconde.
/// Si la borne est atteinte, un seul événement agrégé est émis avec `rate_limited=0x01`.
///
/// Payload format (standard, rate_limited=0x00) :
///   [agent_id 16B | cap_id u64 LE 8B | resource_len u8 | resource [u8;N≤255]
///    | perm_flags u8 | rate_limited u8]
///
/// Payload format (agrégé, rate_limited=0x01) :
///   [agent_id 16B | cap_id u64 LE 8B | count u32 LE 4B | perm_flags u8 | rate_limited u8=0x01]
///   resource_len est utilisé comme premier octet du count (4 bytes).
fn emit_cap_denied(
    caller: &mut Caller<AgentState>,
    cap_id: u64,
    resource: &str,
    perm_flags: u8,
) {
    use os_poc_causal_log::EmitType;

    // SEF-9 / axe 1b (ADR-0050 §D3) : témoin hors-bande AVANT le rate-limit.
    // Enregistre toute tentative refusée — vérité-terrain pour l'oracle 1b.
    // Aucun effet en production (witness = None).
    if let Some(sink) = caller.data().cap_denied_witness.clone() {
        lock_or_recover(&sink).push(CapDeniedAttempt {
            resource: resource.to_string(),
            perm_flags,
        });
    }

    // Rate-limit : 100 refus / 1s par agent
    const CAP_DENIED_RATE_LIMIT: u32 = 100;
    const CAP_DENIED_WINDOW_MS: u64 = 1_000;
    // ADR-0051 §D2 (correctif #6) : borne du nombre de resources distinctes
    // attribuées individuellement par fenêtre. Une resource nouvelle est attribuable
    // tant que le set n'est pas plein (≤ CAP_DENIED_MAX_DISTINCT_RESOURCES distinct).
    // Au-delà, un unique sentinel d'overflow de set est émis par fenêtre (F2) —
    // la garantie d'attribution est bornée, pas inconditionnelle (ADR-0051 D2 §P4 :
    // « borné »). Anti-DoS : au plus ~RATE_LIMIT + borne + 1 événements par fenêtre.
    const CAP_DENIED_MAX_DISTINCT_RESOURCES: usize = 32;

    let now_ms = {
        let s = caller.data();
        s.clock.now_ms()
    };

    // Vérifier et mettre à jour le rate-limit.
    // Une resource NOUVELLE (jamais vue dans la fenêtre, tant que le set n'est pas
    // plein) force un événement attribué AVEC sa resource, même au-delà du rate-limit
    // scalaire (correctif #6). Au-delà de CAP_DENIED_MAX_DISTINCT_RESOURCES resources
    // distinctes, un sentinel d'overflow de set est émis une fois (F2, ADR-0051 D2).
    let (should_emit, is_aggregated, denied_count) = {
        let s = caller.data_mut();
        // Reset de la fenêtre si > 1s
        if now_ms.saturating_sub(s.cap_denied.window_start_ms) >= CAP_DENIED_WINDOW_MS {
            s.cap_denied.reset(now_ms);
        }
        s.cap_denied.count += 1;
        let count = s.cap_denied.count;

        let is_new_resource = !s.cap_denied.resources.contains(resource);
        let has_room = s.cap_denied.resources.len() < CAP_DENIED_MAX_DISTINCT_RESOURCES;

        // Enregistrer la resource neuve avant tout branchement de sortie, pour
        // qu'elle soit visible comme "connue" dès le prochain appel.
        if is_new_resource && has_room {
            s.cap_denied.resources.insert(resource.to_string());
        }

        // F1 : sentinel prioritaire sur l'attribution. Le premier appel qui dépasse
        // la borne scalaire émet le sentinel quel que soit le chemin (new-resource ou
        // non). `aggregate_emitted` évite un double-sentinel dans la même fenêtre.
        if count > CAP_DENIED_RATE_LIMIT && !s.cap_denied.aggregate_emitted {
            s.cap_denied.aggregate_emitted = true;
            (true, true, count)
        } else if is_new_resource && has_room {
            // Resource neuve sous la borne → attribution préservée (correctif #6).
            (true, false, count)
        } else if count <= CAP_DENIED_RATE_LIMIT {
            // Resource connue (ou borne du set atteinte) sous le seuil scalaire.
            (true, false, count)
        } else if !has_room && is_new_resource && !s.cap_denied.set_overflow_emitted {
            // F2 : set plein, nouvelle resource, borne scalaire dépassée. Émettre un
            // unique sentinel d'overflow de set par fenêtre pour signaler le trou
            // d'audit sans réintroduire le DoS (ADR-0051 D2 — garantie "borné").
            s.cap_denied.set_overflow_emitted = true;
            (true, true, count)
        } else {
            // Silence : borne scalaire dépassée, sentinels déjà émis.
            (false, false, count)
        }
    };

    if !should_emit {
        return;
    }

    let (agent_id, log_ref, last_action, last_snapshot, now_us) = {
        let s = caller.data();
        (s.id, s.log_ref.clone(), s.last_action, s.last_snapshot, s.clock.now_us())
    };

    let parent_ids: Vec<[u8; 32]> = last_action.into_iter().collect();
    let hash = last_snapshot.unwrap_or([0u8; 32]);

    let mut payload: Vec<u8> = Vec::new();
    payload.extend_from_slice(&agent_id);
    payload.extend_from_slice(&cap_id.to_le_bytes());

    if is_aggregated {
        // Payload agrégé : count sur 4 bytes à la place de resource_len+resource
        payload.extend_from_slice(&denied_count.to_le_bytes());
        payload.push(perm_flags);
        payload.push(0x01u8); // rate_limited
    } else {
        let res_bytes = resource.as_bytes();
        let res_len = res_bytes.len().min(255);
        payload.push(res_len as u8);
        payload.extend_from_slice(&res_bytes[..res_len]);
        payload.push(perm_flags);
        payload.push(0x00u8); // rate_limited = false
    }

    let seq = caller.data().seq;
    let envelope = EmitEnvelope::new(EmitType::CapabilityDenied, agent_id, seq, now_us, payload);
    let entry = LogEntry {
        agent_id,
        ts_ms: now_ms,
        parent_ids,
        hash_before: hash,
        hash_after: hash,
        emit_payload: Some(envelope.to_msgpack()),
    };
    if let Ok(action_id) = log_ref.append(&entry) {
        caller.data_mut().last_action = Some(action_id);
    }
}

/// Builder unifiant la construction d'un [`ActorInstance`].
///
/// Remplace l'explosion combinatoire des constructeurs `new_precompiled_*` : chaque
/// dimension (caps, timeout, session, inférence, profil, horloge) est un setter optionnel
/// avec un défaut sain, et toute combinaison est valide — constat du refactor 2026-06-07 :
/// l'inner-builder `build_instance_inner_with_profile_and_clock` accepte l'intégralité du
/// produit cartésien, il n'existe aucune combinaison sémantiquement interdite. `build()` ne
/// faillit donc que sur la compilation/instanciation du module (pas de fail-closed de
/// combinaison à prévoir). Voir ADR-0025 (profil), ADR-0019 (inférence), ADR-0014 D14.a
/// (timeout), ADR-0012 D7 (session), ADR-0028 (horloge).
///
/// C'est le point d'entrée prévu pour `.tenant(TenantId)` (chantier MT-1) : ajouter le
/// multi-tenant n'ajoutera pas un 9ᵉ constructeur, juste un setter de plus.
pub struct ActorInstanceBuilder<'a> {
    engine: &'a Engine,
    module: &'a Module,
    agent_id: AgentId,
    store_ref: Arc<ContentStore>,
    log_ref: Arc<CausalLog>,
    cap_store: Arc<Mutex<CapabilityStore>>,
    initial_caps: Vec<CapabilityId>,
    validation_timeout_ms: u64,
    session_max_duration_ms: u64,
    infer_fn: Option<InferFn>,
    profile: AgentProfile,
    clock: Arc<dyn Clock>,
    tenant: TenantId,
    cause_handle_registry: Arc<CauseHandleRegistry>,
    kv_store: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl<'a> ActorInstanceBuilder<'a> {
    /// Builder avec tous les défauts (équivalent de `ActorInstance::new_precompiled`) :
    /// capabilities vides, timeouts par défaut, pas d'inférence, profil `LlmShort`,
    /// `SystemClock`.
    pub fn new(
        engine: &'a Engine,
        module: &'a Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
    ) -> Self {
        Self {
            engine,
            module,
            agent_id,
            store_ref,
            log_ref,
            cap_store: Arc::new(Mutex::new(CapabilityStore::new())),
            initial_caps: vec![],
            validation_timeout_ms: SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            session_max_duration_ms: SESSION_DEFAULT_MAX_DURATION_MS,
            infer_fn: None,
            profile: AgentProfile::LlmShort,
            clock: Arc::new(SystemClock),
            tenant: TenantId::DEFAULT,
            cause_handle_registry: Arc::new(CauseHandleRegistry::new()),
            kv_store: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Store de capabilities partagé + caps initiales (ADR-0005).
    pub fn caps(
        mut self,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
    ) -> Self {
        self.cap_store = cap_store;
        self.initial_caps = initial_caps;
        self
    }

    /// Timeout d'attente du verdict A3 par agent (ADR-0014 D14.a). Tests : valeur courte.
    pub fn validation_timeout_ms(mut self, ms: u64) -> Self {
        self.validation_timeout_ms = ms;
        self
    }

    /// Borne de durée de session (ADR-0012 D7).
    pub fn session_max_duration_ms(mut self, ms: u64) -> Self {
        self.session_max_duration_ms = ms;
        self
    }

    /// Active la host function `agent_infer` (ADR-0019 T5). `infer_fn` provient de
    /// `InferencePool::as_infer_fn(Arc::clone(&pool))`.
    pub fn inference(mut self, infer_fn: InferFn) -> Self {
        self.infer_fn = Some(infer_fn);
        self
    }

    /// Profil watchdog explicite (ADR-0025).
    pub fn profile(mut self, profile: AgentProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Horloge substituable (ADR-0028) : `SystemClock` en prod, `LogicalClock` pour SEF-6.
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Tenant propriétaire de l'acteur (ADR-0057, MT-1). Default = `TenantId::DEFAULT`
    /// (mono-tenant). En multi-tenant, le runner attribue un `CapabilityStore` distinct par
    /// tenant via [`caps`](Self::caps) ; le log et le store restent partagés.
    pub fn tenant(mut self, tenant: TenantId) -> Self {
        self.tenant = tenant;
        self
    }

    /// Registre cross-tenant des `CauseHandle` (ADR-0058 B-fort / ADR-0060). Default = registre
    /// vide propre à cet acteur. Pour autoriser des citations cross-agent (intra- ou cross-tenant)
    /// et rendre la révocation cross-tenant possible, le runner partage le **même**
    /// `Arc<CauseHandleRegistry>` entre les agents concernés ; il minte les handles dans le store
    /// du tenant *grantee* via `registry.get_or_create(tenant_grantee)`. Le store local de
    /// l'agent est dérivé du registre au `build` (jamais construit à part — risque n°1).
    pub fn cause_handle_registry(mut self, registry: Arc<CauseHandleRegistry>) -> Self {
        self.cause_handle_registry = registry;
        self
    }

    /// C1 (revue sécurité) — store clé-valeur PARTAGEABLE des host fns `agent_store_*`. Default =
    /// store propre à cet acteur (mono-agent inchangé). Pour démontrer P4 sur un référent réel,
    /// le runner partage le **même** `Arc<Mutex<…>>` entre les agents d'un tenant (et un store
    /// DISJOINT par tenant — isolation inter-tenant par construction). La capability garde alors
    /// un accès réellement ouvert, pas un magasin privé inaccessible aux autres.
    pub fn kv_store(mut self, kv_store: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>) -> Self {
        self.kv_store = kv_store;
        self
    }

    /// Construit l'acteur. Faillit uniquement sur compilation/instanciation du module.
    pub async fn build(self) -> Result<ActorInstance, RuntimeError> {
        // ADR-0060 (risque n°1) : le store local est DÉRIVÉ du registre pour ce tenant —
        // unique point d'insertion, garantit store_local == registre[tenant] (même Arc).
        let cause_handle_store = self.cause_handle_registry.get_or_create(self.tenant);
        ActorInstance::build_instance_inner_with_profile_and_clock(
            self.engine,
            self.module,
            self.agent_id,
            self.store_ref,
            self.log_ref,
            self.cap_store,
            self.initial_caps,
            self.validation_timeout_ms,
            self.session_max_duration_ms,
            self.infer_fn,
            self.profile,
            self.clock,
            self.tenant,
            cause_handle_store,
            self.cause_handle_registry,
            self.kv_store,
        )
        .await
    }
}

pub struct ActorInstance {
    store: Store<AgentState>,
    memory: Memory,
    process_fn: TypedFunc<(i32, i32), ()>,
}

impl ActorInstance {
    /// Crée un acteur en compilant AGENT_WAT.
    pub async fn new(
        engine: &Engine,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
    ) -> Result<Self, RuntimeError> {
        let module = Module::new(engine, AGENT_WAT)?;
        Self::new_precompiled(engine, &module, agent_id, store_ref, log_ref).await
    }

    /// Crée un acteur depuis un module pré-compilé (partageable entre N instances).
    pub async fn new_precompiled(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .build()
            .await
    }

    /// ADR-0025 : variante sans inference, avec profil watchdog explicite.
    pub async fn new_precompiled_with_profile(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        profile: AgentProfile,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .session_max_duration_ms(0)
            .profile(profile)
            .build()
            .await
    }

    /// Variante avec capabilities partagées (utilisée par Scheduler::spawn_child).
    pub async fn new_precompiled_with_caps(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .caps(cap_store, initial_caps)
            .build()
            .await
    }

    /// ADR-0014 D14.a — Variante permettant de configurer le timeout d'attente de
    /// verdict A3 par agent. Les tests utilisent une valeur courte (ex. 50 ms) pour vérifier
    /// le mécanisme sans attendre 30 s.
    ///
    /// La durée de session par défaut (`SESSION_DEFAULT_MAX_DURATION_MS`) est utilisée ;
    /// pour configurer aussi la borne durée, utiliser
    /// `new_precompiled_with_caps_timeout_and_session`.
    pub async fn new_precompiled_with_caps_and_timeout(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
        validation_timeout_ms: u64,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .caps(cap_store, initial_caps)
            .validation_timeout_ms(validation_timeout_ms)
            .build()
            .await
    }

    /// ADR-0012 D7 — Variante complète exposant aussi `session_max_duration_ms`.
    pub async fn new_precompiled_with_caps_timeout_and_session(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
        validation_timeout_ms: u64,
        session_max_duration_ms: u64,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .caps(cap_store, initial_caps)
            .validation_timeout_ms(validation_timeout_ms)
            .session_max_duration_ms(session_max_duration_ms)
            .build()
            .await
    }

    /// ADR-0019 T5 — Variante avec host function `agent_infer` activée.
    /// `infer_fn` est obtenu via `InferencePool::as_infer_fn(Arc::clone(&pool))`.
    pub async fn new_precompiled_with_inference(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
        validation_timeout_ms: u64,
        session_max_duration_ms: u64,
        infer_fn: InferFn,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .caps(cap_store, initial_caps)
            .validation_timeout_ms(validation_timeout_ms)
            .session_max_duration_ms(session_max_duration_ms)
            .inference(infer_fn)
            .build()
            .await
    }

    /// S6 / ADR-0028 — Variante minimale exposant l'horloge substituable.
    /// Pas d'inférence, pas de profil watchdog explicite (LlmShort par défaut).
    /// Usage : SEF-6 (P5 — déterminisme de transition d'état).
    pub async fn new_precompiled_with_clock(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .clock(clock)
            .build()
            .await
    }

    /// ADR-0025 : variante avec profil watchdog explicite.
    pub async fn new_precompiled_with_inference_and_profile(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
        validation_timeout_ms: u64,
        session_max_duration_ms: u64,
        infer_fn: InferFn,
        profile: AgentProfile,
    ) -> Result<Self, RuntimeError> {
        ActorInstanceBuilder::new(engine, module, agent_id, store_ref, log_ref)
            .caps(cap_store, initial_caps)
            .validation_timeout_ms(validation_timeout_ms)
            .session_max_duration_ms(session_max_duration_ms)
            .inference(infer_fn)
            .profile(profile)
            .build()
            .await
    }

    /// S6 / ADR-0028 — Implémentation unique de construction d'un acteur (toutes dimensions).
    /// SystemClock en prod ; LogicalClock pour SEF-6 (P5 — déterminisme de transition).
    /// Point d'entrée canonique : passer par [`ActorInstanceBuilder`] plutôt que d'appeler
    /// directement (réservé `pub(crate)` depuis le refactor builder 2026-06-07).
    pub(crate) async fn build_instance_inner_with_profile_and_clock(
        engine: &Engine,
        module: &Module,
        agent_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        cap_store: Arc<Mutex<CapabilityStore>>,
        initial_caps: Vec<CapabilityId>,
        validation_timeout_ms: u64,
        session_max_duration_ms: u64,
        infer_fn: Option<InferFn>,
        profile: AgentProfile,
        clock: Arc<dyn Clock>,
        tenant: TenantId,
        cause_handle_store: Arc<Mutex<CauseHandleStore>>,
        cause_handle_registry: Arc<CauseHandleRegistry>,
        kv_store: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    ) -> Result<Self, RuntimeError> {
        let state = AgentState {
            id: agent_id,
            tenant,
            cause_handle_store,
            cause_handle_registry,
            seq: 0,
            store_ref,
            log_ref,
            last_snapshot: None,
            last_action: None,
            lifecycle: LifecycleState::Spawned,
            termination_requested: false,
            pending_commit: None,
            last_verdict: None,
            pending_extra_causes: vec![],
            session_id: 1,
            session_start_seq: 0,
            session_started_at_ms: 0,
            session_max_actions: SESSION_DEFAULT_MAX_ACTIONS,
            session_max_duration_ms,
            cap_store,
            own_caps: initial_caps,
            validation_timeout_ms,
            agent_profile: profile,
            parent_agent_id: None,
            host_error: None,
            clock,
            kv_store,
            cap_denied: CapDeniedLimiter::default(),
            cap_denied_witness: None,
        };

        let mut wasm_store = Store::new(engine, state);
        // D9 (ADR-0019) : quand epoch_interruption est actif, la deadline par défaut est 1
        // (trap au 1er tick du thread background). On la positionne très haute ici ; run_loop
        // la réarme à MAX_PROCESS_ONE_TICKS avant chaque process_one. No-op si epoch désactivé.
        wasm_store.set_epoch_deadline(1_000_000_000);
        wasm_store.epoch_deadline_trap();
        let mut linker: Linker<AgentState> = Linker::new(engine);

        // Wasmtime 25 : func_wrap ne supporte pas Result<(), E> arbitraire pour les host
        // functions synchrones. Les erreurs I/O sont stockées dans AgentState::host_error
        // et converties en RuntimeError::Wasmtime dans process_one après call_async.
        linker.func_wrap("env", "commit_barrier", |mut caller: Caller<AgentState>| {
            if caller.data().termination_requested { return; } // M2 : pas d'effet post-mortem
            // S6 : horloge substituable (SystemClock prod, LogicalClock SEF-6).
            // now_us est inséré dans SnapshotHeader.ts_us (→ snapshot_id) et dans
            // PendingCommit.ts_ms (→ LogEntry.ts_ms du futur emit).
            let (store_ref, agent_id, seq, last_snapshot, last_action, extra_causes,
                 now_us, now_ms) = {
                let s = caller.data();
                (s.store_ref.clone(), s.id, s.seq, s.last_snapshot, s.last_action,
                 s.pending_extra_causes.clone(),
                 s.clock.now_us(), s.clock.now_ms())
            };

            // SEF-4 CrashPoint #1 — avant `put_block`. Aucun effet store/log encore.
            #[cfg(feature = "crash-injection")]
            crate::crash_point::fire(crate::crash_point::CrashPoint::CommitBarrierPrePutBlock);

            let mut state_bytes = [0u8; 64];
            state_bytes[..16].copy_from_slice(&agent_id);
            state_bytes[16..24].copy_from_slice(&seq.to_le_bytes());

            let data_h = match store_ref.put_block(&state_bytes) {
                Ok(h) => h,
                Err(e) => { caller.data_mut().host_error = Some(format!("ContentStore::put_block: {e}")); return; }
            };

            // SEF-4 CrashPoint #2 — entre `put_block` et `put_snapshot`. Block orphelin
            // potentiel (data_h écrit dans CF `blocks`, pas référencé par un SnapshotHeader).
            #[cfg(feature = "crash-injection")]
            crate::crash_point::fire(crate::crash_point::CrashPoint::CommitBarrierBetweenPutBlockAndPutSnapshot);

            let snap_id = match store_ref.put_snapshot(SnapshotHeader {
                data_hash: data_h,
                parent: last_snapshot,
                seq,
                ts_us: now_us,
            }) {
                Ok(s) => s,
                Err(e) => { caller.data_mut().host_error = Some(format!("ContentStore::put_snapshot: {e}")); return; }
            };

            // SEF-4 CrashPoint #3 — après `put_snapshot`, avant que `emit` (côté WASM)
            // n'ait pu appeler `CausalLog::append`. ContentStore en avance sur log :
            // `last_snapshot` du store contient un header pointant vers data_h, mais
            // le log n'a pas l'action correspondante. Asymétrie documentée ADR-0027.
            #[cfg(feature = "crash-injection")]
            crate::crash_point::fire(crate::crash_point::CrashPoint::CommitBarrierPostPutSnapshotPreLogAppend);

            let hash_before = last_snapshot.unwrap_or([0u8; 32]);
            // ADR-0003 : parent_ids = [last_action optionnel] + causes externes (cross-agent).
            let mut parent_ids: Vec<[u8; 32]> = last_action.into_iter().collect();
            parent_ids.extend_from_slice(&extra_causes);

            let s = caller.data_mut();
            s.last_snapshot = Some(snap_id);
            s.seq += 1;
            if s.session_started_at_ms == 0 {
                s.session_started_at_ms = now_ms;
            }
            s.pending_extra_causes.clear();
            s.pending_commit = Some(PendingCommit {
                hash_before,
                hash_after: snap_id,
                parent_ids,
                ts_ms: now_ms,
                seq,
            });
        })?;

        // ADR-0003 / ADR-0036 — agent_add_cause : ajoute un action_id externe aux causes du
        // prochain commit_barrier (B-light : vérification d'existence, pas de capability cross-agent).
        // Retourne : 0 succès, -1 ptr OOB, -2 MAX_EXTRA_CAUSES atteint, -3 action_id inconnu, -4 I/O.
        linker.func_wrap("env", "agent_add_cause", |mut caller: Caller<AgentState>, action_id_ptr: i32| -> i32 {
            if caller.data().termination_requested { return -3; } // M2 : pas de citation post-mortem
            const ACTION_ID_LEN: usize = 32;
            const MAX_EXTRA_CAUSES: usize = 16;

            // borne anti-DoS checkée AVANT lecture mémoire (évite N lookups avant rejet)
            if caller.data().pending_extra_causes.len() >= MAX_EXTRA_CAUSES {
                return -2;
            }

            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("export 'memory' manquant");
            let data_len = memory.data_size(&caller);
            let ptr = action_id_ptr as usize;
            if ptr.checked_add(ACTION_ID_LEN).map_or(true, |end| end > data_len) {
                return -1;
            }
            let mut action_id = [0u8; ACTION_ID_LEN];
            memory
                .read(&caller, ptr, &mut action_id)
                .expect("agent_add_cause: lecture mémoire WASM");

            // ADR-0058 (B-fort) — dispatch d'autorisation. L'existence dans le log reste un
            // préalable (fail-closed I/O), mais ne suffit PLUS à autoriser une citation
            // cross-agent (c'était le confused-deputy de B-light en log partagé, ADR-0057).
            let log = caller.data().log_ref.clone();
            match log.get(&action_id) {
                Ok(Some(entry)) => {
                    let caller_id = caller.data().id;
                    // §D10 — auto-citation : autorité intrinsèque sur ses propres actions.
                    // §D1/R1 — sinon, exige un CauseHandle (grantee=caller, action_id) dans le
                    // store partagé-par-tenant (unique source de vérité, consultée à chaque appel).
                    let allowed = entry.agent_id == caller_id
                        || lock_or_recover(&caller.data().cause_handle_store).contains(&caller_id, &action_id);
                    if allowed {
                        caller.data_mut().pending_extra_causes.push(action_id);
                        0
                    } else {
                        // §D8 — refus indistinct de « action inconnue » (-3), pas d'oracle dédié.
                        -3
                    }
                }
                Ok(None) => -3,
                Err(_) => -4,
            }
        })?;

        // A4 — agent_checkpoint : transition → Checkpointed, enregistrée dans le log.
        // L'acteur signale explicitement un point de sauvegarde (spec §A4).
        // Retourne le seq courant (= position causale du checkpoint).
        linker.func_wrap("env", "agent_checkpoint", |mut caller: Caller<AgentState>| -> i32 {
            if caller.data().termination_requested { return -1; } // M2 : pas de checkpoint post-mortem
            let seq = caller.data().seq;
            caller.data_mut().log_lifecycle_event(LifecycleState::Checkpointed);
            seq as i32
        })?;

        // A4 — agent_terminate : transition → Terminated, enregistrée dans le log.
        // Après cet appel, run_loop doit arrêter le traitement.
        linker.func_wrap("env", "agent_terminate", |mut caller: Caller<AgentState>| {
            // M2 : poser le drapeau AVANT le log → toute host fn mutante appelée ensuite dans
            // le même cycle WASM devient no-op (pas d'effet post-mortem).
            caller.data_mut().termination_requested = true;
            caller.data_mut().log_lifecycle_event(LifecycleState::Terminated);
        })?;

        // A1 — agent_introspect (02c-primitives-agent.md §A1).
        // Lecture seule, non enregistrée dans le log causal.
        // Écrit INTROSPECT_PAYLOAD_LEN bytes à out_ptr ; retourne le nombre d'octets écrits.
        linker.func_wrap("env", "agent_introspect", |mut caller: Caller<AgentState>, out_ptr: i32, out_max_len: i32| -> i32 {
            if out_max_len < INTROSPECT_PAYLOAD_LEN as i32 {
                return -1;
            }
            let (last_action, seq, last_snapshot, lifecycle) = {
                let s = caller.data();
                (s.last_action, s.seq, s.last_snapshot, s.lifecycle)
            };
            let mut buf = [0u8; INTROSPECT_PAYLOAD_LEN];
            let mut flags: u8 = 0;
            if let Some(id) = last_action {
                buf[..32].copy_from_slice(&id);
                flags |= 0x01;
            }
            buf[32..40].copy_from_slice(&seq.to_le_bytes());
            if let Some(snap) = last_snapshot {
                buf[40..72].copy_from_slice(&snap);
                flags |= 0x02;
            }
            buf[72] = flags;
            buf[73] = lifecycle.as_u8();
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("export 'memory' manquant");
            // Bounds-check explicite : out_ptr invalide → -2 (pas de panique hôte, spec/08 §1).
            let data_size = memory.data_size(&caller);
            match (out_ptr as usize).checked_add(INTROSPECT_PAYLOAD_LEN) {
                Some(end) if end <= data_size => {}
                _ => return -2,
            }
            memory
                .write(&mut caller, out_ptr as usize, &buf)
                .expect("bounds vérifiées ci-dessus");
            INTROSPECT_PAYLOAD_LEN as i32
        })?;

        // A2 — agent_self_rollback (02c-primitives-agent.md §A2).
        // Rollback borné : restaure last_snapshot au snapshot target_seq dans la chaîne.
        // Enregistre le rollback comme action causale (SelfRollback) dans le log.
        // Retourne target_seq en cas de succès, <0 en cas d'erreur :
        //   -1 : depth hors bornes [1, MAX_SELF_ROLLBACK_DEPTH]
        //   -2 : aucun historique snapshot (seq = 0)
        //   -3 : historique insuffisant (seq < 1 + depth)
        //   -4 : erreur ContentStore (snapshot corrompu ou chaîne brisée)
        linker.func_wrap("env", "agent_self_rollback", |mut caller: Caller<AgentState>, depth: i32| -> i32 {
            if caller.data().termination_requested { return -1; } // M2 : pas de rollback post-mortem
            if depth < 1 || depth as usize > MAX_SELF_ROLLBACK_DEPTH {
                return -1;
            }

            let (last_snapshot, seq, last_action, store_ref, log_ref, agent_id) = {
                let s = caller.data();
                (s.last_snapshot, s.seq, s.last_action, s.store_ref.clone(), s.log_ref.clone(), s.id)
            };

            let tip = match last_snapshot {
                Some(h) => h,
                None => return -2,
            };

            let depth_u = depth as u64;
            if seq < 1 + depth_u {
                return -3;
            }

            // last_snapshot.seq = seq - 1  →  target_seq = seq - 1 - depth
            let target_seq = seq - 1 - depth_u;

            let path = match store_ref.rollback_path(&tip, target_seq) {
                Ok(p) => p,
                Err(_) => return -4,
            };

            let target_snap = *path.last().expect("rollback_path retourne un vecteur non-vide");

            // S6 : horloge substituable.
            let (now_ms, now_us) = {
                let s = caller.data();
                (s.clock.now_ms(), s.clock.now_us())
            };

            let parent_ids: Vec<[u8; 32]> = last_action.into_iter().collect();
            let mut payload = [0u8; 9];
            payload[0] = depth as u8;
            payload[1..9].copy_from_slice(&target_seq.to_le_bytes());

            let envelope = EmitEnvelope::new(
                EmitType::SelfRollback,
                agent_id,
                seq,
                now_us,
                payload.to_vec(),
            );
            let entry = LogEntry {
                agent_id,
                ts_ms: now_ms,
                parent_ids,
                hash_before: tip,
                hash_after: target_snap,
                emit_payload: Some(envelope.to_msgpack()),
            };
            if let Ok(action_id) = log_ref.append(&entry) {
                let s = caller.data_mut();
                s.last_snapshot = Some(target_snap);
                s.last_action = Some(action_id);
            }

            // ADR-0019 §Q7 : SelfRollback n'incrémente pas seq — le log est append-only et
            // seq est l'horloge logique monotone. Le rollback est tracé via le LogEntry
            // SelfRollback produit ci-dessus ; la prochaine action référencera ce LogEntry.
            target_seq as i32
        })?;

        // ADR-0012 — agent_session_info : lecture de l'état de session courant.
        // Écrit 24 bytes à out_ptr : [session_id u64 LE, action_count u64 LE, started_at_ms u64 LE].
        // Retourne 24 (bytes écrits) ou -1 si out_max_len < 24.
        linker.func_wrap("env", "agent_session_info", |mut caller: Caller<AgentState>, out_ptr: i32, out_max_len: i32| -> i32 {
            const SESSION_INFO_LEN: usize = 24;
            if out_max_len < SESSION_INFO_LEN as i32 {
                return -1;
            }
            let (session_id, action_count, started_at_ms) = {
                let s = caller.data();
                (s.session_id, s.seq - s.session_start_seq, s.session_started_at_ms)
            };
            let mut buf = [0u8; SESSION_INFO_LEN];
            buf[0..8].copy_from_slice(&session_id.to_le_bytes());
            buf[8..16].copy_from_slice(&action_count.to_le_bytes());
            buf[16..24].copy_from_slice(&started_at_ms.to_le_bytes());
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("export 'memory' manquant");
            // Bounds-check explicite : out_ptr invalide → -2 (pas de panique hôte, spec/08 §1).
            let data_size = memory.data_size(&caller);
            match (out_ptr as usize).checked_add(SESSION_INFO_LEN) {
                Some(end) if end <= data_size => {}
                _ => return -2,
            }
            memory
                .write(&mut caller, out_ptr as usize, &buf)
                .expect("bounds vérifiées ci-dessus");
            SESSION_INFO_LEN as i32
        })?;

        // A3 — agent_request_validation (02c-primitives-agent.md §A3).
        // Enregistre une demande de validation dans le log causal et passe l'agent en AwaitingValidation.
        // Le verdict arrive via Message::ValidationResponse (run_loop) puis est récupérable
        // via agent_get_verdict() lors du prochain cycle.
        // risk_level : 0=low, 1=medium, 2=high. Hors borne → -1 (demande ignorée).
        // Retourne 0 en cas d'enregistrement réussi.
        linker.func_wrap("env", "agent_request_validation", |mut caller: Caller<AgentState>, risk_level: i32| -> i32 {
            if caller.data().termination_requested { return -1; } // M2 : ne peut pas ressusciter via AwaitingValidation
            if risk_level < 0 || risk_level > 2 {
                return -1;
            }
            let (agent_id, seq, last_action, last_snapshot, log_ref, now_ms, now_us) = {
                let s = caller.data();
                (s.id, s.seq, s.last_action, s.last_snapshot, s.log_ref.clone(),
                 s.clock.now_ms(), s.clock.now_us())
            };
            let parent_ids: Vec<[u8; 32]> = last_action.into_iter().collect();
            let hash = last_snapshot.unwrap_or([0u8; 32]);
            let envelope = EmitEnvelope::new(
                EmitType::ValidationRequest,
                agent_id,
                seq,
                now_us,
                vec![risk_level as u8],
            );
            let entry = LogEntry {
                agent_id,
                ts_ms: now_ms,
                parent_ids,
                hash_before: hash,
                hash_after: hash,
                emit_payload: Some(envelope.to_msgpack()),
            };
            if let Ok(action_id) = log_ref.append(&entry) {
                let s = caller.data_mut();
                s.last_action = Some(action_id);
                s.lifecycle = LifecycleState::AwaitingValidation;
            }
            0
        })?;

        // A3 — agent_get_verdict : retourne le dernier verdict de validation reçu.
        // 0=Approved, 1=Rejected, 2=Timeout, -1=aucun verdict enregistré.
        linker.func_wrap("env", "agent_get_verdict", |caller: Caller<AgentState>| -> i32 {
            match caller.data().last_verdict {
                Some(v) => v as i32,
                None    => -1,
            }
        })?;

        // agent_check_cap(cap_id: i64, resource_ptr: i32, resource_len: i32, perm_flags: i32) -> i32
        // perm_flags bits: 0=read, 1=write, 2=execute, 3=delegate
        // Retourne 1 si autorisé, 0 si refusé, -1 si out-of-bounds ou UTF-8 invalide.
        linker.func_wrap("env", "agent_check_cap", |mut caller: Caller<AgentState>, cap_id: i64, resource_ptr: i32, resource_len: i32, perm_flags: i32| -> i32 {
            let (agent_id, cap_store_ref) = {
                let s = caller.data();
                (s.id, s.cap_store.clone())
            };
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("export 'memory' manquant");
            let data_len = memory.data_size(&caller);
            let ptr = resource_ptr as usize;
            let len = resource_len as usize;
            if ptr.checked_add(len).map_or(true, |end| end > data_len) {
                return -1;
            }
            let mut resource_bytes = vec![0u8; len];
            memory.read(&caller, ptr, &mut resource_bytes).expect("agent_check_cap: lecture mémoire");
            let resource = match std::str::from_utf8(&resource_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => return -1,
            };
            let perm = Permissions {
                read:     perm_flags & 0x01 != 0,
                write:    perm_flags & 0x02 != 0,
                execute:  perm_flags & 0x04 != 0,
                delegate: perm_flags & 0x08 != 0,
            };
            let ok = lock_or_recover(&cap_store_ref).check(&agent_id, cap_id as u64, &resource, &perm);
            if ok { 1 } else { 0 }
        })?;

        // P1 (SEF-3) — agent_store_get : lecture capability-gated depuis le KV store local.
        // Signature : agent_store_get(resource_ptr: i32, resource_len: i32, cap_id: i64,
        //                             out_ptr: i32) -> i32
        // Retourne 0 si autorisé (valeur écrite à out_ptr ; si absente, rien écrit, 0 retourné),
        //          -1 si refusé (CapabilityDenied logué), -2 si paramètres invalides.
        // Note : 0 même si la clé n'existe pas (absence ≠ refus d'accès).
        linker.func_wrap("env", "agent_store_get",
            |mut caller: Caller<AgentState>,
             resource_ptr: i32, resource_len: i32,
             cap_id: i64,
             out_ptr: i32| -> i32
        {
            let perm_read = Permissions { read: true, write: false, execute: false, delegate: false };
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("export 'memory' manquant");
            let data_len = memory.data_size(&caller);
            let rptr = resource_ptr as usize;
            let rlen = resource_len as usize;
            if rptr.checked_add(rlen).map_or(true, |end| end > data_len) {
                return -2;
            }
            let mut res_bytes = vec![0u8; rlen];
            memory.read(&caller, rptr, &mut res_bytes).expect("agent_store_get: lecture resource");
            let resource = match std::str::from_utf8(&res_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => return -2,
            };

            let (agent_id, cap_store_ref, log_ref) = {
                let s = caller.data();
                (s.id, s.cap_store.clone(), s.log_ref.clone())
            };

            let allowed = lock_or_recover(&cap_store_ref)
                .check(&agent_id, cap_id as u64, &resource, &perm_read);

            if !allowed {
                emit_cap_denied(&mut caller, cap_id as u64, &resource, 0x01 /* read */);
                return -1;
            }

            // Lecture dans le KV store local
            let value_opt = lock_or_recover(&caller.data().kv_store).get(&resource).cloned();
            if let Some(value) = value_opt {
                let out = out_ptr as usize;
                if out.checked_add(value.len()).map_or(true, |end| end > data_len) {
                    return -2;
                }
                let mem2 = caller.get_export("memory")
                    .and_then(|e| e.into_memory()).unwrap();
                mem2.write(&mut caller, out, &value).expect("agent_store_get: écriture out");
            }
            // Pas de valeur → retourne 0 (clé absente ≠ refus)
            let _ = log_ref; // future utilisation
            0
        })?;

        // P1 (SEF-3) — agent_store_put : écriture capability-gated dans le KV store local.
        // Signature : agent_store_put(resource_ptr: i32, resource_len: i32, cap_id: i64,
        //                             val_ptr: i32, val_len: i32) -> i32
        // Retourne 0 si autorisé et écrit, -1 si refusé (CapabilityDenied logué),
        //          -2 si paramètres invalides.
        linker.func_wrap("env", "agent_store_put",
            |mut caller: Caller<AgentState>,
             resource_ptr: i32, resource_len: i32,
             cap_id: i64,
             val_ptr: i32, val_len: i32| -> i32
        {
            let perm_write = Permissions { read: false, write: true, execute: false, delegate: false };
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("export 'memory' manquant");
            let data_len = memory.data_size(&caller);
            let rptr = resource_ptr as usize;
            let rlen = resource_len as usize;
            if rptr.checked_add(rlen).map_or(true, |end| end > data_len) {
                return -2;
            }
            let mut res_bytes = vec![0u8; rlen];
            memory.read(&caller, rptr, &mut res_bytes).expect("agent_store_put: lecture resource");
            let resource = match std::str::from_utf8(&res_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => return -2,
            };

            let vptr = val_ptr as usize;
            let vlen = val_len as usize;
            if vptr.checked_add(vlen).map_or(true, |end| end > data_len) {
                return -2;
            }
            let mut value = vec![0u8; vlen];
            memory.read(&caller, vptr, &mut value).expect("agent_store_put: lecture value");

            let (agent_id, cap_store_ref) = {
                let s = caller.data();
                (s.id, s.cap_store.clone())
            };

            let allowed = lock_or_recover(&cap_store_ref)
                .check(&agent_id, cap_id as u64, &resource, &perm_write);

            if !allowed {
                emit_cap_denied(&mut caller, cap_id as u64, &resource, 0x02 /* write */);
                return -1;
            }

            lock_or_recover(&caller.data().kv_store).insert(resource, value);
            0
        })?;

        linker.func_wrap("env", "emit", |mut caller: Caller<AgentState>, emit_type: i32, ptr: i32, len: i32| {
            if caller.data().termination_requested { return; } // M2 : pas d'émission post-mortem
            let (pending, agent_id, log_ref) = {
                let s = caller.data_mut();
                // Si commit_barrier a déjà enregistré une erreur I/O, court-circuiter
                // pour ne pas écraser le message d'origine avec "pending_commit absent".
                if s.host_error.is_some() { return; }
                // M4 (revue sécurité) : H-cb-correct s'exprime sur `pending_commit` (source UNIQUE).
                // L'ancien drapeau `barrier_fired` était redondant et se désynchronisait sur les
                // chemins de fin de cycle sans emit (request_validation/terminate post-barrier) →
                // crash ProcessFailed au cycle suivant. `pending_commit` est posé/consommé au même
                // endroit, il ne peut pas se désynchroniser.
                debug_assert!(s.pending_commit.is_some(), "H-cb-correct violation: emit sans commit_barrier préalable");
                let pending = match s.pending_commit.take() {
                    Some(p) => p,
                    None => { s.host_error = Some("pending_commit absent après commit_barrier".into()); return; }
                };
                (pending, s.id, s.log_ref.clone())
            };

            // S6 : horloge substituable. now_us est inséré dans EmitEnvelope.ts_us
            // (→ msgpack → LogEntry.emit_payload → action_id).
            let now_us = caller.data().clock.now_us();

            let payload_bytes = {
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .expect("export 'memory' manquant");
                let data = memory.data(&caller);
                let p = ptr as usize;
                let l = len as usize;
                // S4 : un payload hors bornes produirait un LogEntry silencieusement tronqué.
                if p.checked_add(l).map_or(true, |end| end > data.len()) {
                    caller.data_mut().host_error = Some(
                        format!("emit: ptr+len hors bornes WASM ({p}+{l} > {})", data.len())
                    );
                    return;
                }
                data[p..p + l].to_vec()
            };

            let emit_t = EmitType::try_from(emit_type as u8).unwrap_or(EmitType::ActionResult);
            let envelope = EmitEnvelope::new(emit_t, agent_id, pending.seq, now_us, payload_bytes);

            let entry = LogEntry {
                agent_id,
                ts_ms: pending.ts_ms,
                parent_ids: pending.parent_ids,
                hash_before: pending.hash_before,
                hash_after: pending.hash_after,
                emit_payload: Some(envelope.to_msgpack()),
            };
            match log_ref.append(&entry) {
                Ok(action_id) => { caller.data_mut().last_action = Some(action_id); }
                Err(e) => { caller.data_mut().host_error = Some(format!("CausalLog::append: {e}")); }
            }

            // SEF-4 CrashPoint #4 — après `CausalLog::append` réussi. La transaction
            // de l'action en cours est entièrement persistée (ContentStore + log).
            // État post-recovery attendu : `hash_ref_pre[k+1]` partout.
            #[cfg(feature = "crash-injection")]
            crate::crash_point::fire(crate::crash_point::CrashPoint::CommitBarrierPostLogAppend);
        })?;

        // ADR-0019 T5 — host function agent_infer (async, 3 phases).
        // Enregistrée seulement si infer_fn est fourni ; les modules sans import agent_infer
        // (AGENT_WAT, INTROSPECT_AGENT_WAT, etc.) s'instancient sans erreur.
        if let Some(infer_fn) = infer_fn {
            linker.func_wrap_async(
                "env",
                "agent_infer",
                move |mut caller: Caller<AgentState>,
                      (prompt_ptr, prompt_len, response_buf_ptr, response_buf_cap,
                       response_len_out_ptr, timeout_ms_req): (i32, i32, i32, i32, i32, i32)| {
                    let infer_fn = std::sync::Arc::clone(&infer_fn);
                    Box::new(async move {
                        // ── Phase 1 : sync-read ──────────────────────────────────
                        let memory = caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                            .expect("agent_infer: memory export");
                        let data_len = memory.data_size(&caller);
                        let ptr = prompt_ptr as usize;
                        let plen = prompt_len as usize;
                        if ptr.checked_add(plen).map_or(true, |end| end > data_len) {
                            let _ = memory.write(&mut caller, response_len_out_ptr as usize,
                                                 &0u32.to_le_bytes());
                            return 2i32; // Error: prompt out of bounds
                        }
                        // Valider les pointeurs de sortie dès Phase 1 pour garantir que
                        // Phase 3 peut écrire sans erreur silencieuse (spec/08 §1, W3).
                        // Un échec ici n'émet aucun log (pas d'InferenceRequest ni Response).
                        let rlen_end = (response_len_out_ptr as usize).checked_add(4);
                        let rbuf_end = (response_buf_ptr as usize).checked_add(response_buf_cap as usize);
                        match (rlen_end, rbuf_end) {
                            (Some(le), Some(be)) if le <= data_len && be <= data_len => {}
                            _ => return 2i32, // Error: output pointer out of bounds
                        }
                        let mut prompt = vec![0u8; plen];
                        memory.read(&caller, ptr, &mut prompt)
                            .expect("agent_infer: read prompt");

                        let (agent_id, seq, last_action, last_snapshot, log_ref) = {
                            let s = caller.data();
                            (s.id, s.seq, s.last_action, s.last_snapshot, s.log_ref.clone())
                        };

                        let timeout_eff = (timeout_ms_req as u32)
                            .min(HOST_MAX_INFERENCE_DURATION_MS);

                        // ── Pré-calcul pour le payload 0x0C ───────────────────────
                        let mut h = Sha256::new(); h.update(&prompt);
                        let prompt_hash: [u8; 32] = h.finalize().into();
                        let model_id: &[u8] = b"sleepy";
                        // S6 : horloge substituable. Le ts_ms rentre dans LogEntry.ts_ms
                        // (InferenceRequest 0x0C). Le résultat de l'inférence lui-même
                        // reste non-déterministe (backend stochastique) — hors P5.
                        let ts_0c_before = caller.data().clock.now_ms();
                        let hash_0c = last_snapshot.unwrap_or([0u8; 32]);
                        let parents_0c: Vec<[u8;32]> = last_action.into_iter().collect();

                        caller.data_mut().log_lifecycle_event(LifecycleState::WaitingInference);
                        // INVARIANT : la restauration vers Active (ligne ci-dessous phase 3)
                        // repose sur l'absence d'annulation externe de la task Tokio. Si une
                        // primitive Scheduler::abort() est introduite, il faudra émettre un
                        // Lifecycle{Terminated} dans le Drop de AgentState via log_ref clone.

                        // ── Phase 2 : async ──────────────────────────────────────
                        let t0 = std::time::Instant::now();
                        let result = infer_fn(agent_id, prompt, timeout_eff).await;
                        let duration_ms = t0.elapsed().as_millis() as u32;

                        // ── Phase 3 : sync-write ─────────────────────────────────
                        // S6 : horloge substituable pour le ts_ms post-inférence.
                        // `duration_ms` (Instant::now() elapsed) reste non-déterministe — il
                        // capture du temps wall-clock et inscrit cette mesure dans le payload
                        // InferenceResponse. SEF-6 n'applique pas à agent_infer pour cette raison.
                        let now_resp = caller.data().clock.now_ms();
                        let (last_action2, last_snapshot2) = {
                            let s = caller.data();
                            (s.last_action, s.last_snapshot)
                        };
                        let parents_resp: Vec<[u8;32]> = last_action2.into_iter().collect();
                        let hash_resp = last_snapshot2.unwrap_or([0u8; 32]);

                        // ── Log InferenceRequest (0x0C) avec enrichissement ADR-0022 ──
                        // Émis après l'appel infer_fn pour inclure slot_info (priority_class,
                        // queue_depth_at_admission, promoted_from). Payload format :
                        //   prompt_hash(32) | model_id_len(1) | model_id(N) |
                        //   timeout_req(4 LE) | timeout_eff(4 LE) |
                        //   priority_class(1) | queue_depth(2 LE) | promoted_from(1)
                        {
                            let slot_info_opt = match &result {
                                Ok(r) => r.slot_info.as_ref(),
                                Err(_) => None,
                            };
                            let mut pl_0c: Vec<u8> = Vec::new();
                            pl_0c.extend_from_slice(&prompt_hash);
                            pl_0c.push(model_id.len() as u8);
                            pl_0c.extend_from_slice(model_id);
                            pl_0c.extend_from_slice(&(timeout_ms_req as u32).to_le_bytes());
                            pl_0c.extend_from_slice(&timeout_eff.to_le_bytes());
                            // Enrichissement ADR-0022 (bytes additionnels, rétro-compatibles).
                            if let Some(si) = slot_info_opt {
                                pl_0c.push(si.priority_class as u8);
                                pl_0c.extend_from_slice(&si.queue_depth_at_admission.to_le_bytes());
                                pl_0c.push(si.promoted_from.map(|c| c as u8).unwrap_or(0xFF));
                            }
                            let env = EmitEnvelope::new(EmitType::InferenceRequest,
                                agent_id, seq, ts_0c_before * 1000, pl_0c);
                            let entry = LogEntry { agent_id, ts_ms: ts_0c_before,
                                parent_ids: parents_0c, hash_before: hash_0c, hash_after: hash_0c,
                                emit_payload: Some(env.to_msgpack()) };
                            if let Ok(aid) = log_ref.append(&entry) {
                                caller.data_mut().last_action = Some(aid);
                            }
                        }

                        let ret_code: i32 = match result {
                            Ok(resp) => {
                                let bytes = resp.text.as_bytes();
                                let cap = response_buf_cap as usize;
                                let truncated = bytes.len() > cap;
                                let len_w = bytes.len().min(cap);
                                let mem2 = caller.get_export("memory")
                                    .and_then(|e| e.into_memory()).unwrap();
                                // Pointeurs validés en Phase 1 — expect() légitime.
                                mem2.write(&mut caller, response_buf_ptr as usize, &bytes[..len_w])
                                    .expect("bounds vérifiées en Phase 1");
                                mem2.write(&mut caller, response_len_out_ptr as usize,
                                    &(len_w as u32).to_le_bytes())
                                    .expect("bounds vérifiées en Phase 1");

                                let mut hh = Sha256::new();
                                hh.update(resp.text.as_bytes());
                                let rh: [u8;32] = hh.finalize().into();
                                let mut pl_0d = Vec::with_capacity(41);
                                pl_0d.extend_from_slice(&rh);
                                pl_0d.extend_from_slice(&0u32.to_le_bytes()); // tokens_est
                                pl_0d.extend_from_slice(&duration_ms.to_le_bytes());
                                pl_0d.push(if truncated { 1u8 } else { 0u8 });
                                let env = EmitEnvelope::new(EmitType::InferenceResponse,
                                    agent_id, seq, now_resp * 1000, pl_0d);
                                let entry = LogEntry { agent_id, ts_ms: now_resp,
                                    parent_ids: parents_resp, hash_before: hash_resp,
                                    hash_after: hash_resp,
                                    emit_payload: Some(env.to_msgpack()) };
                                if let Ok(aid) = log_ref.append(&entry) {
                                    caller.data_mut().last_action = Some(aid);
                                }
                                0
                            }
                            Err(InferError::Cancelled) => {
                                let mem2 = caller.get_export("memory")
                                    .and_then(|e| e.into_memory()).unwrap();
                                mem2.write(&mut caller, response_len_out_ptr as usize, &0u32.to_le_bytes())
                                    .expect("bounds vérifiées en Phase 1");
                                let mut pl_0e = Vec::with_capacity(9);
                                pl_0e.extend_from_slice(&now_resp.to_le_bytes());
                                pl_0e.push(0x01u8); // cause = Rollback
                                let env = EmitEnvelope::new(EmitType::InferenceCancelled,
                                    agent_id, seq, now_resp * 1000, pl_0e);
                                let entry = LogEntry { agent_id, ts_ms: now_resp,
                                    parent_ids: parents_resp, hash_before: hash_resp,
                                    hash_after: hash_resp,
                                    emit_payload: Some(env.to_msgpack()) };
                                // ADR-0027 : append() non-durable suffit pour 0x0E. Sous SIGKILL/
                                // panic le WAL OS-buffered survit ; sous power-loss (hors scope
                                // Phase 6), perte possible mais symétrique à 0x11/0x0B/0x12 du
                                // même chemin de compensation — pas de mode mixte pathologique.
                                // La cancellation Tokio n'écrit rien dans le ContentStore : P6
                                // tient via état persistant inchangé.
                                if let Ok(aid) = log_ref.append(&entry) {
                                    caller.data_mut().last_action = Some(aid);
                                }
                                4
                            }
                            Err(e) => {
                                let mem2 = caller.get_export("memory")
                                    .and_then(|e| e.into_memory()).unwrap();
                                mem2.write(&mut caller, response_len_out_ptr as usize, &0u32.to_le_bytes())
                                    .expect("bounds vérifiées en Phase 1");
                                let (error_code, msg) = match &e {
                                    InferError::Timeout =>
                                        (0x01u8, "timeout".to_string()),
                                    InferError::BackendError { code, message } =>
                                        (*code, message.clone()),
                                    InferError::NoSlot =>
                                        (0x20u8, "no slot".to_string()),
                                    InferError::Cancelled => unreachable!(),
                                };
                                let mb = msg.as_bytes();
                                let ml = mb.len().min(255);
                                let mut pl_0f = Vec::with_capacity(2 + ml);
                                pl_0f.push(error_code);
                                pl_0f.push(ml as u8);
                                pl_0f.extend_from_slice(&mb[..ml]);
                                let env = EmitEnvelope::new(EmitType::InferenceFailed,
                                    agent_id, seq, now_resp * 1000, pl_0f);
                                let entry = LogEntry { agent_id, ts_ms: now_resp,
                                    parent_ids: parents_resp, hash_before: hash_resp,
                                    hash_after: hash_resp,
                                    emit_payload: Some(env.to_msgpack()) };
                                if let Ok(aid) = log_ref.append(&entry) {
                                    caller.data_mut().last_action = Some(aid);
                                }
                                match e {
                                    InferError::Timeout => 1,
                                    InferError::NoSlot  => 3,
                                    _                   => 2,
                                }
                            }
                        };

                        caller.data_mut().log_lifecycle_event(LifecycleState::Active);
                        ret_code
                    }) as Box<dyn std::future::Future<Output = i32> + Send + '_>
                },
            )?;
        }

        let instance = linker.instantiate_async(&mut wasm_store, &module).await?;
        let memory = instance
            .get_memory(&mut wasm_store, "memory")
            .ok_or_else(|| wasmtime::Error::msg("export 'memory' manquant dans le module WASM"))?;
        let process_fn =
            instance.get_typed_func::<(i32, i32), ()>(&mut wasm_store, "process")?;

        Ok(Self { store: wasm_store, memory, process_fn })
    }

    /// Traite un message : copie `data` dans la mémoire WASM, appelle `process(0, len)`.
    /// Déclenche commit_barrier + emit via les host functions.
    /// Si commit_barrier s'est déclenché sans emit suivant, flush avec emit_payload = None.
    pub async fn process_one(&mut self, data: &[u8]) -> Result<(), RuntimeError> {
        const MAX_MSG: usize = 65_536; // une page WASM = 64 KiB
        if data.len() > MAX_MSG {
            return Err(RuntimeError::MessageTooLarge(data.len()));
        }
        self.memory
            .write(&mut self.store, 0, data)
            .map_err(|_| RuntimeError::MemoryOutOfBounds)?;
        self.process_fn.call_async(&mut self.store, (0, data.len() as i32)).await?;

        // Propagation des erreurs I/O stockées par commit_barrier ou emit (Point 1 / ADR-0015).
        if let Some(err) = self.store.data_mut().host_error.take() {
            return Err(RuntimeError::Wasmtime(wasmtime::Error::msg(err)));
        }

        if let Some(pending) = self.store.data_mut().pending_commit.take() {
            let (log_ref, agent_id) = {
                let s = self.store.data();
                (s.log_ref.clone(), s.id)
            };
            let entry = LogEntry {
                agent_id,
                ts_ms: pending.ts_ms,
                parent_ids: pending.parent_ids,
                hash_before: pending.hash_before,
                hash_after: pending.hash_after,
                emit_payload: None,
            };
            match log_ref.append(&entry) {
                Ok(action_id) => { self.store.data_mut().last_action = Some(action_id); }
                Err(e) => { return Err(RuntimeError::Wasmtime(wasmtime::Error::msg(format!("CausalLog::append pending: {e}")))); }
            }
        }

        // ADR-0012 : vérifie les bornes de session après chaque action.
        // D7 : `session_max_duration_ms == 0` désactive la borne durée (convention),
        // évite ainsi une boucle de sessions vides au premier `process_one`
        // (où `elapsed == 0` car `session_started_at_ms` vient d'être initialisé).
        {
            let s = self.store.data();
            let actions = s.seq - s.session_start_seq;
            let elapsed = if s.session_started_at_ms > 0 {
                s.clock.now_ms().saturating_sub(s.session_started_at_ms)
            } else {
                0
            };
            let max_actions = s.session_max_actions;
            let max_duration = s.session_max_duration_ms;
            let _ = s;
            let actions_exceeded = actions >= max_actions;
            let duration_exceeded = max_duration > 0 && elapsed >= max_duration;
            if actions_exceeded || duration_exceeded {
                self.store.data_mut().log_session_boundary();
            }
        }

        Ok(())
    }

    pub fn agent_id(&self) -> AgentId {
        self.store.data().id
    }

    /// Tenant propriétaire (ADR-0057, MT-1). `TenantId::DEFAULT` en mono-tenant.
    pub fn tenant(&self) -> TenantId {
        self.store.data().tenant
    }

    /// M1 (revue sécurité) — identité du `CapabilityStore` de cet agent (pointeur de l'`Arc`),
    /// pour le garde d'isolation de câblage du `Registry` : deux tenants distincts ne doivent
    /// jamais partager le même `cap_store` (sinon fuite d'autorité cross-tenant non détectée).
    pub fn cap_store_ptr(&self) -> usize {
        Arc::as_ptr(&self.store.data().cap_store) as *const () as usize
    }

    pub fn seq(&self) -> u64 {
        self.store.data().seq
    }

    pub fn lifecycle(&self) -> LifecycleState {
        self.store.data().lifecycle
    }

    pub fn last_action(&self) -> Option<[u8; 32]> {
        self.store.data().last_action
    }

    pub fn last_snapshot(&self) -> Option<[u8; 32]> {
        self.store.data().last_snapshot
    }

    pub fn last_verdict(&self) -> Option<ValidationVerdict> {
        self.store.data().last_verdict
    }

    pub fn session_id(&self) -> u64 {
        self.store.data().session_id
    }

    pub fn session_action_count(&self) -> u64 {
        let s = self.store.data();
        s.seq - s.session_start_seq
    }

    /// Accès direct à AgentState pour les tests (transitions manuelles).
    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut AgentState {
        self.store.data_mut()
    }

    /// ADR-0015 D15.2-b — Renseigne le `parent_agent_id` de l'agent.
    /// Appelé par `Scheduler::spawn_child` avant `register`. Idempotent : un second
    /// appel écrase l'éventuel parent précédent (utile aux tests).
    pub fn set_parent_agent_id(&mut self, parent: AgentId) {
        self.store.data_mut().parent_agent_id = Some(parent);
    }

    // ── Helpers privés partagés par les deux chemins de restauration ──────────

    /// Fail-safe #7a (ADR-0051 §D3, P6) — source unique de vérité.
    /// Vérifie que le snapshot du point de reprise est présent dans le store autoritaire.
    /// Appelé par restore_from_evicted ET restore_from_evicted_with_inference_and_profile :
    /// les deux chemins partagent exactement la même vérification, sans copie.
    async fn check_resume_snapshot(
        evicted:   &EvictedState,
        store_ref: &Arc<ContentStore>,
    ) -> Result<(), RuntimeError> {
        if let Some(snap) = evicted.last_snapshot {
            let store = store_ref.clone();
            let present = tokio::task::spawn_blocking(move || store.has_snapshot(&snap))
                .await
                .map_err(|e| RuntimeError::Wasmtime(wasmtime::Error::msg(format!("spawn_blocking: {e}"))))?
                ?;
            if !present {
                return Err(RuntimeError::Store(StoreError::MissingBlock(snap)));
            }
        }
        Ok(())
    }

    /// Copie les champs causaux depuis un EvictedState vers une instance fraîche.
    fn copy_evicted_fields(instance: &mut Self, evicted: &EvictedState) {
        let st = instance.store.data_mut();
        st.seq           = evicted.seq;
        st.last_snapshot = evicted.last_snapshot;
        st.last_action   = evicted.last_action;
    }

    // ── API publique de restauration ──────────────────────────────────────────

    /// Reconstruit un acteur depuis un `EvictedState` (ADR-0030 §FutureWork).
    ///
    /// Identique à `new_precompiled` sauf que `AgentState.{seq, last_snapshot, last_action}`
    /// sont restaurés depuis l'état évincé. Le premier message reçu reprendra donc la
    /// causalité exactement là où l'agent s'était arrêté.
    ///
    /// ADR-0051 §D3 (correctif #7a, P6 fail-safe) : vérifie que `last_snapshot` existe
    /// dans le ContentStore avant de reprendre — fail-loud plutôt que silencieux.
    ///
    /// Pour les modules qui importent `agent_infer`, utiliser
    /// `restore_from_evicted_with_inference_and_profile`.
    pub async fn restore_from_evicted(
        engine:     &Engine,
        module:     &Module,
        evicted:    &EvictedState,
        store_ref:  Arc<ContentStore>,
        log_ref:    Arc<CausalLog>,
    ) -> Result<Self, RuntimeError> {
        Self::check_resume_snapshot(evicted, &store_ref).await?;
        // ADR-0062 Q3 : réhydratation = build neuf (via le builder canonique) + copie des
        // champs causaux. Les deux variantes `restore_*` passent explicitement par
        // `ActorInstanceBuilder` (uniformité ; le réveil reste un chemin runtime, hors loader).
        let mut instance = ActorInstanceBuilder::new(engine, module, evicted.id, store_ref, log_ref)
            .build()
            .await?;
        Self::copy_evicted_fields(&mut instance, evicted);
        Ok(instance)
    }

    /// Reconstruit un acteur avec inference depuis un `EvictedState`.
    ///
    /// Variante de `restore_from_evicted` pour les modules qui importent `agent_infer`
    /// (ex: `multi_turn.wasm`, agents Rust compilés). Câble la host function et restaure
    /// les champs causaux avec le même fail-safe #7a que `restore_from_evicted`.
    ///
    /// Le profil et l'`infer_fn` sont fournis explicitement — pas d'héritage depuis
    /// l'instance originale (les capabilities et la priorité sont réassignées par le
    /// runner selon le rôle de l'agent réveillé, décision architect 2026-05-31).
    pub async fn restore_from_evicted_with_inference_and_profile(
        engine:    &Engine,
        module:    &Module,
        evicted:   &EvictedState,
        store_ref: Arc<ContentStore>,
        log_ref:   Arc<CausalLog>,
        infer_fn:  InferFn,
        profile:   AgentProfile,
    ) -> Result<Self, RuntimeError> {
        Self::check_resume_snapshot(evicted, &store_ref).await?;
        let mut instance = ActorInstanceBuilder::new(engine, module, evicted.id, store_ref, log_ref)
            .inference(infer_fn)
            .profile(profile)
            .build()
            .await?;
        Self::copy_evicted_fields(&mut instance, evicted);
        Ok(instance)
    }

    /// SEF-3 / P1 (tests uniquement) — Lecture directe de la mémoire WASM de l'agent.
    /// Retourne `len` bytes à partir de l'offset `offset`. Panique si hors bornes.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn read_memory_at(&self, offset: usize, len: usize) -> Vec<u8> {
        let data = self.memory.data(&self.store);
        data[offset..offset + len].to_vec()
    }
}

/// ADR-0015 D15.2-a — Classifie une `RuntimeError` issue de `process_one`
/// pour distinguer un trap watchdog (epoch_interruption, ADR-0025) d'un échec
/// `process_one` ordinaire. Le trap watchdog est exposé par wasmtime comme un
/// `wasmtime::Trap::Interrupt` attaché à l'erreur (downcastable via anyhow).
///
/// Retourne `CrashCause::WatchdogTrap` si l'erreur contient un `Trap::Interrupt`,
/// sinon `CrashCause::ProcessFailed` (cas par défaut couvrant tous les autres
/// chemins : `MessageTooLarge`, `MemoryOutOfBounds`, autres traps WASM, erreurs
/// `Store`/`Log` remontées). Les variants non-Wasmtime n'ont pas de trap attaché.
fn classify_process_one_error(err: &RuntimeError) -> CrashCause {
    if let RuntimeError::Wasmtime(e) = err {
        if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
            if *trap == wasmtime::Trap::Interrupt {
                return CrashCause::WatchdogTrap;
            }
        }
    }
    CrashCause::ProcessFailed
}

pub async fn run_loop(
    mut instance: ActorInstance,
    mut inbox: tokio::sync::mpsc::Receiver<Message>,
) {
    // A4 : transition Spawned → enregistrée au démarrage de la task.
    instance.store.data_mut().log_lifecycle_event(LifecycleState::Spawned);

    // BF-2 (ADR-0058 §D6) / XR-1 (ADR-0060) — à la terminaison de l'agent (canal fermé, crash,
    // return ou panic), révoquer tous les CauseHandle qu'il a ÉMIS : un handle accordé par un
    // agent mort ne doit plus autoriser de citation. Le garde Drop couvre tous les chemins de
    // sortie de `run_loop`. **Balayage cross-tenant (XR-1) :** la révocation porte sur TOUS les
    // stores du registre, pas seulement celui du tenant de l'agent — un handle émis par cet
    // agent au profit d'un grantee d'un AUTRE tenant vit dans le store de cet autre tenant et
    // doit aussi être révoqué (dette cross-tenant ADR-0058 §D6 close). Le garde porte une réf
    // au registre PARTAGÉ (même classe d'objet que store/log partagés) — il n'accède jamais au
    // Scheduler (frontière ADR-0058 §D2 / jurisprudence run_loop ADR-0014 §D14.b préservées).
    struct IssuedHandleRevoker {
        registry: Arc<CauseHandleRegistry>,
        agent_id: AgentId,
    }
    impl Drop for IssuedHandleRevoker {
        fn drop(&mut self) {
            self.registry.revoke_issued_by_all(&self.agent_id);
        }
    }
    let _issued_revoker = IssuedHandleRevoker {
        registry: instance.store.data().cause_handle_registry.clone(),
        agent_id: instance.agent_id(),
    };

    // Message différé depuis la boucle AwaitingValidation (ex. Rollback reçu pendant
    // l'attente d'un verdict A3 — annule la validation, retraite le Rollback normalement).
    let mut pending_msg: Option<Message> = None;
    loop {
        let msg = if let Some(m) = pending_msg.take() {
            m
        } else {
            match inbox.recv().await {
                Some(m) => m,
                None => break,
            }
        };
        match msg {
            Message::Data { payload, cause } => {
                // ADR-0025 : réarme le watchdog selon le profil de l'agent.
                // No-op si l'engine n'a pas epoch_interruption (ex. benchmarks, Engine::default()).
                {
                    let max_ticks = instance.store.data().agent_profile.max_ticks();
                    instance.store.set_epoch_deadline(max_ticks);
                }
                // A4 : transition → Active avant chaque cycle de traitement.
                instance.store.data_mut().log_lifecycle_event(LifecycleState::Active);
                // ADR-0003 : injecte la cause cross-agent si présente (causalité implicite).
                if let Some(cause_id) = cause {
                    instance.store.data_mut().pending_extra_causes.push(cause_id);
                }
                if let Err(e) = instance.process_one(&payload).await {
                    let cause = classify_process_one_error(&e);
                    tracing::error!(
                        agent = ?instance.agent_id(),
                        error = %e,
                        cause = ?cause,
                        "process_one failed — arrêt de l'acteur"
                    );
                    // ADR-0015 D15.2-a / D-Q-V2.2 : log_agent_crash fixe lifecycle=Terminated
                    // atomiquement (un seul append RocksDB). Ne pas appeler log_lifecycle_event.
                    instance.store.data_mut().log_agent_crash(cause);
                    return;
                }
                // A3 : si l'agent attend un verdict (request_validation), bloquer jusqu'à réception
                // OU jusqu'à expiration du timeout (ADR-0014 D14.b — `tokio::time::timeout`).
                if instance.store.data().lifecycle == LifecycleState::AwaitingValidation {
                    let timeout_ms = instance.store.data().validation_timeout_ms;
                    let timeout_dur = std::time::Duration::from_millis(timeout_ms);
                    // Échéance absolue : la mesure court depuis l'entrée en `AwaitingValidation`
                    // et n'est pas réinitialisée par les messages ignorés (D14.a — timer absolu).
                    let deadline = tokio::time::Instant::now() + timeout_dur;
                    loop {
                        match tokio::time::timeout_at(deadline, inbox.recv()).await {
                            Ok(Some(Message::ValidationResponse { verdict })) => {
                                instance.store.data_mut().record_validation_response(verdict);
                                break;
                            }
                            Ok(Some(Message::Suspend)) => {
                                // Suspension externe pendant l'attente → on respecte.
                                return;
                            }
                            Ok(Some(rollback @ Message::Rollback { .. })) => {
                                // Rollback superviseur pendant AwaitingValidation : annuler la
                                // demande de validation, remettre le Rollback en pending pour
                                // qu'il soit traité dans le prochain tour de la boucle principale.
                                instance.store.data_mut().record_validation_response(ValidationVerdict::Cancelled);
                                pending_msg = Some(rollback);
                                break;
                            }
                            Ok(None) => return, // inbox fermée
                            Ok(_) => {} // autres messages ignorés pendant l'attente de validation
                            Err(_) => {
                                // ADR-0014 D14.b — timeout : `run_loop` injecte directement
                                // le verdict `Timeout` sans router via Message::ValidationResponse.
                                // L'événement est observable via le log causal en filtrant
                                // EmitType::ValidationResponse avec verdict == 2 (D14.d).
                                tracing::info!(
                                    agent = ?instance.agent_id(),
                                    timeout_ms,
                                    "A3 timeout sur AwaitingValidation — verdict Timeout injecté (ADR-0014)"
                                );
                                instance.store.data_mut().record_validation_response(ValidationVerdict::Timeout);
                                break;
                            }
                        }
                    }
                }
                // Si l'agent a demandé terminate depuis WASM, on respecte.
                if instance.store.data().lifecycle == LifecycleState::Terminated {
                    return;
                }
            }
            Message::Suspend => {
                // A4 : transition → Suspended.
                instance.store.data_mut().log_lifecycle_event(LifecycleState::Suspended);
                break;
            }
            Message::Checkpoint => {
                // A4 : transition → Checkpointed (superviseur ou scheduler).
                instance.store.data_mut().log_lifecycle_event(LifecycleState::Checkpointed);
                // L'acteur reste dans l'inbox — il peut être resumé par Data.
            }
            Message::Evict { reply } => {
                // Éviction propre : log Suspended, renvoie l'état minimal, se termine.
                // Le scheduler stocke l'EvictedState dans sa table dormant et drop l'instance
                // (libère la mémoire WASM). Le ContentStore conserve tous les snapshots.
                instance.store.data_mut().log_lifecycle_event(LifecycleState::Suspended);
                let st = instance.store.data();
                let evicted = EvictedState {
                    id:            st.id,
                    seq:           st.seq,
                    last_snapshot: st.last_snapshot,
                    last_action:   st.last_action,
                    // evicted_at sera mis à jour par Scheduler::evict_agent juste après
                    // (ADR-0031 §D4 : capturé dans le scheduler, pas dans l'actor).
                    evicted_at:    std::time::Instant::now(),
                };
                // Erreur d'envoi ignorée : le caller peut avoir disparu.
                let _ = reply.send(evicted);
                return;
            }
            Message::Rollback { target_seq } => {
                let (seq, last_snapshot, last_action, store_ref, log_ref, agent_id, cap_store, cause_handle_registry) = {
                    let s = instance.store.data();
                    (s.seq, s.last_snapshot, s.last_action, s.store_ref.clone(), s.log_ref.clone(), s.id, s.cap_store.clone(), s.cause_handle_registry.clone())
                };

                // Validation : aucun historique → noop.
                let tip = match last_snapshot {
                    None => {
                        tracing::warn!(agent = ?agent_id, "Rollback ignoré : aucun historique");
                        continue;
                    }
                    Some(h) => h,
                };

                // Validation : target_seq hors de l'historique disponible → noop.
                if target_seq >= seq {
                    tracing::warn!(
                        agent = ?agent_id,
                        target_seq,
                        current_seq = seq,
                        "Rollback ignoré : target_seq hors de l'historique"
                    );
                    continue;
                }

                let path = match store_ref.rollback_path(&tip, target_seq) {
                    Ok(p) => p,
                    Err(e) => {
                        // Chaîne ContentStore brisée = corruption ; on ne peut pas continuer.
                        tracing::error!(agent = ?agent_id, error = %e, "Rollback : chaîne ContentStore brisée");
                        // ADR-0015 D15.2-a / D-Q-V2.2 : log_agent_crash fixe lifecycle=Terminated atomiquement.
                        instance.store.data_mut().log_agent_crash(CrashCause::ContentStoreBroken);
                        return;
                    }
                };

                let target_snap = *path.last().expect("rollback_path retourne un vecteur non-vide");
                let distance = (path.len().saturating_sub(1)).min(255) as u8;

                // S6 : horloge substituable.
                let (now_ms, now_us) = {
                    let s = instance.store.data();
                    (s.clock.now_ms(), s.clock.now_us())
                };

                // D8 / ADR-0007 — Révocation des caps de l'agent émises après le snapshot cible.
                // Source du timestamp : header du snapshot cible (ts_us → ms, même horloge
                // que `Capability.issued_at_ms`). Lookup O(1) RocksDB ; la chaîne de path
                // vient de toucher l'entrée, donc cache chaud.
                let caps_invalidated: u8 = match store_ref.get_header(&target_snap) {
                    Ok(Some(header)) => {
                        let target_ts_ms = header.ts_us / 1000;
                        let revoked = lock_or_recover(&cap_store)
                            .revoke_owned_after(&agent_id, target_ts_ms);
                        // ADR-0058 §D7 (BF-2) / XR-1 (ADR-0060) — symétrie sur les CauseHandle :
                        // on révoque ceux ÉMIS par cet agent après le snapshot cible (et non
                        // « détenus », cf. caps). Un handle accordé par A après un état que A
                        // annule perd son référent. **Balayage cross-tenant :** sur TOUS les
                        // stores du registre (un handle émis au profit d'un grantee d'un autre
                        // tenant vit dans le store de cet autre tenant).
                        cause_handle_registry.revoke_issued_after_all(&agent_id, target_ts_ms);
                        revoked.min(255) as u8
                    }
                    Ok(None) => {
                        // Snapshot cible introuvable malgré rollback_path : incohérence.
                        // On log mais on n'avorte pas le rollback déjà accompli.
                        tracing::warn!(
                            agent = ?agent_id,
                            target_snap = ?target_snap,
                            "Rollback : header du snapshot cible introuvable, caps non révoquées"
                        );
                        0
                    }
                    Err(e) => {
                        tracing::warn!(
                            agent = ?agent_id,
                            error = %e,
                            "Rollback : erreur lookup header cible, caps non révoquées"
                        );
                        0
                    }
                };

                let parent_ids: Vec<[u8; 32]> = last_action.into_iter().collect();
                // Payload : [distance u8 | target_seq u64 LE | caps_invalidated u8]
                // D8 (2026-05-15) : caps_invalidated reflète maintenant le nombre réel de
                // caps de l'agent révoquées (ADR-0007). Clamp à 255 par construction.
                let mut payload = [0u8; 10];
                payload[0] = distance;
                payload[1..9].copy_from_slice(&target_seq.to_le_bytes());
                payload[9] = caps_invalidated;

                let envelope = EmitEnvelope::new(
                    EmitType::SchedulerRollback,
                    agent_id,
                    seq,
                    now_us,
                    payload.to_vec(),
                );
                let entry = LogEntry {
                    agent_id,
                    ts_ms: now_ms,
                    parent_ids,
                    hash_before: tip,
                    hash_after: target_snap,
                    emit_payload: Some(envelope.to_msgpack()),
                };

                // ADR-0027 : append() non-durable. C'est le site « commit barrier de rollback »
                // (`hash_after = target_snap`), mais le rollback n'écrit RIEN dans le
                // ContentStore — `rollback_path` est un lookup de chaîne existante,
                // `revoke_owned_after` mute CapabilityStore in-memory. Perdre 0x0B sous
                // power-loss équivaut donc à « rollback jamais appliqué » : au redémarrage
                // l'agent repart de `last_snapshot` antérieur (= tip), exactement l'état
                // « avant transaction » de P6. Cf. ADR-0027 §Justification cas C.
                if let Ok(action_id) = log_ref.append(&entry) {
                    let s = instance.store.data_mut();
                    s.last_snapshot = Some(target_snap);
                    s.last_action = Some(action_id);
                }

                // Lifecycle explicite : un superviseur lisant le log voit l'agent reprendre.
                instance.store.data_mut().log_lifecycle_event(LifecycleState::Active);
            }
            Message::ValidationResponse { .. } => {
                // Réponse inattendue hors contexte de validation — ignorée.
                tracing::warn!(agent = ?instance.agent_id(), "ValidationResponse reçue sans demande en cours");
            }
            Message::SessionResume { summary } => {
                // ADR-0012 : injection du résumé causal au démarrage d'une nouvelle session.
                // Le payload est délivré comme premier Data de la session.
                {
                    let max_ticks = instance.store.data().agent_profile.max_ticks();
                    instance.store.set_epoch_deadline(max_ticks);
                }
                instance.store.data_mut().log_lifecycle_event(LifecycleState::Active);
                if let Err(e) = instance.process_one(&summary).await {
                    let cause = classify_process_one_error(&e);
                    tracing::error!(
                        agent = ?instance.agent_id(),
                        error = %e,
                        cause = ?cause,
                        "SessionResume process_one failed"
                    );
                    // ADR-0015 D15.2-a / D-Q-V2.2 : log_agent_crash fixe lifecycle=Terminated atomiquement.
                    instance.store.data_mut().log_agent_crash(cause);
                    return;
                }
            }
        }
    }

    // Sortie naturelle de la boucle (inbox fermée ou Suspend).
    if instance.store.data().lifecycle != LifecycleState::Suspended
        && instance.store.data().lifecycle != LifecycleState::Terminated
    {
        instance.store.data_mut().log_lifecycle_event(LifecycleState::Terminated);
    }
}
