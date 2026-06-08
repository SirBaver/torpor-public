// agent-sdk — wrappers idiomatiques autour des host functions A1–A4.
// Compilé pour wasm32-unknown-unknown : les extern "C" deviennent des imports WASM
// dans le module "env" (cohérent avec les WAT inline de poc/runtime/src/actor.rs).
// Sur targets non-wasm32 : les wrappers sont des no-ops pour permettre `cargo check`.

/// Taille du buffer retourné par `agent_introspect` (A1).
/// Layout : [0..32] last_action_id, [32..40] seq u64 LE, [40..72] last_snapshot,
///          [72] flags (bit0=action set, bit1=snapshot set), [73] lifecycle_state.
pub const INTROSPECT_LEN: usize = 74;

/// Taille du buffer retourné par `agent_session_info`.
/// Layout : [0..8] session_id u64 LE, [8..16] action_count u64 LE, [16..24] started_at_ms u64 LE.
pub const SESSION_INFO_LEN: usize = 24;

// ── Déclarations extern "C" (wasm32 uniquement) ───────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn commit_barrier();
    fn emit(emit_type: i32, ptr: *const u8, len: i32);
    fn agent_introspect(buf_ptr: *mut u8, buf_len: i32) -> i32;
    fn agent_self_rollback(depth: i32) -> i32;
    fn agent_request_validation(risk: i32) -> i32;
    fn agent_get_verdict() -> i32;
    fn agent_checkpoint() -> i32;
    fn agent_terminate();
    fn agent_session_info(buf_ptr: *mut u8, buf_len: i32) -> i32;
    fn agent_add_cause(action_id_ptr: *const u8) -> i32;
}

// ── Wrappers publics ──────────────────────────────────────────────────────────

/// S4 : crée un snapshot ContentStore et marque la barrière avant tout `emit`.
/// Doit être appelé avant chaque `emit_*`.
#[inline]
pub fn barrier() {
    #[cfg(target_arch = "wasm32")]
    unsafe { commit_barrier() }
}

/// ADR-0010 : émet un événement dans le log causal.
/// `emit_type` : constantes définies dans causal-log (ex. 1=ActionResult, 6=Introspect).
/// Doit être appelé après `barrier()`.
#[inline]
pub fn emit_raw(emit_type: i32, data: &[u8]) {
    #[cfg(target_arch = "wasm32")]
    unsafe { emit(emit_type, data.as_ptr(), data.len() as i32) }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (emit_type, data);
}

/// RFC-0002 (famille 4) — émet une **directive de routage typée** comme `Event` (`0x03`).
///
/// Remplace l'émission de texte libre (`"escalate:…"`, `"delegate:…"`) par un format binaire **typé,
/// à vocabulaire fermé**, que le `DispatchRouter` côté host décode *fail-closed* (un `kind` inconnu →
/// refus, pas de défaut silencieux). Zéro dépendance (agent-sdk doit rester minimal) : layout manuel.
///
/// **Format de fil — doit rester synchronisé avec `os_poc_runtime::fleet::RouteDirective` (host) :**
/// ```text
///   octet 0..2 : [0xFD, 0x04]   (magic)
///   octet 2    : 0x01           (version)
///   octet 3    : kind           (1 = Escalate, 2 = Delegate)
///   octet 4..  : payload
/// ```
/// **Interdictions structurelles** (le format n'a pas de slot) : pas de cause (forgerie), pas de
/// caps (confused-deputy), pas de template (l'agent choisit l'intention, pas le code à exécuter).
/// Doit être appelé après `barrier()`.
#[inline]
pub fn emit_route_directive(kind: u8, payload: &[u8]) {
    let mut wire: Vec<u8> = Vec::with_capacity(4 + payload.len());
    wire.extend_from_slice(&[0xFD, 0x04, 0x01, kind]);
    wire.extend_from_slice(payload);
    emit_raw(0x03, &wire); // EmitType::Event
}

/// Intentions de routage famille 4 (RFC-0002) — vocabulaire **fermé** partagé host/guest.
pub mod directive_kind {
    /// `support` — escalade vers un spécialiste.
    pub const ESCALATE: u8 = 1;
    /// `orchestrate` — délégation d'une sous-question.
    pub const DELEGATE: u8 = 2;
}

/// A1 : lit l'état courant de l'agent dans `buf`.
/// Retourne le nombre d'octets écrits (INTROSPECT_LEN si succès, -1 si buf trop petit).
#[inline]
pub fn introspect(buf: &mut [u8; INTROSPECT_LEN]) -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_introspect(buf.as_mut_ptr(), INTROSPECT_LEN as i32) };
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = buf; INTROSPECT_LEN as i32 }
}

/// A2 : rollback vers le snapshot `depth` niveaux en arrière (max 3).
/// Codes retour : target_seq si succès, -1=depth hors bornes, -2=pas d'historique,
/// -3=historique insuffisant, -4=erreur store.
#[inline]
pub fn self_rollback(depth: i32) -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_self_rollback(depth) };
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = depth; -1 }
}

/// A3 : demande une validation avec niveau de risque `risk` (0=low, 1=medium, 2=high).
/// L'agent passe en AwaitingValidation. Utiliser `get_verdict()` après pour lire le résultat.
#[inline]
pub fn request_validation(risk: i32) -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_request_validation(risk) };
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = risk; 0 }
}

/// A3 : lit le dernier verdict de validation (0=Approved, 1=Rejected, 2=Timeout).
/// N'est valide qu'après avoir reçu une ValidationResponse.
#[inline]
pub fn get_verdict() -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_get_verdict() };
    #[cfg(not(target_arch = "wasm32"))]
    { 0 }
}

/// A4 : checkpoint explicite. Retourne le seq courant.
#[inline]
pub fn checkpoint() -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_checkpoint() };
    #[cfg(not(target_arch = "wasm32"))]
    { 0 }
}

/// A4 : terminate. L'agent signale sa fin — run_loop arrête le traitement.
#[inline]
pub fn terminate() {
    #[cfg(target_arch = "wasm32")]
    unsafe { agent_terminate() }
}

/// ADR-0012 : lit les informations de session courante dans `buf`.
/// Retourne le nombre d'octets écrits (SESSION_INFO_LEN si succès).
#[inline]
pub fn session_info(buf: &mut [u8; SESSION_INFO_LEN]) -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_session_info(buf.as_mut_ptr(), SESSION_INFO_LEN as i32) };
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = buf; SESSION_INFO_LEN as i32 }
}

/// ADR-0003 : ajoute une cause cross-agent au prochain commit_barrier.
/// `action_id` : 32 bytes identifiant l'action causale d'un autre agent.
/// Doit être appelé avant `barrier()`.
#[inline]
pub fn add_cause(action_id: &[u8; 32]) -> i32 {
    #[cfg(target_arch = "wasm32")]
    return unsafe { agent_add_cause(action_id.as_ptr()) };
    #[cfg(not(target_arch = "wasm32"))]
    { let _ = action_id; 0 }
}

// ── Helpers lecture du payload introspect ────────────────────────────────────

/// Extrait le `seq` courant du buffer rempli par `introspect()`.
pub fn seq_from_introspect(buf: &[u8; INTROSPECT_LEN]) -> u64 {
    u64::from_le_bytes(buf[32..40].try_into().unwrap())
}

/// Extrait le `lifecycle_state` byte du buffer rempli par `introspect()`.
pub fn lifecycle_from_introspect(buf: &[u8; INTROSPECT_LEN]) -> u8 {
    buf[73]
}

/// Retourne true si `last_action_id` est renseigné (bit 0 de flags).
pub fn has_action(buf: &[u8; INTROSPECT_LEN]) -> bool {
    buf[72] & 0x01 != 0
}

/// Retourne true si `last_snapshot` est renseigné (bit 1 de flags).
pub fn has_snapshot(buf: &[u8; INTROSPECT_LEN]) -> bool {
    buf[72] & 0x02 != 0
}

// ── agent_infer (ADR-0019) ────────────────────────────────────────────────────

/// Taille maximale du buffer de réponse LLM (ADR-0019 §Q-OPEN-6 : inline borné 8 KB).
pub const INFER_RESPONSE_BUF_LEN: usize = 8 * 1024;

/// Codes retour d'agent_infer (ADR-0019 §Q1).
pub const INFER_OK: i32       = 0;
pub const INFER_TIMEOUT: i32  = 1;
pub const INFER_ERROR: i32    = 2;
pub const INFER_NO_SLOT: i32  = 3; // réservé Phase 6 (D-Q-V2.6)
pub const INFER_CANCELLED: i32 = 4;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn agent_infer(
        prompt_ptr:       *const u8,
        prompt_len:       u32,
        response_buf_ptr: *mut u8,
        response_buf_cap: u32,
        response_len_out: *mut u32,
        timeout_ms:       u32,
    ) -> i32;
}

// ── AgentProfile (ADR-0025) ───────────────────────────────────────────────────

/// Profil watchdog d'un agent WASM (ADR-0025).
///
/// Détermine le plafond de temps par `process_one` via l'epoch_interruption de Wasmtime.
/// Codé sur u8 dans le payload de `Spawned (0x01)` (1 byte additionnel en fin de payload).
///
/// | Profil    | MAX_TICKS | Plafond (~EPOCH_TICK_MS_BASE=10ms) |
/// |-----------|-----------|-----------------------------------|
/// | Algo      |    10     | ~100 ms                            |
/// | LlmShort  |   500     | ~5 s (défaut actuel)               |
/// | LlmLong   |  3 000    | ~30 s                              |
/// | Batch     | 30 000    | ~5 min                             |
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentProfile {
    /// Agent déterministe pur — boucle courte, réponse < 100 ms attendue.
    Algo     = 0x01,
    /// Agent LLM avec inférence courte (défaut, compatible avec les agents existants).
    LlmShort = 0x02,
    /// Agent LLM avec inférence longue (jusqu'à ~30 s).
    LlmLong  = 0x03,
    /// Agent de traitement par lots — peut prendre jusqu'à ~5 min.
    Batch    = 0x04,
}

impl AgentProfile {
    /// Retourne le nombre maximum de ticks epoch par `process_one`.
    pub fn max_ticks(self) -> u64 {
        match self {
            AgentProfile::Algo     =>     10,
            AgentProfile::LlmShort =>    500,
            AgentProfile::LlmLong  =>  3_000,
            AgentProfile::Batch    => 30_000,
        }
    }

    /// Construit un AgentProfile depuis son discriminant u8.
    /// Retourne `AgentProfile::LlmShort` si le byte est inconnu (compatibilité ascendante).
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

/// ADR-0019 — appelle le LLM externe avec le prompt donné.
///
/// Bloque du côté WASM jusqu'à réception d'une réponse, d'un timeout,
/// ou d'une cancellation par le scheduler (rollback).
///
/// Retourne `Ok(bytes_written)` si succès, `Err(code)` sinon.
/// `response_buf` doit avoir au moins `INFER_RESPONSE_BUF_LEN` octets.
#[inline]
pub fn infer(prompt: &[u8], response_buf: &mut [u8], timeout_ms: u32) -> Result<usize, i32> {
    #[cfg(target_arch = "wasm32")]
    {
        let mut len_out: u32 = 0;
        let rc = unsafe {
            agent_infer(
                prompt.as_ptr(),
                prompt.len() as u32,
                response_buf.as_mut_ptr(),
                response_buf.len() as u32,
                &mut len_out,
                timeout_ms,
            )
        };
        if rc == INFER_OK { Ok(len_out as usize) } else { Err(rc) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (prompt, response_buf, timeout_ms);
        Ok(0)
    }
}
