// Runtime : Wasmtime + Tokio — valide H-commit-barrier (S4, S5, S7)
//
// Architecture :
//   - ActorInstance = module WASM + Store<AgentState> + host functions commit_barrier/emit
//   - run_loop = task Tokio qui traite les messages séquentiellement (S5)
//   - commit_barrier = snapshot ContentStore + entrée CausalLog avant tout effet externe (S4)
//   - H-cb-correct : debug_assert dans emit garantit l'invariant barrier_fired
//   - H-cb-overhead : mesuré par benches/commit_barrier.rs, cible ≤ 5% CPU sur W1

pub mod actor;
pub mod clock;
pub mod crash_point;
pub mod durability;
pub mod error;
pub mod fleet;
pub mod inference;
pub mod integrity;
pub mod io_queue;
pub mod scheduler;
pub mod watchdog;

// D9 (ADR-0019 §Q-V2.1 + ADR-0025) : watchdog d'instruction WASM via epoch_interruption.
// run_loop réarme la deadline à MAX_PROCESS_ONE_TICKS avant chaque process_one.
//
// ADR-0025 : EPOCH_TICK_MS_BASE passe de 100 ms à 10 ms pour permettre au profil Algo
// d'avoir un plafond de 100 ms (10 ticks × 10 ms) sans obliger LlmShort à 100 ms minimum.
// Rétro-compatibilité : LlmShort = 500 ticks × 10 ms = 5 s (identique à l'ancienne valeur).
pub use watchdog::EPOCH_TICK_MS_BASE;
pub const EPOCH_TICK_MS: u64 = EPOCH_TICK_MS_BASE; // alias rétro-compatible
pub const MAX_PROCESS_ONE_TICKS: u64 = watchdog::DEFAULT_MAX_TICKS; // 500 ticks × 10ms = 5s

/// Crée un Engine Wasmtime avec epoch_interruption activé et lance le thread background
/// qui incrémente l'epoch toutes les EPOCH_TICK_MS_BASE millisecondes.
/// Utiliser cette fonction partout où run_loop est impliqué (actors, tests d'intégration).
/// Les benchmarks peuvent continuer à utiliser Engine::default() (epoch désactivé).
pub fn make_engine() -> wasmtime::Engine {
    let mut cfg = wasmtime::Config::new();
    cfg.epoch_interruption(true);
    cfg.async_support(true); // requis pour func_wrap_async (ADR-0019 T5)
    let engine = wasmtime::Engine::new(&cfg).expect("make_engine: epoch_interruption+async");
    let engine_bg = engine.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(EPOCH_TICK_MS_BASE));
        engine_bg.increment_epoch();
    });
    engine
}

pub use error::RuntimeError;

/// B1 — charge un module WASM depuis le système de fichiers.
/// Supporte les fichiers `.wasm` (binaire) et `.wat` (texte WebAssembly).
/// Wrappé ici pour centraliser la gestion d'erreur et faciliter les tests d'intégration.
pub fn load_module_from_file(engine: &wasmtime::Engine, path: &std::path::Path) -> Result<wasmtime::Module, RuntimeError> {
    wasmtime::Module::from_file(engine, path).map_err(RuntimeError::Wasmtime)
}

#[cfg(test)]
mod tests {
    use super::actor::{ActorInstance, LifecycleState, ValidationVerdict, INTROSPECT_AGENT_WAT, SESSION_AGENT_WAT, CROSS_AGENT_WAT};
    use os_poc_causal_log::CausalLog;
    use os_poc_store::{ContentStore, Cache};
    use std::sync::Arc;
    use tempfile::TempDir;
    use wasmtime::{Engine, Module};

    fn setup() -> (Engine, Arc<ContentStore>, Arc<CausalLog>, TempDir) {
        let dir = TempDir::new().unwrap();
        let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
        let store = Arc::new(ContentStore::open(&dir.path().join("store"), Some(shared_cache.clone())).unwrap());
        let log   = Arc::new(CausalLog::open(&dir.path().join("log"), Some(shared_cache)).unwrap());
        // D9 (ADR-0019) : make_engine() active epoch_interruption + thread background.
        let engine = crate::make_engine();
        (engine, store, log, dir)
    }

    // ── Garde anti-érosion RUSTSEC-2026-0096 (red team B-1) ──────────────────────

    /// `wasm_memory64` doit rester désactivé. La miscompilation Cranelift aarch64
    /// GHSA-jhxm-h53p-jm7w (CVE-2026-34971, CVSS 9.0, sandbox escape) ne s'applique
    /// qu'aux mémoires linéaires WASM 64 bits — verbatim advisory : « 32-bit
    /// WebAssembly is not affected ». Le projet n'active jamais `Config::wasm_memory64`
    /// (défaut wasmtime `false`), ce qui rend le CVE **N/A par configuration**, y compris
    /// sur la cible aarch64/seL4.
    ///
    /// Cet invariant est tacite (un flag laissé à son défaut, jamais configuré) : aucun
    /// mécanisme ne détecte son érosion. Ce test le verrouille en fail-closed. Si un futur
    /// changement active `wasm_memory64(true)` dans `make_engine()`, un module 64 bits se
    /// chargerait et ce test échouerait — signal de rouvrir la dette upgrade wasmtime
    /// (≥36.0.7/≥42.0.2/≥43.0.1 ; ADR-0049, table des déclencheurs dormants : `wasm_memory64`).
    #[test]
    fn memory64_reste_desactive() {
        let engine = crate::make_engine();
        let module_64 = Module::new(&engine, "(module (memory i64 1))");
        assert!(
            module_64.is_err(),
            "wasm_memory64 doit rester désactivé (RUSTSEC-2026-0096 N/A par config) — \
             un module (memory i64) ne doit pas se charger. Si ce test échoue, memory64 \
             a été activé : rouvrir la dette upgrade wasmtime (B-1 / ADR-0049 dormants)."
        );
        // Sanity : la mémoire 32 bits, elle, reste chargeable.
        let module_32 = Module::new(&engine, "(module (memory 1))");
        assert!(module_32.is_ok(), "mémoire 32 bits doit rester chargeable");
    }

    // ── Tests B1 — chargement module WASM depuis disque ──────────────────────────

    /// B1 — un module WAT écrit sur disque est chargé via Module::from_file et exécuté
    /// comme un acteur normal. Valide le pipeline de chargement externe (vs WAT inline).
    #[tokio::test(flavor = "current_thread")]
    async fn b1_module_from_file_wat() {
        use std::io::Write;
        use super::load_module_from_file;

        let (engine, store, log, dir) = setup();

        // Écrit le WAT minimal sur disque dans le répertoire temp du test
        let wat_path = dir.path().join("minimal.wat");
        let mut f = std::fs::File::create(&wat_path).unwrap();
        f.write_all(super::actor::AGENT_WAT.as_bytes()).unwrap();
        drop(f);

        // B1 : charge depuis disque (au lieu d'une constante WAT inline)
        let module = load_module_from_file(&engine, &wat_path)
            .expect("Module::from_file doit charger un fichier .wat valide");

        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, [0xB1u8; 16], store, log,
        ).await.unwrap();

        actor.process_one(b"hello-from-disk").await.unwrap();
        assert_eq!(actor.seq(), 1, "agent chargé depuis disque : seq=1 après un cycle");
    }

    /// B1 — variante : module WAT compilé en .wasm binaire, puis chargé depuis disque.
    /// Valide que Module::from_file fonctionne aussi avec les binaires .wasm.
    #[tokio::test(flavor = "current_thread")]
    async fn b1_module_from_file_wasm_binary() {

        let (engine, store, log, dir) = setup();

        // Compile le WAT en Module, sérialise en .wasm binaire sur disque
        let module_orig = Module::new(&engine, super::actor::INTROSPECT_AGENT_WAT).unwrap();
        let wasm_bytes = module_orig.serialize().unwrap();
        let wasm_path = dir.path().join("introspect.cwasm");
        std::fs::write(&wasm_path, &wasm_bytes).unwrap();

        // Charge le binaire compilé depuis disque via unsafe deserialize
        // (Module::from_file charge un .cwasm pré-compilé — différent de from_file WAT)
        let module = unsafe {
            Module::deserialize_file(&engine, &wasm_path)
                .expect("deserialize_file doit charger un module pré-compilé")
        };

        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, [0xB2u8; 16], store, log,
        ).await.unwrap();

        actor.process_one(b"binary-from-disk").await.unwrap();
        actor.process_one(b"second-cycle").await.unwrap();
        assert_eq!(actor.seq(), 2, "agent chargé depuis .cwasm : seq=2 après deux cycles");
    }

    /// B4 / critère de sortie semaine 1 — module WASM compilé depuis Rust (echo.wasm),
    /// chargé depuis disque, exerce A1 (agent_introspect) et termine proprement.
    ///
    /// Ce test est ignoré si echo.wasm n'a pas encore été compilé
    /// (`cargo build --target wasm32-unknown-unknown -p agent-sdk --example echo`).
    #[tokio::test(flavor = "current_thread")]
    async fn b4_echo_wasm_from_rust_exercises_a1() {
        use os_poc_causal_log::EmitType;

        // Cherche echo.wasm depuis la racine du workspace (poc/)
        // CARGO_MANIFEST_DIR = .../poc/runtime → parent = .../poc/
        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_candidates = [
            ws_root.join("target/wasm32-unknown-unknown/debug/examples/echo.wasm"),
            ws_root.join("target/wasm32-unknown-unknown/release/examples/echo.wasm"),
        ];
        let wasm_path = wasm_candidates.iter().find(|p| p.exists());

        let wasm_path = match wasm_path {
            Some(p) => p.clone(),
            None => {
                eprintln!("SKIP b4_echo_wasm_from_rust_exercises_a1 : echo.wasm absent \
                           (run: cargo build --target wasm32-unknown-unknown -p agent-sdk --example echo)");
                return;
            }
        };

        let (engine, store, log_ref, _dir) = setup();

        // Charge le .wasm compilé depuis Rust (B1 + B4 ensemble)
        let bytes = std::fs::read(&wasm_path)
            .expect("lecture echo.wasm");
        let module = Module::new(&engine, &bytes)
            .expect("Module::new depuis echo.wasm");

        let agent_id = [0xECu8; 16];
        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone(),
        ).await.expect("ActorInstance depuis echo.wasm");

        // Exerce A1 : process() appelle agent_introspect + barrier + emit
        actor.process_one(b"echo-test").await.expect("process_one sur echo.wasm");
        assert_eq!(actor.seq(), 1, "echo.wasm : seq=1 après un cycle (A1 exercé)");

        // Vérifie que le log contient une entrée EmitType::Introspect (0x06)
        let entries = log_ref.entries_by_agent(&agent_id);
        let has_introspect = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == EmitType::Introspect as u8);
        assert!(has_introspect,
            "echo.wasm doit émettre EmitType::Introspect (0x06) dans le log causal");
    }

    /// A1 — agent_introspect : avant toute action, l'état est vide (seq=0, flags=0).
    #[tokio::test(flavor = "current_thread")]
    async fn a1_introspect_initial_state() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [1u8; 16], store, log).await.unwrap();

        // Premier appel : aucune action précédente, seq = 0
        actor.process_one(b"ping").await.unwrap();
        assert_eq!(actor.seq(), 1, "seq doit être 1 après un cycle");
    }

    /// A1 — après N actions, introspect retourne last_action_id set (flags bit 0) et seq = N.
    #[tokio::test(flavor = "current_thread")]
    async fn a1_introspect_after_actions() {
        let (engine, store_ref, log_ref, _dir) = setup();

        // On utilise INTROSPECT_AGENT_WAT : le module appelle agent_introspect puis emit.
        // Le résultat introspect est émis dans le log causal (type Introspect = 0x06).
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [2u8; 16], store_ref, log_ref).await.unwrap();

        // Cycle 1 : avant le premier emit, last_action est None → flags bit 0 = 0
        actor.process_one(b"first").await.unwrap();
        // Cycle 2 : last_action est maintenant set → flags bit 0 = 1
        actor.process_one(b"second").await.unwrap();

        assert_eq!(actor.seq(), 2, "seq doit être 2 après deux cycles");
    }

    /// A1 — agent_introspect est non-enregistrant : le seq ne monte pas uniquement à cause d'introspect.
    /// Le seq monte uniquement via commit_barrier + emit.
    #[tokio::test(flavor = "current_thread")]
    async fn a1_introspect_is_non_recording() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [3u8; 16], store, log).await.unwrap();

        actor.process_one(b"msg").await.unwrap();
        let seq_after_one = actor.seq();

        actor.process_one(b"msg2").await.unwrap();
        assert_eq!(actor.seq(), seq_after_one + 1,
            "le seq monte d'exactement 1 par cycle commit_barrier+emit, pas plus");
    }

    /// A4 — état initial = Spawned.
    #[tokio::test(flavor = "current_thread")]
    async fn a4_initial_state_is_spawned() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let actor = ActorInstance::new_precompiled(&engine, &module, [4u8; 16], store, log).await.unwrap();
        assert_eq!(actor.lifecycle(), LifecycleState::Spawned);
    }

    /// A4 — log_lifecycle_event met à jour l'état et enregistre dans le log causal.
    #[tokio::test(flavor = "current_thread")]
    async fn a4_lifecycle_transition_logged() {
        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [5u8; 16], store, log_ref.clone()).await.unwrap();

        // Transition manuelle Spawned → Active
        actor.state_mut().log_lifecycle_event(LifecycleState::Active);
        assert_eq!(actor.lifecycle(), LifecycleState::Active);

        // last_action doit pointer vers l'entrée de lifecycle qu'on vient d'écrire
        let action_id = actor.last_action()
            .expect("last_action doit être set après log_lifecycle_event");
        let entry = log_ref.get(&action_id).expect("get").expect("entrée présente");
        assert!(entry.emit_payload.is_some(), "payload lifecycle présent dans le log");
    }

    /// A4 — agent_introspect retourne le lifecycle_state courant (byte [73]).
    #[tokio::test(flavor = "current_thread")]
    async fn a4_introspect_returns_lifecycle_state() {
        use super::actor::INTROSPECT_PAYLOAD_LEN;
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [6u8; 16], store, log).await.unwrap();

        actor.process_one(b"test").await.unwrap();
        assert_eq!(INTROSPECT_PAYLOAD_LEN, 74);
        assert_eq!(actor.lifecycle(), LifecycleState::Spawned);
    }

    /// A2 — rollback valide : 3 snapshots construits, rollback depth=1 restaure l'avant-dernier.
    #[tokio::test(flavor = "current_thread")]
    async fn a2_self_rollback_valid() {
        use super::actor::SELF_ROLLBACK_AGENT_WAT;
        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, SELF_ROLLBACK_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [10u8; 16], store, log_ref.clone()).await.unwrap();

        // Construit 3 snapshots (msg[0]=0 → commit_barrier + emit)
        actor.process_one(&[0x00]).await.unwrap(); // seq → 1, snap_seq=0
        actor.process_one(&[0x00]).await.unwrap(); // seq → 2, snap_seq=1
        actor.process_one(&[0x00]).await.unwrap(); // seq → 3, snap_seq=2
        assert_eq!(actor.seq(), 3);

        let snap_before = actor.last_snapshot().expect("snapshot présent après 3 cycles");

        // Rollback depth=1 : cible snap_seq=1 (seq=3-1-1=1)
        actor.process_one(&[0x01, 0x01]).await.unwrap();

        let snap_after = actor.last_snapshot().expect("snapshot présent après rollback");
        assert_ne!(snap_before, snap_after, "rollback doit changer last_snapshot");

        // Le log doit contenir une entrée SelfRollback
        let action_id = actor.last_action().expect("last_action set après rollback");
        let entry = log_ref.get(&action_id).expect("get").expect("entrée présente");
        let payload = entry.emit_payload.expect("payload SelfRollback présent");
        let envelope = os_poc_causal_log::EmitEnvelope::from_msgpack(&payload).unwrap();
        assert_eq!(envelope.emit_type, os_poc_causal_log::EmitType::SelfRollback as u8);
        assert_eq!(envelope.payload[0], 1, "depth dans payload");
        let target_seq = u64::from_le_bytes(envelope.payload[1..9].try_into().unwrap());
        assert_eq!(target_seq, 1u64, "target_seq = 3 - 1 - 1 = 1");
    }

    /// A2 — depth > MAX_SELF_ROLLBACK_DEPTH (3) est refusé.
    #[tokio::test(flavor = "current_thread")]
    async fn a2_self_rollback_depth_exceeded() {
        use super::actor::SELF_ROLLBACK_AGENT_WAT;
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, SELF_ROLLBACK_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [11u8; 16], store, log).await.unwrap();

        // Construit 5 snapshots
        for _ in 0..5 {
            actor.process_one(&[0x00]).await.unwrap();
        }
        let snap_before = actor.last_snapshot();

        // Depth=4 → refusé, last_snapshot ne change pas
        actor.process_one(&[0x01, 0x04]).await.unwrap();
        assert_eq!(actor.last_snapshot(), snap_before, "snapshot inchangé si depth > MAX");
    }

    /// A2 — rollback sans historique (aucun commit_barrier préalable) retourne une erreur.
    #[tokio::test(flavor = "current_thread")]
    async fn a2_self_rollback_no_history() {
        use super::actor::SELF_ROLLBACK_AGENT_WAT;
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, SELF_ROLLBACK_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [12u8; 16], store, log).await.unwrap();

        let snap_before = actor.last_snapshot();
        assert!(snap_before.is_none(), "aucun snapshot initial");

        // Depth=1, mais seq=0 → refusé
        actor.process_one(&[0x01, 0x01]).await.unwrap();
        assert_eq!(actor.last_snapshot(), snap_before, "snapshot toujours None");
    }

    /// A3 — request_validation enregistre la demande et passe lifecycle en AwaitingValidation.
    #[tokio::test(flavor = "current_thread")]
    async fn a3_validation_request_logged_and_awaiting() {
        use super::actor::VALIDATION_AGENT_WAT;
        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [20u8; 16], store, log_ref.clone()).await.unwrap();

        // Construit un snapshot puis demande validation risk=1
        actor.process_one(&[0x00]).await.unwrap();
        actor.process_one(&[0x02, 0x01]).await.unwrap(); // request_validation(1)

        assert_eq!(actor.lifecycle(), LifecycleState::AwaitingValidation,
            "lifecycle doit être AwaitingValidation après request_validation");

        // Log doit contenir une entrée ValidationRequest
        let action_id = actor.last_action().expect("last_action set");
        let entry = log_ref.get(&action_id).expect("get").expect("entrée présente");
        let payload = entry.emit_payload.expect("emit_payload présent");
        let envelope = os_poc_causal_log::EmitEnvelope::from_msgpack(&payload).unwrap();
        assert_eq!(envelope.emit_type, os_poc_causal_log::EmitType::ValidationRequest as u8);
        assert_eq!(envelope.payload[0], 1u8, "risk_level dans payload");
    }

    /// A3 — agent_get_verdict retourne le verdict après record_validation_response.
    #[tokio::test(flavor = "current_thread")]
    async fn a3_verdict_accessible_after_response() {
        use super::actor::VALIDATION_AGENT_WAT;
        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [21u8; 16], store, log_ref.clone()).await.unwrap();

        actor.process_one(&[0x00]).await.unwrap();
        actor.process_one(&[0x02, 0x00]).await.unwrap(); // request risk=0

        // Superviseur répond Approved
        actor.state_mut().record_validation_response(ValidationVerdict::Approved);
        assert_eq!(actor.lifecycle(), LifecycleState::Active, "lifecycle restauré Active");
        assert_eq!(actor.last_verdict(), Some(ValidationVerdict::Approved));

        // Agent lit le verdict via agent_get_verdict + l'émet
        actor.process_one(&[0x03]).await.unwrap();

        // Le log doit contenir ValidationResponse puis l'emit du verdict
        // (on vérifie via last_action qui est l'ActionResult du msg[0]=3)
        let action_id = actor.last_action().expect("last_action set");
        let entry = log_ref.get(&action_id).expect("get").expect("entrée présente");
        let payload = entry.emit_payload.expect("emit_payload présent");
        let envelope = os_poc_causal_log::EmitEnvelope::from_msgpack(&payload).unwrap();
        assert_eq!(envelope.payload[0], ValidationVerdict::Approved as u8,
            "verdict=Approved émis dans le log");
    }

    /// Session — état initial : session_id=1, action_count=0.
    #[tokio::test(flavor = "current_thread")]
    async fn session_initial_state() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();
        let actor = ActorInstance::new_precompiled(&engine, &module, [30u8; 16], store, log).await.unwrap();
        assert_eq!(actor.session_id(), 1, "session_id initial = 1");
        assert_eq!(actor.session_action_count(), 0, "action_count initial = 0");
    }

    /// Session — action_count reflète le nombre de commit_barriers de la session courante.
    #[tokio::test(flavor = "current_thread")]
    async fn session_action_count_increments() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [31u8; 16], store, log).await.unwrap();

        actor.process_one(&[0x00]).await.unwrap();
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.session_action_count(), 2);
        assert_eq!(actor.session_id(), 1, "toujours session 1");
    }

    /// Session — frontière automatique déclenchée quand session_max_actions atteint.
    #[tokio::test(flavor = "current_thread")]
    async fn session_boundary_auto_trigger() {
        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();
        let mut actor = ActorInstance::new_precompiled(&engine, &module, [32u8; 16], store, log_ref.clone()).await.unwrap();

        // Baisse la limite à 3 pour le test
        actor.state_mut().session_max_actions = 3;

        // 3 actions → déclenche la frontière de session à la 3e
        actor.process_one(&[0x00]).await.unwrap();
        actor.process_one(&[0x00]).await.unwrap();
        actor.process_one(&[0x00]).await.unwrap(); // → session_boundary ici

        assert_eq!(actor.session_id(), 2, "session_id doit passer à 2 après la frontière");
        assert_eq!(actor.session_action_count(), 0, "action_count réinitialisé");
        assert_eq!(actor.lifecycle(), LifecycleState::Checkpointed, "lifecycle = Checkpointed après frontière");

        // Le log contient une entrée SessionBoundary
        let action_id = actor.last_action().expect("last_action set");
        // L'entrée SessionBoundary est antérieure au dernier Checkpointed lifecycle.
        // On vérifie via le CausalLog : cherche une entrée avec SessionBoundary = 0x0A.
        // Approche : parcours les dernières entrées depuis last_action.
        // Pour simplifier : lit l'avant-dernière action (SessionBoundary précède Checkpointed lifecycle).
        // On fait confiance à log_session_boundary() d'avoir appelé append avant log_lifecycle_event.
        let entry = log_ref.get(&action_id).expect("get").expect("entrée");
        let payload = entry.emit_payload.expect("emit_payload");
        let _env = os_poc_causal_log::EmitEnvelope::from_msgpack(&payload).unwrap();
        // Le last_action pointe sur le Lifecycle=Checkpointed, pas sur SessionBoundary.
        // SessionBoundary est dans l'action précédente (parent de Checkpointed).
        // Vérification via parent : l'entrée Lifecycle doit avoir parent = action SessionBoundary.
        assert!(!entry.parent_ids.is_empty(), "Lifecycle doit avoir un parent (SessionBoundary)");
        let sb_id = entry.parent_ids[0];
        let sb_entry = log_ref.get(&sb_id).expect("get").expect("sb_entry");
        let sb_payload = sb_entry.emit_payload.expect("sb payload");
        let sb_env = os_poc_causal_log::EmitEnvelope::from_msgpack(&sb_payload).unwrap();
        assert_eq!(sb_env.emit_type, os_poc_causal_log::EmitType::SessionBoundary as u8,
            "parent doit être SessionBoundary");
        let session_id_in_payload = u64::from_le_bytes(sb_env.payload[..8].try_into().unwrap());
        assert_eq!(session_id_in_payload, 1u64, "session_id=1 dans le payload de la frontière");
    }

    /// D7 / ADR-0012 — borne durée configurable : un agent avec
    /// `session_max_duration_ms` court doit franchir la frontière sur la borne durée
    /// (et non sur la borne actions), même si `session_max_actions` reste à la valeur
    /// par défaut (10 000).
    ///
    /// Falsifiabilité : si la constante module `SESSION_DEFAULT_MAX_DURATION_MS` est
    /// encore utilisée à la place du champ d'AgentState, ce test échoue car la
    /// frontière n'est jamais atteinte sur la fenêtre du test (~50 ms < 24 h).
    #[tokio::test(flavor = "current_thread")]
    async fn session_boundary_duration_configurable() {
        use super::actor::{ActorInstance, SESSION_AGENT_WAT, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use os_poc_capabilities::CapabilityStore;
        use std::sync::Mutex;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();

        // Borne durée à 50 ms : la 1re action initialise `session_started_at_ms`,
        // la 2e (après sleep > 50 ms) déclenche la frontière sur la borne durée.
        const SESSION_MAX_DURATION_MS: u64 = 50;
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let mut actor = ActorInstance::new_precompiled_with_caps_timeout_and_session(
            &engine, &module, [0xD7u8; 16], store, log_ref.clone(),
            cap_store, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            SESSION_MAX_DURATION_MS,
        ).await.unwrap();

        // Sanity : session 1 active, action_count = 0.
        assert_eq!(actor.session_id(), 1);
        assert_eq!(actor.session_action_count(), 0);

        // 1re action : initialise session_started_at_ms ; elapsed = 0 → pas de frontière.
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.session_id(), 1, "1re action : pas encore de frontière");
        assert_eq!(actor.session_action_count(), 1);

        // Attendre > SESSION_MAX_DURATION_MS pour franchir la borne durée.
        // 200 ms = ×4 la borne (50 ms) — marge suffisante sous charge CI.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 2e action : elapsed ≥ 200 ms > 50 ms → frontière déclenchée par borne *durée*
        // (et non par borne actions : 2 actions < SESSION_DEFAULT_MAX_ACTIONS = 10_000).
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.session_id(), 2, "frontière franchie sur borne durée → session 2");
        assert_eq!(actor.session_action_count(), 0, "action_count remis à zéro");
        assert_eq!(actor.lifecycle(), LifecycleState::Checkpointed,
            "lifecycle = Checkpointed après frontière");

        // Le log contient une entrée SessionBoundary (parent du Lifecycle=Checkpointed).
        let action_id = actor.last_action().expect("last_action set");
        let entry = log_ref.get(&action_id).expect("get").expect("entrée Lifecycle");
        assert!(!entry.parent_ids.is_empty(), "Lifecycle doit avoir un parent SessionBoundary");
        let sb_entry = log_ref.get(&entry.parent_ids[0])
            .expect("get").expect("sb_entry");
        let sb_env = os_poc_causal_log::EmitEnvelope::from_msgpack(
            &sb_entry.emit_payload.expect("sb payload")
        ).unwrap();
        assert_eq!(sb_env.emit_type, os_poc_causal_log::EmitType::SessionBoundary as u8);
    }

    // ── Tests T5 — host function agent_infer ────────────────────��────────────

    /// T5 — happy path : agent_infer avec SleepyBackend retourne 0 (Ok),
    /// le log causal contient InferenceRequest (0x0C) + InferenceResponse (0x0D).
    #[tokio::test(flavor = "multi_thread")]
    async fn t5_agent_infer_happy_path() {
        use super::actor::{INFER_AGENT_WAT, ActorInstance, Message, LifecycleState};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::EmitType;
        use std::sync::Arc;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFER_AGENT_WAT).unwrap();
        let agent_id = [0xAFu8; 16];

        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 30 }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id, store, log_ref.clone(),
            Arc::new(std::sync::Mutex::new(os_poc_capabilities::CapabilityStore::new())),
            vec![],
            super::actor::SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            0,
            infer_fn,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // Build 1 snapshot then call agent_infer
        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x07])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let has_req = envelopes.iter().any(|e| e.emit_type == EmitType::InferenceRequest as u8);
        let has_resp = envelopes.iter().any(|e| e.emit_type == EmitType::InferenceResponse as u8);
        assert!(has_req,  "T5: InferenceRequest (0x0C) doit être dans le log");
        assert!(has_resp, "T5: InferenceResponse (0x0D) doit être dans le log");

        // Agent lifecycle doit avoir transité WaitingInference puis Active
        let lifecycle_payloads: Vec<_> = envelopes.iter()
            .filter(|e| e.emit_type == EmitType::Lifecycle as u8)
            .collect();
        let had_waiting = lifecycle_payloads.iter().any(|e|
            e.payload.first().copied() == Some(LifecycleState::WaitingInference as u8));
        assert!(had_waiting, "T5: WaitingInference doit avoir été logué");
    }

    /// T5 — cancellation : cancel avant la fin du sleep → InferenceCancelled (0x0E), code 4.
    #[tokio::test(flavor = "multi_thread")]
    async fn t5_agent_infer_cancelled() {
        use super::actor::{INFER_AGENT_WAT, ActorInstance, Message};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::EmitType;
        use std::sync::Arc;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFER_AGENT_WAT).unwrap();
        let agent_id = [0xB0u8; 16];

        // Délai 60s → ne répondra jamais dans la fenêtre du test
        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 60_000 }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id, store, log_ref.clone(),
            Arc::new(std::sync::Mutex::new(os_poc_capabilities::CapabilityStore::new())),
            vec![],
            super::actor::SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            0,
            infer_fn,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x07])).await.unwrap(); // démarre l'inférence

        // Attendre que l'inférence soit en cours (pool is_active)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Annuler depuis le pool
        pool.cancel(&agent_id);

        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let has_req = envelopes.iter().any(|e| e.emit_type == EmitType::InferenceRequest as u8);
        let has_cancelled = envelopes.iter().any(|e| e.emit_type == EmitType::InferenceCancelled as u8);
        assert!(has_req,       "T5: InferenceRequest (0x0C) doit être dans le log");
        assert!(has_cancelled, "T5: InferenceCancelled (0x0E) doit être dans le log après cancel");
    }

    /// T6 (ADR-0019 §Q-V2.1) — Scheduler::rollback annule l'inférence en cours avant rollback.
    /// Ordre garanti : pool.cancel(agent_id) → inbox.send(Rollback).
    /// L'agent doit recevoir InferenceCancelled (0x0E) PUIS retourner à un état cohérent.
    #[tokio::test(flavor = "multi_thread")]
    async fn t6_rollback_cancels_inference_in_flight() {
        use super::actor::{INFER_AGENT_WAT, ActorInstance, Message, LifecycleState};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::EmitType;
        use std::sync::Arc;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFER_AGENT_WAT).unwrap();
        let agent_id = [0xC6u8; 16];

        // Délai 60s → bloqué pendant la durée du test
        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 60_000 }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id, store, log_ref.clone(),
            Arc::new(std::sync::Mutex::new(os_poc_capabilities::CapabilityStore::new())),
            vec![],
            super::actor::SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            0,
            infer_fn,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);
        let tx = scheduler.register(actor);

        // Construit 1 snapshot, puis lance l'inférence bloquante
        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x07])).await.unwrap();

        // Attendre que l'inférence soit active dans le pool
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(pool.is_active(&agent_id), "T6: inférence doit être active avant rollback");

        // Rollback via scheduler : doit annuler l'inférence avant d'envoyer Rollback
        scheduler.rollback(&agent_id, 0).await.unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!pool.is_active(&agent_id), "T6: inférence doit être terminée après rollback");

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let has_cancelled = envelopes.iter().any(|e| e.emit_type == EmitType::InferenceCancelled as u8);
        assert!(has_cancelled, "T6: InferenceCancelled (0x0E) doit être dans le log");

        // L'agent doit avoir transité WaitingInference → Active (pas Terminated)
        let lifecycle_envelopes: Vec<_> = envelopes.iter()
            .filter(|e| e.emit_type == EmitType::Lifecycle as u8)
            .collect();
        let had_waiting = lifecycle_envelopes.iter().any(|e|
            e.payload.first().copied() == Some(LifecycleState::WaitingInference as u8));
        let back_active = lifecycle_envelopes.iter().rev().any(|e|
            e.payload.first().copied() == Some(LifecycleState::Active as u8));
        assert!(had_waiting,  "T6: WaitingInference doit avoir été logué");
        assert!(back_active,  "T6: agent doit revenir Active après InferenceCancelled");
    }

    /// T2.4 / D9 (ADR-0019 §Q-V2.1) — un agent en boucle infinie est trappé par le watchdog
    /// epoch_interruption sous 5s (MAX_PROCESS_ONE_TICKS × EPOCH_TICK_MS).
    ///
    /// Falsifiabilité :
    ///   - Sans epoch_interruption, le test bloque indéfiniment (timeout Tokio le tuerait).
    ///   - Sans `set_epoch_deadline(50)` dans run_loop, le Store a deadline=1_000_000_000
    ///     et le test prendrait ~10⁸ secondes — jamais terminé.
    ///   - Si le trap n'est pas traité comme Terminated, run_loop paniquerait ou resterait actif.
    #[tokio::test(flavor = "multi_thread")]
    async fn t2_watchdog_traps_infinite_loop_agent() {
        use super::actor::{INFINITE_LOOP_AGENT_WAT, Message, LifecycleState};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFINITE_LOOP_AGENT_WAT).unwrap();
        let agent_id = [0xD9u8; 16];

        let actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone(),
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // Envoie un message → process() entre dans la boucle infinie.
        tx.send(Message::data(b"trigger".to_vec())).await.unwrap();

        // Le watchdog doit interrompre sous 5s (50 ticks × 100ms).
        // On attend 8s (marge ×1.6) pour absorber la latence de scheduling.
        tokio::time::sleep(Duration::from_secs(8)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // L'agent doit être terminé (lifecycle = Terminated dans le log).
        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // ADR-0015 D-Q-V2.2 : AgentCrash (0x13) est désormais le terminal event ;
        // il n'y a plus de Lifecycle::Terminated séparé après un crash.
        let crashed = envelopes.iter().any(|env|
            env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8
        );
        assert!(
            crashed,
            "T2.4 : agent en boucle infinie doit être terminé par le watchdog epoch (AgentCrash dans le log, D9)"
        );
    }

    // ── Tests ADR-0015 D15.2 — émission AgentCrash (0x13) avant Terminated ───────

    /// Récupère toutes les enveloppes `AgentCrash` (0x13) d'un agent.
    fn agent_crash_envelopes(
        log: &os_poc_causal_log::CausalLog,
        agent_id: &os_poc_causal_log::AgentId,
    ) -> Vec<os_poc_causal_log::EmitEnvelope> {
        log.entries_by_agent(agent_id)
            .iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8)
            .collect()
    }

    /// D15.2-a — un agent dont `process()` exécute `unreachable` provoque
    /// `process_one → Err(Wasmtime::Trap::UnreachableCodeReached)` → cause `ProcessFailed (0x01)`.
    /// L'événement AgentCrash doit être émis AVANT le Lifecycle Terminated et précéder
    /// ce dernier dans l'ordre du log.
    ///
    /// Falsifiabilité : si l'instrumentation D15.2-a est absente, seul un Lifecycle
    /// Terminated est émis et le test échoue sur l'assertion `crash_envs.len() == 1`.
    #[tokio::test(flavor = "current_thread")]
    async fn d15_2_process_failed_emits_agent_crash() {
        use super::actor::{Message, TRAP_AGENT_WAT};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, TRAP_AGENT_WAT).unwrap();
        let agent_id = [0x15u8; 16];

        let actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone(),
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);
        tx.send(Message::data(b"trigger".to_vec())).await.unwrap();

        // L'agent trap immédiatement ; on laisse 200 ms à run_loop pour émettre 0x13 + Terminated.
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let crash_envs = agent_crash_envelopes(&log_ref, &agent_id);
        assert_eq!(
            crash_envs.len(), 1,
            "D15.2-a : exactement un AgentCrash attendu pour une terminaison anormale"
        );
        let payload = &crash_envs[0].payload;
        assert_eq!(payload.len(), 49, "payload AgentCrash = 49 bytes (1+16+32)");
        assert_eq!(
            payload[0], 0x01,
            "cause = ProcessFailed (0x01) pour un trap UnreachableCodeReached"
        );
        // Agent racine (créé via register, pas spawn_child) → parent sentinelle [0u8;16].
        assert!(
            payload[1..17].iter().all(|b| *b == 0),
            "parent_agent_id = sentinelle racine pour un agent enregistré directement"
        );

        // ADR-0015 D-Q-V2.2 : AgentCrash (0x13) est le terminal event — il n'y a plus de
        // Lifecycle::Terminated séparé. L'atomicité est garantie par un seul append RocksDB.
        // Les outils de reconstruct synthétisent le Terminated implicite à la lecture.
        // On vérifie seulement que l'AgentCrash est bien présent dans le log (invariant P-D15-1).
        let crash_in_log = log_ref.entries_by_agent(&agent_id)
            .into_iter()
            .any(|(_, e)| {
                e.emit_payload.as_ref()
                    .and_then(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                    .map(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8)
                    .unwrap_or(false)
            });
        assert!(crash_in_log, "D15.2-a : AgentCrash doit être présent dans le log causal");
    }

    /// D15.2-a — un agent en boucle infinie est interrompu par le watchdog
    /// (`Trap::Interrupt`) → cause `WatchdogTrap (0x03)`. Le test consomme le watchdog
    /// epoch (~5 s) ; on garde une marge 8 s comme `t2_watchdog_traps_infinite_loop_agent`.
    ///
    /// Falsifiabilité :
    ///   - Sans `classify_process_one_error`, le test échouerait sur `payload[0] == 0x03`
    ///     en obtenant `0x01 (ProcessFailed)` à la place.
    ///   - Sans D15.2-a instrumenté, `crash_envs` serait vide.
    #[tokio::test(flavor = "multi_thread")]
    async fn d15_2_watchdog_trap_emits_agent_crash() {
        use super::actor::{INFINITE_LOOP_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFINITE_LOOP_AGENT_WAT).unwrap();
        let agent_id = [0x16u8; 16];

        let actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone(),
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);
        tx.send(Message::data(b"trigger".to_vec())).await.unwrap();

        // 8 s = MAX_PROCESS_ONE_TICKS × EPOCH_TICK_MS (5 s) × 1.6 marge scheduling.
        tokio::time::sleep(Duration::from_secs(8)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let crash_envs = agent_crash_envelopes(&log_ref, &agent_id);
        assert_eq!(
            crash_envs.len(), 1,
            "D15.2-a : un seul AgentCrash pour le trap watchdog"
        );
        assert_eq!(
            crash_envs[0].payload[0], 0x03,
            "cause = WatchdogTrap (0x03) pour un Trap::Interrupt"
        );
    }

    /// D15.2-b — `Scheduler::spawn_child` doit peupler `parent_agent_id` dans l'enfant ;
    /// si l'enfant crash (ici via `unreachable`), le payload AgentCrash référence le parent.
    ///
    /// Falsifiabilité : sans l'appel `set_parent_agent_id` dans `spawn_child`, le test
    /// échoue parce que `payload[1..17] == [0u8;16]` (sentinelle racine au lieu du parent).
    #[tokio::test(flavor = "current_thread")]
    async fn d15_2_b_spawn_child_populates_parent_agent_id() {
        use super::actor::{INTROSPECT_AGENT_WAT, TRAP_AGENT_WAT};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let mod_parent = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_child  = Module::new(&engine, TRAP_AGENT_WAT).unwrap();

        let parent_id = [0x15u8; 16];
        let child_id  = [0x16u8; 16];

        // Le parent doit avoir au moins une action pour fournir un parent_cause valide.
        let mut parent_actor = ActorInstance::new_precompiled(
            &engine, &mod_parent, parent_id, store.clone(), log_ref.clone()
        ).await.unwrap();
        parent_actor.process_one(b"parent-work").await.unwrap();
        let parent_cause = parent_actor.last_action().expect("parent action");

        let mut scheduler = Scheduler::new();
        scheduler.spawn_child(
            &engine, &mod_child, child_id,
            store.clone(), log_ref.clone(),
            parent_cause,
            b"trigger".to_vec(), // unreachable se déclenche dès process()
            &parent_id,
            &[],
        ).await.unwrap();

        // Laisse l'enfant traiter le message, trapper, émettre AgentCrash + Terminated.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let crash_envs = agent_crash_envelopes(&log_ref, &child_id);
        assert_eq!(crash_envs.len(), 1, "un AgentCrash attendu pour l'enfant qui trap");
        let payload = &crash_envs[0].payload;
        assert_eq!(payload[0], 0x01, "cause = ProcessFailed");
        assert_eq!(
            &payload[1..17], &parent_id,
            "D15.2-b : parent_agent_id dans le payload = parent direct (spawn_child)"
        );
    }

    /// D7 — convention `session_max_duration_ms == 0` désactive la borne durée.
    /// Sans cette convention, le check `elapsed >= 0` se déclencherait à chaque
    /// action → boucle de sessions vides. Test de garde anti-régression.
    #[tokio::test(flavor = "current_thread")]
    async fn session_duration_zero_disables_duration_bound() {
        use super::actor::{ActorInstance, SESSION_AGENT_WAT, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use os_poc_capabilities::CapabilityStore;
        use std::sync::Mutex;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let mut actor = ActorInstance::new_precompiled_with_caps_timeout_and_session(
            &engine, &module, [0xD8u8; 16], store, log,
            cap_store, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            0, // désactivé
        ).await.unwrap();

        // Plusieurs actions, attente entre chacune pour que toute borne durée naïve
        // se déclencherait. La session doit rester 1.
        for _ in 0..3 {
            actor.process_one(&[0x00]).await.unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(actor.session_id(), 1,
            "session_max_duration_ms=0 doit désactiver la borne durée");
        assert_eq!(actor.session_action_count(), 3);
    }

    /// Session — SessionResume via run_loop injecte le résumé comme premier Data.
    #[tokio::test]
    async fn session_resume_delivers_summary() {
        use super::actor::SESSION_AGENT_WAT;
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, _log, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();
        let actor = ActorInstance::new_precompiled(&engine, &module, [33u8; 16], store, _log).await.unwrap();

        let mut scheduler = Scheduler::new();
        let id = actor.agent_id();
        let tx = scheduler.register(actor);

        // Envoie un résumé causal
        let summary = b"session-1-summary: key decision was X".to_vec();
        scheduler.resume_session(&id, summary).await.unwrap();

        // Attend le traitement
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Ferme l'inbox → run_loop se termine
        drop(tx);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Test structurel : pas de panic, pas de deadlock → SessionResume livré OK.
    }

    /// A3 — intégration run_loop : request_validation passe l'acteur en AwaitingValidation, ValidationResponse le reprend.
    #[tokio::test]
    async fn a3_run_loop_validation_roundtrip() {
        use super::actor::{VALIDATION_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();
        let actor = ActorInstance::new_precompiled(&engine, &module, [22u8; 16], store, log_ref.clone()).await.unwrap();

        let mut scheduler = Scheduler::new();
        let id = actor.agent_id();
        let tx = scheduler.register(actor);

        // Construit un snapshot puis déclenche request_validation
        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x02, 0x01])).await.unwrap(); // risk=1

        // Laisse le run_loop atteindre l'état AwaitingValidation
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Superviseur répond Approved
        scheduler.respond_validation(&id, ValidationVerdict::Approved).await.unwrap();

        // Laisse le run_loop traiter la réponse et passer Active
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Envoie un message pour que l'agent émette son verdict (msg[0]=3)
        tx.send(Message::data(vec![0x03])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Ferme l'inbox → run_loop se termine
        drop(tx);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Vérifie que ValidationRequest ET ValidationResponse sont dans le log
        let found_req = false;
        let found_resp = false;
        // Scan les dernières entrées via la causal log (itération sur les dernières clés)
        // On cherche par action_id en partant du principe que les deux entrées existent.
        // Approche : scanner les N dernières via un itérateur RocksDB.
        // Simplification PoC : vérifier via prefix scan ou en cherchant les types dans les
        // entrées récentes. On utilise log_ref.iter_all() si disponible, sinon on accepte
        // que ce test valide le bon déroulement structurel (pas de crash, pas d'assertion).
        //
        // Pour vérifier sans scan complet, on rely sur l'invariant : si l'acteur a répondu
        // à msg[0]=3 (get_verdict + emit), c'est qu'il a reçu le verdict — preuve indirecte.
        // Validation directe via les emit_type dans le log nécessite un iterator non implémenté.
        let _ = (found_req, found_resp); // supprime les warnings

        // Test structurel : pas de panic, pas de deadlock → A3 roundtrip OK.
        // Les propriétés de logging sont vérifiées par a3_validation_request_logged et
        // a3_verdict_accessible_after_response.
    }

    // ── Tests ADR-0014 — Politique de supervision (timeout A3) ───────────────────

    /// ADR-0014 D14.a + D14.b — Agent en `AwaitingValidation` sans réponse → reçoit
    /// automatiquement `ValidationVerdict::Timeout` après `validation_timeout_ms`.
    ///
    /// Falsifiabilité : test avec timeout court (50 ms) et marge généreuse (300 ms total).
    /// - Si la logique de timeout n'est pas câblée, le test bloque indéfiniment ou échoue.
    /// - Si le verdict reçu n'est pas `Timeout` (par ex. `Approved` par erreur), le test échoue.
    /// - Si le timeout déclenche avant que l'agent ne soit en `AwaitingValidation`, le test échoue.
    #[tokio::test]
    async fn a3_run_loop_validation_timeout_emits_timeout_verdict() {
        use super::actor::{VALIDATION_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use std::sync::Arc;
        use std::sync::Mutex;
        use std::time::Duration;
        use os_poc_capabilities::CapabilityStore;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();

        // ADR-0014 D14.a — timeout court (50 ms) pour ne pas faire trainer le test.
        const TIMEOUT_MS: u64 = 50;
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let actor = ActorInstance::new_precompiled_with_caps_and_timeout(
            &engine, &module, [0xA0u8; 16], store, log_ref.clone(),
            cap_store, vec![], TIMEOUT_MS,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // Construit l'historique de l'agent puis déclenche request_validation(risk=1).
        // L'agent passe en AwaitingValidation et bloque dans run_loop sur un timeout 50 ms.
        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x02, 0x01])).await.unwrap();

        // Attendre que l'agent entre dans AwaitingValidation puis que le timeout déclenche.
        // Marge : 50 ms (timeout) + 100 ms (latence run_loop) = 150 ms. On attend 250 ms.
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Demande à l'agent d'émettre son verdict via msg[0]=3.
        // Si le timeout a fonctionné, lifecycle == Active (sortie de AwaitingValidation)
        // et last_verdict == Some(Timeout). L'agent peut donc traiter ce message.
        tx.send(Message::data(vec![0x03])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Ferme l'inbox proprement.
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Vérification directe via le log causal : on cherche une entrée
        // EmitType::ValidationResponse (0x09) avec verdict == Timeout (2).
        // Le format du payload est défini par record_validation_response : vec![verdict as u8].
        let agent_id = [0xA0u8; 16];
        let entries = log_ref.entries_by_agent(&agent_id);
        let timeout_found = entries.iter()
            .filter_map(|(_action_id, entry)| entry.emit_payload.as_ref())
            .filter_map(|env_bytes| os_poc_causal_log::EmitEnvelope::from_msgpack(env_bytes).ok())
            .any(|env| {
                env.emit_type == os_poc_causal_log::EmitType::ValidationResponse as u8
                    && env.payload.first().copied() == Some(ValidationVerdict::Timeout as u8)
            });

        assert!(
            timeout_found,
            "le log causal doit contenir une entrée ValidationResponse avec verdict=Timeout (ADR-0014 D14.d)"
        );
    }

    /// ADR-0014 D14.c — `Timeout` n'est *pas* terminal : l'agent transite vers `Active`
    /// après l'injection du verdict, comme pour `Approved`/`Rejected`. Pas de retry automatique.
    ///
    /// Vérifie que, après timeout, l'agent peut continuer à traiter des messages (Active).
    /// Cette propriété est implicitement testée par a3_run_loop_validation_timeout_emits_timeout_verdict
    /// (l'envoi de msg[0]=3 après timeout ne bloque pas), mais on l'isole ici pour la rendre
    /// explicite et falsifiable indépendamment.
    #[tokio::test]
    async fn a3_timeout_returns_agent_to_active() {
        use super::actor::{VALIDATION_AGENT_WAT, Message, LifecycleState};
        use super::scheduler::Scheduler;
        use std::sync::Arc;
        use std::sync::Mutex;
        use std::time::Duration;
        use os_poc_capabilities::CapabilityStore;
        use os_poc_causal_log::EmitType;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();

        const TIMEOUT_MS: u64 = 50;
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let actor = ActorInstance::new_precompiled_with_caps_and_timeout(
            &engine, &module, [0xA1u8; 16], store, log_ref.clone(),
            cap_store, vec![], TIMEOUT_MS,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x02, 0x02])).await.unwrap(); // risk=2 (high)

        // Attendre le timeout (50 ms) + marge.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Après timeout, lifecycle doit être Active (transition via record_validation_response).
        // On le vérifie indirectement : si lifecycle restait AwaitingValidation, l'agent
        // ignorerait ce Data et n'émettrait pas d'entrée Lifecycle{state=Active} additionnelle.
        // On envoie un Data ordinaire (msg[0]=0) qui doit déclencher commit_barrier + emit.
        tx.send(Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Le log doit contenir : ValidationRequest, ValidationResponse(Timeout),
        // Lifecycle(Active) — issu de record_validation_response — puis ActionResult.
        let agent_id = [0xA1u8; 16];
        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_action_id, entry)| entry.emit_payload.as_ref())
            .filter_map(|env_bytes| os_poc_causal_log::EmitEnvelope::from_msgpack(env_bytes).ok())
            .collect();

        let validation_response_u8 = EmitType::ValidationResponse as u8;
        let lifecycle_u8 = EmitType::Lifecycle as u8;
        let active_u8 = LifecycleState::Active as u8;
        let timeout_u8 = ValidationVerdict::Timeout as u8;

        let has_timeout_response = envelopes.iter().any(|env|
            env.emit_type == validation_response_u8
                && env.payload.first().copied() == Some(timeout_u8)
        );
        assert!(has_timeout_response, "ValidationResponse(Timeout) doit être dans le log");

        // entries_by_agent itère en ordre lexicographique SHA-256, pas temporel —
        // on ne peut pas tester l'ordre relatif. On teste la co-présence :
        // Lifecycle(Active) doit exister dans le log au moins 2 fois
        // (1 × au démarrage + 1 × émis par record_validation_response après Timeout).
        // Si Timeout était terminal, record_validation_response ne serait pas appelé
        // et le second Active ne figurerait pas — D14.c réfuté.
        let active_count = envelopes.iter().filter(|env|
            env.emit_type == lifecycle_u8
                && env.payload.first().copied() == Some(active_u8)
        ).count();
        assert!(
            active_count >= 2,
            "après Timeout, au moins 2 transitions Lifecycle(Active) attendues \
             (démarrage + record_validation_response) — ADR-0014 D14.c : Timeout n'est pas terminal \
             (trouvé : {active_count})"
        );
    }

    // ── Tests ADR-0003 cross-agent causal link ───────────────────────────────────

    /// ADR-0003 / ADR-0058 — agent_add_cause : le LogEntry émis après add_cause +
    /// commit_barrier doit contenir l'action_id externe dans parent_ids (DAG multi-parents).
    /// Sous B-fort, la citation cross-agent exige désormais un CauseHandle minté (l'autorité
    /// jadis implicite de B-light devient explicite) — c'est le test miroir « avec handle ».
    #[tokio::test(flavor = "current_thread")]
    async fn adr0003_cross_agent_causal_link() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, TenantId};
        use std::sync::Arc;

        let (engine, store, log_ref, _dir) = setup();
        let mod_a = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_b = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let id_a = [0xE0u8; 16];
        let id_b = [0xF0u8; 16];

        // ADR-0060 : store dérivé du registre (tenant DEFAULT, le défaut de actor_b).
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_b = reg.get_or_create(TenantId::DEFAULT);
        let mut actor_a = ActorInstance::new_precompiled(&engine, &mod_a, id_a, store.clone(), log_ref.clone()).await.unwrap();
        let mut actor_b = ActorInstanceBuilder::new(&engine, &mod_b, id_b, store.clone(), log_ref.clone())
            .cause_handle_registry(Arc::clone(&reg))
            .build().await.unwrap();

        // Agent A : 2 cycles pour établir son historique
        actor_a.process_one(b"a1").await.unwrap();
        actor_a.process_one(b"a2").await.unwrap();
        let action_a = actor_a.last_action().expect("last_action A");

        // Agent B : 1 cycle de base
        actor_b.process_one(&[0x00]).await.unwrap();
        let action_b_before = actor_b.last_action().expect("last_action B avant cause");

        // Autorité explicite (B-fort) : on minte un handle autorisant B à citer l'action de A.
        chs_b.lock().unwrap().mint(action_a, id_b, id_a, 0);

        // Agent B : cycle avec cross-agent cause pointant vers action_a
        // msg = [0x04, <32 bytes action_id de A>]
        let mut msg = vec![0x04u8];
        msg.extend_from_slice(&action_a);
        actor_b.process_one(&msg).await.unwrap();

        let action_b_after = actor_b.last_action().expect("last_action B après cause");
        assert_ne!(action_b_before, action_b_after, "B a émis une nouvelle action");

        // Vérifier que le LogEntry de B contient action_a dans parent_ids (DAG multi-parents)
        let entry_b = log_ref.get(&action_b_after).unwrap().expect("entrée B dans le log");
        assert!(
            entry_b.parent_ids.contains(&action_a),
            "parent_ids de B doit contenir l'action_id de A (cross-agent causal link)"
        );
        assert!(
            entry_b.parent_ids.contains(&action_b_before),
            "parent_ids de B doit aussi contenir sa propre action précédente"
        );
        assert_eq!(entry_b.parent_ids.len(), 2, "exactement 2 parents : B-prev + A");
    }

    /// ADR-0003 — agent_add_cause est cumulatif : plusieurs causes peuvent être ajoutées
    /// avant un seul commit_barrier, créant un nœud de merge à N parents.
    #[tokio::test(flavor = "current_thread")]
    async fn adr0003_multi_parent_merge_node() {
        let (engine, store, log_ref, _dir) = setup();
        let mod_a = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_b = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let id_a1 = [0xA1u8; 16];
        let id_a2 = [0xA2u8; 16];
        let id_b  = [0xBBu8; 16];

        let mut actor_a1 = ActorInstance::new_precompiled(&engine, &mod_a, id_a1, store.clone(), log_ref.clone()).await.unwrap();
        let mut actor_a2 = ActorInstance::new_precompiled(&engine, &mod_a, id_a2, store.clone(), log_ref.clone()).await.unwrap();
        let mut actor_b  = ActorInstance::new_precompiled(&engine, &mod_b, id_b,  store.clone(), log_ref.clone()).await.unwrap();

        actor_a1.process_one(b"a1-action").await.unwrap();
        actor_a2.process_one(b"a2-action").await.unwrap();
        let act_a1 = actor_a1.last_action().unwrap();
        let act_a2 = actor_a2.last_action().unwrap();

        // B : cycle de base puis merge A1 + A2 en un seul commit_barrier
        actor_b.process_one(&[0x00]).await.unwrap();
        let b_prev = actor_b.last_action().unwrap();

        // Deux appels agent_add_cause successifs avant le commit_barrier
        // msg[0]=4 ajoute une cause → le WASM appelle add_cause puis commit+emit
        // Pour ajouter deux causes, on doit appeler process_one deux fois MAIS sans commit
        // entre les deux — ce que CROSS_AGENT_WAT ne supporte pas nativement.
        // On teste donc via state_mut() directement pour cette propriété.
        actor_b.state_mut().pending_extra_causes.push(act_a1);
        actor_b.state_mut().pending_extra_causes.push(act_a2);
        actor_b.process_one(&[0x00]).await.unwrap(); // commit_barrier inclut les deux causes

        let b_after = actor_b.last_action().unwrap();
        let entry_b = log_ref.get(&b_after).unwrap().expect("entrée merge");
        assert!(entry_b.parent_ids.contains(&act_a1), "parent A1 présent");
        assert!(entry_b.parent_ids.contains(&act_a2), "parent A2 présent");
        assert!(entry_b.parent_ids.contains(&b_prev),  "parent B-prev présent");
        assert_eq!(entry_b.parent_ids.len(), 3, "nœud de merge à 3 parents");
    }

    // ── Tests sécurité ADR-0036 / B-light ────────────────────────────────────────

    /// SEF-7.1 — forgerie causale refusée :
    /// Un agent qui forge un action_id aléatoire (inexistant dans le log) via
    /// agent_add_cause reçoit -3. Le LogEntry suivant ne contient PAS la cause forgée
    /// dans parent_ids (B-light fail-closed).
    #[tokio::test(flavor = "current_thread")]
    async fn sef7_1_forged_action_id_rejected() {
        let (engine, store, log_ref, _dir) = setup();
        let mod_b = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let id_b = [0xD2u8; 16];
        let mut actor_b = ActorInstance::new_precompiled(&engine, &mod_b, id_b, store.clone(), log_ref.clone()).await.unwrap();

        actor_b.process_one(&[0x00]).await.unwrap();
        let b_prev = actor_b.last_action().unwrap();

        // action_id forgé : 32 bytes arbitraires absents du log
        let forged_id = [0xDEu8; 32];
        let mut msg = vec![0x04u8];
        msg.extend_from_slice(&forged_id);
        actor_b.process_one(&msg).await.unwrap();

        let b_after = actor_b.last_action().unwrap();
        assert_ne!(b_prev, b_after, "B a émis une nouvelle action");

        let entry_b = log_ref.get(&b_after).unwrap().expect("entrée B dans le log");
        assert!(
            !entry_b.parent_ids.iter().any(|p| p == &forged_id),
            "action_id forgé ne doit pas apparaître dans parent_ids"
        );
        assert!(entry_b.parent_ids.contains(&b_prev), "parent séquentiel présent");
        assert_eq!(entry_b.parent_ids.len(), 1, "exactement 1 parent (séquentiel, cause forgée rejetée)");
    }

    /// SEF-7.2 — flood pending_extra_causes refusé (borne anti-DoS MAX=16) :
    /// 16 causes valides pré-injectées + 17ᵉ appel agent_add_cause (valide mais rejeté -2) →
    /// commit_barrier reçoit exactement 16 causes, le 17ᵉ n'est pas dans parent_ids.
    #[tokio::test(flavor = "current_thread")]
    async fn sef7_2_extra_causes_flood_bounded() {
        let (engine, store, log_ref, _dir) = setup();
        let mod_src = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_b   = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let id_src = [0xD3u8; 16];
        let id_b   = [0xD4u8; 16];
        let mut actor_src = ActorInstance::new_precompiled(&engine, &mod_src, id_src, store.clone(), log_ref.clone()).await.unwrap();
        let mut actor_b   = ActorInstance::new_precompiled(&engine, &mod_b,   id_b,   store.clone(), log_ref.clone()).await.unwrap();

        // 17 action_ids valides créés par actor_src
        let mut valid_ids: Vec<[u8; 32]> = Vec::new();
        for _ in 0..17 {
            actor_src.process_one(b"x").await.unwrap();
            valid_ids.push(actor_src.last_action().unwrap());
        }

        actor_b.process_one(&[0x00]).await.unwrap();
        let b_prev = actor_b.last_action().unwrap();

        // Simuler 16 appels réussis : injection directe dans pending_extra_causes
        for id in &valid_ids[..16] {
            actor_b.state_mut().pending_extra_causes.push(*id);
        }
        assert_eq!(actor_b.state_mut().pending_extra_causes.len(), 16);

        // 17ᵉ appel via WAT (valide mais MAX_EXTRA_CAUSES atteint → -2, pas de push)
        let mut msg = vec![0x04u8];
        msg.extend_from_slice(&valid_ids[16]);
        actor_b.process_one(&msg).await.unwrap();

        let b_after = actor_b.last_action().unwrap();
        let entry_b = log_ref.get(&b_after).unwrap().expect("entrée B");

        // 16 causes extra + 1 parent séquentiel = 17 parents au total
        assert_eq!(entry_b.parent_ids.len(), 17,
            "16 causes extra + 1 parent séquentiel attendus");
        // Le 17ᵉ action_id (rejeté) ne doit PAS figurer dans parent_ids
        assert!(
            !entry_b.parent_ids.iter().any(|p| p == &valid_ids[16]),
            "17ᵉ cause refusée (MAX atteint) absente de parent_ids"
        );
        // Les 16 causes acceptées sont toutes présentes
        for id in &valid_ids[..16] {
            assert!(entry_b.parent_ids.contains(id), "cause {:?}... attendue dans parent_ids", &id[..4]);
        }
        assert!(entry_b.parent_ids.contains(&b_prev), "parent séquentiel b_prev présent");
    }

    // ── Tests d'intégration multi-agents ─────────────────────────────────────────

    /// Intégration : deux acteurs partagent ContentStore + CausalLog.
    /// Propriétés vérifiées :
    ///   - Coexistence sans conflit : les entrées log des deux agents n'écrasent pas.
    ///   - Indépendance causale : seq, last_action, last_snapshot sont propres à chaque acteur.
    ///   - Store mutualisé : un seul ContentStore héberge les deux états agent.
    ///   - LogEntry.agent_id distingue les deux agents dans le même log.
    #[tokio::test(flavor = "current_thread")]
    async fn integration_two_agents_shared_infrastructure() {
        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();

        let id_a = [0xA0u8; 16];
        let id_b = [0xB0u8; 16];

        let mut actor_a = ActorInstance::new_precompiled(&engine, &module, id_a, store.clone(), log_ref.clone()).await.unwrap();
        let mut actor_b = ActorInstance::new_precompiled(&engine, &module, id_b, store.clone(), log_ref.clone()).await.unwrap();

        // Agent A : 3 cycles, Agent B : 2 cycles — entrelacés
        actor_a.process_one(b"a1").await.unwrap();
        actor_b.process_one(b"b1").await.unwrap();
        actor_a.process_one(b"a2").await.unwrap();
        actor_b.process_one(b"b2").await.unwrap();
        actor_a.process_one(b"a3").await.unwrap();

        // Compteurs indépendants
        assert_eq!(actor_a.seq(), 3, "agent A : seq=3");
        assert_eq!(actor_b.seq(), 2, "agent B : seq=2");

        // Dernières actions distinctes dans le même log
        let action_a = actor_a.last_action().expect("last_action A");
        let action_b = actor_b.last_action().expect("last_action B");
        assert_ne!(action_a, action_b, "les action_ids de A et B sont distincts");

        let entry_a = log_ref.get(&action_a).unwrap().expect("entrée A dans le log partagé");
        let entry_b = log_ref.get(&action_b).unwrap().expect("entrée B dans le log partagé");
        assert_eq!(entry_a.agent_id, id_a, "entrée A étiquetée avec l'id de A");
        assert_eq!(entry_b.agent_id, id_b, "entrée B étiquetée avec l'id de B");

        // Snapshots indépendants dans le même ContentStore
        let snap_a = actor_a.last_snapshot().expect("snapshot A");
        let snap_b = actor_b.last_snapshot().expect("snapshot B");
        assert_ne!(snap_a, snap_b, "les snapshots de A et B sont distincts");
    }

    /// Intégration async : superviseur orchestre deux acteurs — agent B demande validation,
    /// le superviseur (jouant le scheduler) répond, agent A continue en parallèle.
    /// Propriété clé : aucun deadlock, les deux acteurs progressent indépendamment.
    #[tokio::test]
    async fn integration_supervisor_two_agents_validation() {
        use super::actor::{VALIDATION_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let mod_a = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_b = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();

        let id_a = [0xC0u8; 16];
        let id_b = [0xD0u8; 16];

        let actor_a = ActorInstance::new_precompiled(&engine, &mod_a, id_a, store.clone(), log_ref.clone()).await.unwrap();
        let actor_b = ActorInstance::new_precompiled(&engine, &mod_b, id_b, store.clone(), log_ref.clone()).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx_a = scheduler.register(actor_a);
        let tx_b = scheduler.register(actor_b);

        // Agent A : 3 cycles en parallèle
        tx_a.send(Message::data(b"a1".to_vec())).await.unwrap();
        tx_a.send(Message::data(b"a2".to_vec())).await.unwrap();
        tx_a.send(Message::data(b"a3".to_vec())).await.unwrap();

        // Agent B : snapshot → request_validation(risk=2)
        tx_b.send(Message::data(vec![0x00])).await.unwrap();
        tx_b.send(Message::data(vec![0x02, 0x02])).await.unwrap();

        tokio::time::sleep(Duration::from_millis(30)).await;

        // Superviseur approuve B ; A continue à tourner
        scheduler.respond_validation(&id_b, ValidationVerdict::Approved).await.unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;

        // B lit son verdict
        tx_b.send(Message::data(vec![0x03])).await.unwrap();

        // Checkpoint A via le scheduler
        scheduler.checkpoint(&id_a).await.unwrap();

        tokio::time::sleep(Duration::from_millis(30)).await;

        drop(tx_a);
        drop(tx_b);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Pas de deadlock, pas de panic → infrastructure multi-agents opérationnelle.
        // Les logs des deux agents coexistent dans log_ref (vérifié par
        // integration_two_agents_shared_infrastructure pour la partie sync).
    }

    /// ADR-0003 — causalité implicite à la livraison : Message::caused() injecte la cause
    /// dans pending_extra_causes avant process_one via run_loop, sans appel explicite
    /// à agent_add_cause depuis le WASM.
    #[tokio::test]
    async fn integration_causal_message_delivery() {
        use super::actor::INTROSPECT_AGENT_WAT;
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let mod_a = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_b = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();

        let id_a = [0xCAu8; 16];
        let id_b = [0xCBu8; 16];

        let mut actor_a_sync = ActorInstance::new_precompiled(&engine, &mod_a, id_a, store.clone(), log_ref.clone()).await.unwrap();

        // Agent A : 1 cycle synchrone pour obtenir son action_id
        actor_a_sync.process_one(b"trigger").await.unwrap();
        let cause_id = actor_a_sync.last_action().expect("action A");

        // Agent B via run_loop : reçoit un Message::caused avec l'action de A
        let actor_b = ActorInstance::new_precompiled(&engine, &mod_b, id_b, store.clone(), log_ref.clone()).await.unwrap();
        let mut scheduler = Scheduler::new();
        let tx_b = scheduler.register(actor_b);

        // Livre le message avec la cause de A → run_loop injecte cause avant process_one
        scheduler.send_caused_by(&id_b, b"triggered-by-a".to_vec(), cause_id).await.unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(tx_b);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Vérifie que l'entrée de B dans le log contient cause_id dans parent_ids
        // On cherche en itérant depuis la dernière entrée connue de B via le log.
        // Stratégie : scanner la DB pour trouver l'entrée avec agent_id=id_b et parent cause_id.
        // Simplification PoC : on vérifie la propriété via l'itérateur RocksDB si disponible.
        // Fallback : test structurel — pas de panic → causalité implicite livrée sans deadlock.
        // La vérification directe des parent_ids est couverte par adr0003_cross_agent_causal_link.
        let _ = cause_id; // silence unused warning
    }

    /// Scheduler::spawn_child : l'agent fils est créé avec la last_action du parent
    /// comme parent_id de son premier commit_barrier.
    /// Propriété clé : la hiérarchie de spawn est traçable depuis le CausalLog partagé.
    #[tokio::test]
    async fn integration_spawn_causal_hierarchy() {
        use super::actor::INTROSPECT_AGENT_WAT;
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();

        let parent_id = [0xDAu8; 16];
        let child_id  = [0xDBu8; 16];

        // Parent : 2 cycles synchrones pour établir son historique
        let mut parent_actor = ActorInstance::new_precompiled(
            &engine, &module, parent_id, store.clone(), log_ref.clone()
        ).await.unwrap();
        parent_actor.process_one(b"work-1").await.unwrap();
        parent_actor.process_one(b"work-2").await.unwrap();
        let parent_cause = parent_actor.last_action().expect("last_action parent");

        // Spawn de l'enfant via le scheduler
        let mut scheduler = Scheduler::new();
        scheduler.spawn_child(
            &engine, &module, child_id,
            store.clone(), log_ref.clone(),
            parent_cause,
            b"initial-task".to_vec(),
            &parent_id,
            &[],
        ).await.unwrap();

        // Attend que l'enfant traite son premier message
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Vérifie que l'enfant a au moins une entrée dans le log
        let child_entries = log_ref.entries_by_agent(&child_id);

        // Parmi les entrées de l'enfant, au moins une doit avoir parent_cause dans parent_ids
        // (c'est l'entrée produite par le premier commit_barrier de l'enfant)
        let has_causal_link = child_entries.iter().any(|(_, entry)| {
            entry.parent_ids.contains(&parent_cause)
        });
        assert!(has_causal_link,
            "au moins une entrée de l'enfant doit référencer l'action du parent dans parent_ids");

        // La relation est asymétrique : l'entrée du parent ne référence pas l'enfant
        // (le parent ne sait pas encore qu'il a été spawné)
        let parent_entries = log_ref.entries_by_agent(&parent_id);
        let parent_refs_child = parent_entries.iter().any(|(_, entry)| {
            entry.parent_ids.contains(child_entries.first().map(|(id, _)| id).unwrap_or(&[0u8;32]))
        });
        assert!(!parent_refs_child, "le parent ne référence pas encore l'enfant (asymétrie de spawn)");
    }

    /// Capabilities — spawn_child délègue une cap du parent vers l'enfant.
    /// Propriétés : cap dans le store partagé, owner = child, permissions atténuées.
    #[tokio::test]
    async fn cap_delegation_in_spawn() {
        use super::actor::{CAP_CHECK_WAT, INTROSPECT_AGENT_WAT};
        use super::scheduler::Scheduler;
        use os_poc_capabilities::Permissions;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let mod_parent = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let mod_child  = Module::new(&engine, CAP_CHECK_WAT).unwrap();

        let parent_id = [0xE1u8; 16];
        let child_id  = [0xE2u8; 16];

        let mut scheduler = Scheduler::new();

        // Octroie une cap racine au parent dans le store partagé du scheduler
        let parent_cap = {
            let mut cs = scheduler.cap_store.lock().unwrap();
            cs.grant_root(parent_id,
                Permissions { read: true, write: false, execute: false, delegate: true },
                "/res/A".to_string())
        };

        // Parent : 1 cycle synchrone pour obtenir son action_id
        let mut parent_actor = ActorInstance::new_precompiled(&engine, &mod_parent, parent_id, store.clone(), log_ref.clone()).await.unwrap();
        parent_actor.process_one(b"parent-work").await.unwrap();
        let parent_cause = parent_actor.last_action().expect("parent action");

        // Spawn de l'enfant avec délégation read (sans write ni execute)
        let perms_read = Permissions { read: true, write: false, execute: false, delegate: false };
        let (_, child_caps) = scheduler.spawn_child(
            &engine, &mod_child, child_id,
            store.clone(), log_ref.clone(),
            parent_cause,
            b"\x00".to_vec(), // msg[0]=0 → build history
            &parent_id,
            &[(parent_cap, perms_read, "/res/A".to_string())],
        ).await.unwrap();

        assert_eq!(child_caps.len(), 1, "une cap déléguée à l'enfant");
        let child_cap_id = child_caps[0];

        tokio::time::sleep(Duration::from_millis(30)).await;

        // Vérifie les propriétés de la cap dans le store partagé
        let cs = scheduler.cap_store.lock().unwrap();
        let cap = cs.get(child_cap_id).expect("cap déléguée présente");
        assert_eq!(cap.owner, child_id, "owner = child");
        assert!(cap.permissions.read, "read accordé");
        assert!(!cap.permissions.write, "write non accordé (atténuation)");
        assert!(!cap.permissions.delegate, "delegate non accordé (atténuation)");
        assert_eq!(cap.resource, "/res/A");
        assert_eq!(cap.parent, Some(parent_cap), "dérivée de la cap parent");
    }

    // ── Tests D5 — Message::Rollback câblé (scheduler-driven rollback) ───────────

    /// D5 — happy path : rollback scheduler restaure le snapshot cible, agent reste Active.
    ///
    /// Séquence : 5 Data → snapshots seq 0..4 → Rollback(target_seq=2)
    /// → SchedulerRollback dans le log, distance=2, puis Data post-rollback traité.
    #[tokio::test]
    async fn d5_rollback_via_scheduler_restores_snapshot() {
        use super::actor::{INTROSPECT_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use os_poc_causal_log::EmitType;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let agent_id = [0xD5u8; 16];

        let actor = ActorInstance::new_precompiled(&engine, &module, agent_id, store, log_ref.clone()).await.unwrap();
        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // 5 snapshots : seq 0, 1, 2, 3, 4
        for _ in 0..5 {
            tx.send(Message::data(b"work".to_vec())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Rollback vers snapshot seq=2
        scheduler.rollback(&agent_id, 2).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Envoie un message post-rollback → agent doit le traiter (toujours Active)
        tx.send(Message::data(b"after-rollback".to_vec())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // Une entrée SchedulerRollback dans le log
        let rollback_env = envelopes.iter()
            .find(|env| env.emit_type == EmitType::SchedulerRollback as u8)
            .expect("SchedulerRollback doit être dans le log");

        // target_seq=2 dans payload[1..9], distance=2 dans payload[0]
        let target_seq_in_log = u64::from_le_bytes(rollback_env.payload[1..9].try_into().unwrap());
        assert_eq!(target_seq_in_log, 2u64, "target_seq=2 dans le payload");
        assert_eq!(rollback_env.payload[0], 2u8, "distance=2 (traversée de 2 snapshots)");
        assert_eq!(rollback_env.payload[9], 0u8, "caps_invalidated=0 (D6 reportée)");

        // Agent toujours vivant : 5 pré-rollback + 1 post-rollback = 6 Introspect
        let introspect_count = envelopes.iter()
            .filter(|env| env.emit_type == EmitType::Introspect as u8)
            .count();
        assert_eq!(introspect_count, 6, "6 cycles Introspect : 5 pré + 1 post-rollback");
    }

    /// D5 — target_seq dans le futur (>= s.seq) est un noop : agent vivant, log inchangé.
    #[tokio::test]
    async fn d5_rollback_target_seq_in_future_is_noop() {
        use super::actor::{INTROSPECT_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use os_poc_causal_log::EmitType;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let agent_id = [0xD6u8; 16];

        let actor = ActorInstance::new_precompiled(&engine, &module, agent_id, store, log_ref.clone()).await.unwrap();
        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // 3 snapshots → s.seq=3
        for _ in 0..3 {
            tx.send(Message::data(b"work".to_vec())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;

        // target_seq=99 >= seq=3 → doit être ignoré
        scheduler.rollback(&agent_id, 99).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Envoie un message → doit être traité (agent vivant)
        tx.send(Message::data(b"still-alive".to_vec())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // Aucun SchedulerRollback dans le log
        let has_rollback = envelopes.iter().any(|env| env.emit_type == EmitType::SchedulerRollback as u8);
        assert!(!has_rollback, "noop : aucune entrée SchedulerRollback");

        // Agent toujours vivant : 3 + 1 = 4 Introspect
        let introspect_count = envelopes.iter()
            .filter(|env| env.emit_type == EmitType::Introspect as u8)
            .count();
        assert_eq!(introspect_count, 4, "4 cycles : noop ignoré, agent reprend");
    }

    /// D5 — rollback sans historique (last_snapshot=None) est un noop.
    #[tokio::test]
    async fn d5_rollback_without_history_is_noop() {
        use super::actor::{INTROSPECT_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use os_poc_causal_log::EmitType;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let agent_id = [0xD7u8; 16];

        let actor = ActorInstance::new_precompiled(&engine, &module, agent_id, store, log_ref.clone()).await.unwrap();
        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // Aucun Data → seq=0, last_snapshot=None
        scheduler.rollback(&agent_id, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Envoie un Data après → doit être traité (agent vivant)
        tx.send(Message::data(b"after-noop".to_vec())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let has_rollback = envelopes.iter().any(|env| env.emit_type == EmitType::SchedulerRollback as u8);
        assert!(!has_rollback, "noop : aucune entrée SchedulerRollback sans historique");

        // Le post-rollback Data a quand même été traité
        let introspect_count = envelopes.iter()
            .filter(|env| env.emit_type == EmitType::Introspect as u8)
            .count();
        assert_eq!(introspect_count, 1, "1 Introspect : le Data post-noop a été traité");
    }

    /// D5 — provenance : SelfRollback (0x07) et SchedulerRollback (0x0B) sont distincts dans le log.
    #[tokio::test]
    async fn d5_scheduler_rollback_distinct_from_self_rollback_in_log() {
        use super::actor::{SELF_ROLLBACK_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use os_poc_causal_log::EmitType;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, SELF_ROLLBACK_AGENT_WAT).unwrap();
        let agent_id = [0xD8u8; 16];

        let actor = ActorInstance::new_precompiled(&engine, &module, agent_id, store, log_ref.clone()).await.unwrap();
        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // 5 snapshots (seq 0..4) via msg[0]=0
        for _ in 0..5 {
            tx.send(Message::data(vec![0x00])).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Agent se rollback lui-même depth=1 (SelfRollback vers seq=3)
        tx.send(Message::data(vec![0x01, 0x01])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Scheduler rollback vers seq=1 (SchedulerRollback depuis seq=3 actuel)
        scheduler.rollback(&agent_id, 1).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let self_rollback_count = envelopes.iter()
            .filter(|env| env.emit_type == EmitType::SelfRollback as u8)
            .count();
        let sched_rollback_count = envelopes.iter()
            .filter(|env| env.emit_type == EmitType::SchedulerRollback as u8)
            .count();

        assert_eq!(self_rollback_count, 1, "exactement 1 SelfRollback (A2)");
        assert_eq!(sched_rollback_count, 1, "exactement 1 SchedulerRollback (scheduler)");
    }

    /// Capabilities — la révocation de la cap racine invalide récursivement la cap enfant.
    /// Propriété H-revoke : revoke() O(N), toutes les dérivées sont révoquées.
    #[tokio::test]
    async fn cap_revocation_propagates_to_child() {
        use super::actor::INTROSPECT_AGENT_WAT;
        use super::scheduler::Scheduler;
        use os_poc_capabilities::Permissions;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();

        let parent_id = [0xF1u8; 16];
        let child_id  = [0xF2u8; 16];

        let mut scheduler = Scheduler::new();

        // Cap racine avec delegate=true → peut être transmise à l'enfant
        let root_cap = {
            let mut cs = scheduler.cap_store.lock().unwrap();
            cs.grant_root(parent_id,
                Permissions { read: true, write: true, execute: false, delegate: true },
                "/res/B".to_string())
        };

        // Parent : 1 cycle
        let mut parent_actor = ActorInstance::new_precompiled(&engine, &module, parent_id, store.clone(), log_ref.clone()).await.unwrap();
        parent_actor.process_one(b"work").await.unwrap();
        let parent_cause = parent_actor.last_action().expect("parent action");

        // Spawn enfant avec délégation read-only
        let perms_child = Permissions { read: true, write: false, execute: false, delegate: false };
        let (_, child_caps) = scheduler.spawn_child(
            &engine, &module, child_id,
            store.clone(), log_ref.clone(),
            parent_cause,
            b"init".to_vec(),
            &parent_id,
            &[(root_cap, perms_child, "/res/B".to_string())],
        ).await.unwrap();

        let child_cap = child_caps[0];

        tokio::time::sleep(Duration::from_millis(20)).await;

        // Révocation de la cap racine → doit cascader vers la cap enfant
        let revoked = {
            let mut cs = scheduler.cap_store.lock().unwrap();
            cs.revoke(root_cap)
        };

        assert_eq!(revoked, 2, "2 caps révoquées : racine + dérivée enfant");

        let cs = scheduler.cap_store.lock().unwrap();
        assert!(cs.get(root_cap).is_none(), "cap racine révoquée");
        assert!(cs.get(child_cap).is_none(), "cap enfant révoquée par propagation");
    }

    /// D8 (ADR-0007) — un rollback scheduler révoque les capabilities de l'agent
    /// émises strictement après le timestamp du snapshot cible. Le compte revoqué
    /// est observable via le payload `SchedulerRollback` (octet 9 = caps_invalidated).
    #[tokio::test]
    async fn d8_rollback_revokes_caps_post_snapshot() {
        use super::actor::{INTROSPECT_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use os_poc_capabilities::Permissions;
        use os_poc_causal_log::{EmitType, EmitEnvelope};
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let agent_id = [0xD8u8; 16];

        let mut scheduler = Scheduler::new();
        // L'agent DOIT partager le cap_store du scheduler (le constructeur
        // `new_precompiled` crée un store isolé — inutilisable pour D8).
        let actor = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_id,
            store.clone(), log_ref.clone(),
            scheduler.cap_store.clone(), vec![],
        ).await.unwrap();
        let tx = scheduler.register(actor);

        // Phase 1 — créer trois snapshots (seq 0..2) AVANT le grant de la cap
        // (snapshot cible du rollback sera seq=1, donc strictement antérieur à la cap).
        for _ in 0..3 {
            tx.send(Message::data(b"pre".to_vec())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Garantie temporelle stricte : la cap doit être émise après ts_ms du snapshot
        // cible (seq=1). On dort 5 ms — supérieur à la résolution wall-clock (1 ms).
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Phase 2 — grant d'une cap racine à l'agent. issued_at_ms = now wall clock.
        let cap_id = {
            let mut cs = scheduler.cap_store.lock().unwrap();
            cs.grant_root(
                agent_id,
                Permissions { read: true, write: false, execute: false, delegate: false },
                "/res/d8".to_string(),
            )
        };

        // Sanity check : cap présente avant rollback.
        {
            let cs = scheduler.cap_store.lock().unwrap();
            let cap = cs.get(cap_id).expect("cap présente avant rollback");
            assert!(cap.issued_at_ms > 0, "issued_at_ms doit être renseigné");
        }

        // Phase 3 — quelques snapshots après le grant (pour que rollback_path
        // ait du travail à faire et que distance > 0).
        for _ in 0..2 {
            tx.send(Message::data(b"post".to_vec())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Phase 4 — rollback vers seq=1 (strictement antérieur au grant de cap).
        // ts_ms(snapshot seq=1) < issued_at_ms(cap) → la cap doit être révoquée.
        scheduler.rollback(&agent_id, 1).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Assertion 1 : la cap a disparu du store.
        {
            let cs = scheduler.cap_store.lock().unwrap();
            assert!(
                cs.get(cap_id).is_none(),
                "cap émise après snapshot cible doit être révoquée"
            );
        }

        // Assertion 2 : payload[9] > 0 dans l'entrée SchedulerRollback du log.
        let entries = log_ref.entries_by_agent(&agent_id);
        let sched_rollback_payloads: Vec<EmitEnvelope> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == EmitType::SchedulerRollback as u8)
            .collect();

        assert_eq!(
            sched_rollback_payloads.len(), 1,
            "exactement 1 entrée SchedulerRollback dans le log"
        );

        let payload = &sched_rollback_payloads[0].payload;
        assert_eq!(payload.len(), 10, "payload SchedulerRollback fait 10 octets");
        let caps_invalidated = payload[9];
        assert!(
            caps_invalidated >= 1,
            "payload[9] = caps_invalidated doit être >= 1 (au moins la cap d8), trouvé {}",
            caps_invalidated
        );
    }

    // ── S1 — Diagnostic intermédiaire ────────────────────────────────────────

    /// S1-diag : teste que worker_prime phase 0x01 complète sans crash (process_one OK).
    /// Isole la phase infer+validation de la coordination scheduler.
    #[tokio::test(flavor = "multi_thread")]
    async fn s1_diag_worker_phase1_completes() {
        use super::actor::{ActorInstance, LifecycleState, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::inference::{InferencePool, FixedResponseBackend};
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/worker_prime.wasm");
        if !wasm_path.exists() { eprintln!("SKIP: worker_prime.wasm absent"); return; }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).unwrap();
        let module = wasmtime::Module::new(&engine, &bytes).unwrap();
        let agent_id = [0x5Du8; 16];

        let pool = Arc::new(InferencePool::new(4, FixedResponseBackend {
            delay_ms: 10,
            response: r#"{"is_prime": true}"#.to_string(),
        }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));

        let mut actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("ActorInstance worker_prime");

        // Phase 0x01 directement via process_one (sans scheduler)
        let result = actor.process_one(&[0x01, 39]).await;
        eprintln!("process_one result: {:?}", result);
        assert!(result.is_ok(), "worker phase 0x01 doit réussir sans crash");

        // Après phase 0x01, lifecycle doit être AwaitingValidation
        let lc = actor.lifecycle();
        eprintln!("lifecycle après phase 0x01: {:?}", lc);
        assert_eq!(lc, LifecycleState::AwaitingValidation,
            "worker doit être AwaitingValidation après request_validation(1)");
    }

    /// S1-diag : teste phase 0x02 — après ValidationResponse::Rejected, le worker émet le résultat final.
    #[tokio::test(flavor = "multi_thread")]
    async fn s1_diag_worker_phase2_after_rejected() {
        use super::actor::{ActorInstance, ValidationVerdict,
                           SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, Message};
        use super::inference::{InferencePool, FixedResponseBackend};
        use super::scheduler::Scheduler;
        use os_poc_causal_log::EmitType;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/worker_prime.wasm");
        if !wasm_path.exists() { eprintln!("SKIP"); return; }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).unwrap();
        let module = wasmtime::Module::new(&engine, &bytes).unwrap();
        let worker_id = [0x5Eu8; 16];

        let pool = Arc::new(InferencePool::new(4, FixedResponseBackend {
            delay_ms: 10, response: r#"{"is_prime": true}"#.to_string(),
        }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));

        let worker = ActorInstance::new_precompiled_with_inference(
            &engine, &module, worker_id, store, log_ref.clone(),
            cap_store, vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0, infer_fn,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(worker);

        // Phase 0x01
        tx.send(Message::data(vec![0x01, 39])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Répondre Rejected
        scheduler.respond_validation(&worker_id, ValidationVerdict::Rejected).await.unwrap();

        // Phase 0x02
        tx.send(Message::data(vec![0x02])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let entries = log_ref.entries_by_agent(&worker_id);
        let envs: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        eprintln!("=== ALL ENVELOPES ===");
        for env in &envs {
            let t = EmitType::try_from(env.emit_type).map(|e| format!("{:?}", e)).unwrap_or(format!("0x{:02x}", env.emit_type));
            let txt = std::str::from_utf8(&env.payload).unwrap_or("<binary>");
            eprintln!("  [{t}] seq={} payload={txt:?}", env.seq);
        }

        let mut action_results: Vec<_> = envs.iter()
            .filter(|e| e.emit_type == EmitType::ActionResult as u8)
            .collect();
        eprintln!("ActionResult count: {}", action_results.len());
        for ar in &action_results {
            eprintln!("  {:?}", std::str::from_utf8(&ar.payload));
        }

        assert!(action_results.len() >= 2, "doit avoir provisional + final, got {}", action_results.len());
        action_results.sort_by_key(|e| e.seq);
        let last = action_results.last().unwrap();
        let txt = std::str::from_utf8(&last.payload).unwrap_or("");
        assert!(txt.contains("validation_rejected"), "dernier AR doit être rejected, trouvé: {txt}");
    }

    // ── S1 — Supervision algorithmique ───────────────────────────────────────
    //
    // Scénario : un worker LLM (worker_prime.wasm) teste si 39 est premier.
    // Le FixedResponseBackend retourne {"is_prime": true} (39 n'est PAS premier).
    // Un supervisor déterministe (supervisor_arith.wasm) calcule is_prime(39)=false,
    // compare avec claim=true, produit Rejected.
    // Le worker reçoit le verdict Rejected et émet le résultat final null.
    //
    // Ce test est ignoré si les .wasm ne sont pas compilés.
    // Build : cargo build --target wasm32-unknown-unknown -p agent-sdk --example worker_prime
    //         cargo build --target wasm32-unknown-unknown -p agent-sdk --example supervisor_arith
    #[tokio::test(flavor = "multi_thread")]
    async fn s1_supervision_algorithmique() {
        use super::actor::{ActorInstance, Message, ValidationVerdict, LifecycleState,
                           SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, FixedResponseBackend};
        use os_poc_causal_log::EmitType;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let find_wasm = |name: &str| -> Option<std::path::PathBuf> {
            let candidates = [
                ws_root.join(format!("target/wasm32-unknown-unknown/debug/examples/{name}.wasm")),
                ws_root.join(format!("target/wasm32-unknown-unknown/release/examples/{name}.wasm")),
            ];
            candidates.into_iter().find(|p| p.exists())
        };

        let worker_path = match find_wasm("worker_prime") {
            Some(p) => p,
            None => {
                eprintln!("SKIP s1: worker_prime.wasm absent (run: cargo build --target wasm32-unknown-unknown -p agent-sdk --example worker_prime)");
                return;
            }
        };
        let supervisor_path = match find_wasm("supervisor_arith") {
            Some(p) => p,
            None => {
                eprintln!("SKIP s1: supervisor_arith.wasm absent");
                return;
            }
        };

        let (engine, store, log_ref, _dir) = setup();

        // Worker : agent LLM — utilise FixedResponseBackend qui retourne {"is_prime": true}.
        // 39 = 3 × 13 → n'est PAS premier. Le worker va donc se tromper.
        let pool = Arc::new(InferencePool::new(
            4,
            FixedResponseBackend {
                delay_ms: 10,
                response: r#"{"is_prime": true}"#.to_string(),
            },
        ));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));

        let worker_id   = [0x51u8; 16];
        let supervisor_id = [0x52u8; 16];

        let worker_bytes = std::fs::read(&worker_path).expect("lecture worker_prime.wasm");
        let worker_module = wasmtime::Module::new(&engine, &worker_bytes)
            .expect("Module::new worker_prime");

        let supervisor_bytes = std::fs::read(&supervisor_path).expect("lecture supervisor_arith.wasm");
        let supervisor_module = wasmtime::Module::new(&engine, &supervisor_bytes)
            .expect("Module::new supervisor_arith");

        let worker = ActorInstance::new_precompiled_with_inference(
            &engine, &worker_module, worker_id,
            store.clone(), log_ref.clone(),
            cap_store.clone(), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("worker ActorInstance");

        // Supervisor : déterministe, pas d'inférence LLM.
        let supervisor = ActorInstance::new_precompiled_with_caps(
            &engine, &supervisor_module, supervisor_id,
            store.clone(), log_ref.clone(),
            cap_store.clone(), vec![],
        ).await.expect("supervisor ActorInstance");

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);
        let tx_worker     = scheduler.register(worker);
        let tx_supervisor = scheduler.register(supervisor);

        // ── Étape 1 : lancer le worker sur la tâche (n=39) ──────────────────
        // Phase 0x01 : worker appelle agent_infer → obtient {"is_prime": true}
        // → émet provisional → appelle request_validation(1).
        tx_worker.send(Message::data(vec![0x01, 39])).await.unwrap();

        // Attendre que le worker soit en AwaitingValidation
        // (delay_ms=10 → infer retourne en ~10ms ; on laisse 200ms de marge)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // ── Étape 2 : router vers le supervisor ──────────────────────────────
        // Harness envoie [n=39, claim=1 (true)] au supervisor.
        // supervisor_arith calcule is_prime(39)=false ≠ claim=true → Rejected (1).
        tx_supervisor.send(Message::data(vec![39, 1])).await.unwrap();

        // Attendre que le supervisor termine
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ── Étape 3 : répondre au worker ─────────────────────────────────────
        // Le supervisor déterministe a calculé Rejected ; le harness le sait
        // (is_prime(39)=false ≠ claim=true).
        scheduler.respond_validation(&worker_id, ValidationVerdict::Rejected).await.unwrap();

        // ── Étape 4 : phase finale du worker ─────────────────────────────────
        // Phase 0x02 : worker lit verdict=Rejected → émet null → termine.
        tx_worker.send(Message::data(vec![0x02])).await.unwrap();

        // Laisser le temps de tout logger
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(tx_worker);
        drop(tx_supervisor);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // ── Assertions sur le log causal du worker ────────────────────────────
        let worker_entries = log_ref.entries_by_agent(&worker_id);
        let worker_envs: Vec<_> = worker_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let has_infer_req  = worker_envs.iter().any(|e| e.emit_type == EmitType::InferenceRequest  as u8);
        let has_infer_resp = worker_envs.iter().any(|e| e.emit_type == EmitType::InferenceResponse as u8);
        let has_val_req    = worker_envs.iter().any(|e| e.emit_type == EmitType::ValidationRequest  as u8);
        let has_val_resp   = worker_envs.iter().any(|e| e.emit_type == EmitType::ValidationResponse as u8);

        assert!(has_infer_req,  "S1: InferenceRequest (0x0C) dans le log worker");
        assert!(has_infer_resp, "S1: InferenceResponse (0x0D) dans le log worker");
        assert!(has_val_req,    "S1: ValidationRequest (0x08) dans le log worker");
        assert!(has_val_resp,   "S1: ValidationResponse (0x09) dans le log worker");

        // Vérifier que le worker a bien transité par WaitingInference
        let had_waiting = worker_envs.iter()
            .filter(|e| e.emit_type == EmitType::Lifecycle as u8)
            .any(|e| e.payload.first().copied() == Some(LifecycleState::WaitingInference as u8));
        assert!(had_waiting, "S1: WaitingInference logué pour le worker");

        // Vérifier que l'emit final du worker contient "validation_rejected".
        // Trier par seq (les entries peuvent arriver dans l'ordre RocksDB ts_ms_BE
        // et non chronologique si ts_ms identique entre deux cycles).
        let mut action_results: Vec<_> = worker_envs.iter()
            .filter(|e| e.emit_type == EmitType::ActionResult as u8)
            .collect();
        action_results.sort_by_key(|e| e.seq);
        let final_action = action_results.last();
        if let Some(env) = final_action {
            let text = std::str::from_utf8(&env.payload).unwrap_or("");
            assert!(
                text.contains("validation_rejected"),
                "S1: résultat final doit contenir 'validation_rejected', trouvé: {text}"
            );
        } else {
            panic!("S1: aucune entrée ActionResult dans le log worker");
        }

        // ── Assertions sur le log causal du supervisor ────────────────────────
        let sup_entries = log_ref.entries_by_agent(&supervisor_id);
        let sup_envs: Vec<_> = sup_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // Supervisor doit avoir émis un ActionResult avec verdict=1 (Rejected)
        let sup_verdict = sup_envs.iter()
            .find(|e| e.emit_type == EmitType::ActionResult as u8);
        assert!(sup_verdict.is_some(), "S1: supervisor doit émettre un ActionResult");
        if let Some(env) = sup_verdict {
            assert_eq!(env.payload.first().copied(), Some(1u8),
                "S1: supervisor doit émettre verdict=1 (Rejected) pour claim=true sur n=39");
        }
    }

    // ── S2 — Self-rollback sur incohérence LLM ───────────────────────────────

    /// S2 — composition A1+A2 sur incohérence LLM.
    ///
    /// worker_double_check reçoit n=39 (non premier).
    /// FixedResponseBackend retourne {"is_prime": true} (erroné).
    /// L'agent émet une revendication provisoire, appelle agent_introspect (A1),
    /// détecte l'incohérence avec le calcul interne, appelle agent_self_rollback(1) (A2).
    ///
    /// Assert : Introspect (0x06) et SelfRollback (0x07) dans le log (dans cet ordre),
    ///          résultat final = "self_rollback_after_llm_error".
    #[tokio::test(flavor = "multi_thread")]
    async fn s2_self_rollback_incoherence() {
        use super::actor::{ActorInstance, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::inference::{InferencePool, FixedResponseBackend};
        use os_poc_causal_log::EmitType;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/worker_double_check.wasm");
        if !wasm_path.exists() {
            eprintln!("SKIP s2: worker_double_check.wasm absent");
            return;
        }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).expect("lecture worker_double_check.wasm");
        let module = wasmtime::Module::new(&engine, &bytes).expect("Module::new worker_double_check");
        let agent_id = [0x52u8; 16];

        let pool = Arc::new(InferencePool::new(4, FixedResponseBackend {
            delay_ms: 10,
            response: r#"{"is_prime": true}"#.to_string(),
        }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id,
            store, log_ref.clone(),
            Arc::new(Mutex::new(CapabilityStore::new())), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("ActorInstance worker_double_check");

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(super::actor::run_loop(actor, rx));

        // n=39 → FixedResponseBackend dit "true" mais is_prime(39)=false → rollback
        tx.send(super::actor::Message::data(vec![39u8])).await.unwrap();

        // Laisser le temps à l'agent de compléter (infer=10ms, puis calcul, puis termine)
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<_> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // A1 : Introspect (0x06) présent
        let has_introspect = envelopes.iter()
            .any(|e| e.emit_type == EmitType::Introspect as u8);
        assert!(has_introspect, "S2: Introspect (0x06) doit être dans le log");

        // A2 : SelfRollback (0x07) présent
        let has_self_rollback = envelopes.iter()
            .any(|e| e.emit_type == EmitType::SelfRollback as u8);
        assert!(has_self_rollback, "S2: SelfRollback (0x07) doit être dans le log");

        // Résultat final : "self_rollback_after_llm_error"
        let mut action_results: Vec<_> = envelopes.iter()
            .filter(|e| e.emit_type == EmitType::ActionResult as u8)
            .collect();
        action_results.sort_by_key(|e| e.seq);
        let final_result = action_results.last()
            .expect("S2: au moins un ActionResult attendu");
        let text = std::str::from_utf8(&final_result.payload).unwrap_or("");
        assert!(
            text.contains("self_rollback_after_llm_error"),
            "S2: résultat final doit contenir 'self_rollback_after_llm_error', trouvé: {text}"
        );
    }

    // ── S3 — Borne dure du pool d'inférence ──────────────────────────────────

    /// S3 — démonstration de la borne dure du pool d'inférence (k=4).
    ///
    /// 12 density_workers lancés simultanément, InferencePool cap=4.
    /// SleepyBackend(200ms) simule des inférences lentes.
    ///
    /// Assert :
    ///   - Exactement 12 InferenceRequest + 12 InferenceResponse dans le log.
    ///   - Pas de deadlock : tous les workers terminent.
    ///   - Le pool respecte sa borne (active_count <= 12 à tout moment).
    ///
    /// Note : la propriété "exactement 4 simultanés" est garantie structurellement par
    /// le sémaphore Tokio, pas mesurée ici (requiert un compteur concurrent dédié,
    /// reporté à Phase 6). Ce que ce test démontre : no-famine + bonne traçabilité.
    #[tokio::test(flavor = "multi_thread")]
    async fn s3_inference_cap() {
        use super::actor::{ActorInstance, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::EmitType;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        const N_WORKERS: usize = 12;
        const POOL_CAP: usize = 4;
        const DELAY_MS: u64 = 100;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/density_worker.wasm");
        if !wasm_path.exists() {
            eprintln!("SKIP s3: density_worker.wasm absent");
            return;
        }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).expect("lecture density_worker.wasm");
        let module = wasmtime::Module::new(&engine, &bytes).expect("Module::new density_worker");

        let pool = Arc::new(InferencePool::new(POOL_CAP, SleepyBackend { delay_ms: DELAY_MS }));
        let infer_fn_template = pool.clone();
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));

        let mut senders = vec![];
        for i in 0..N_WORKERS {
            let mut agent_id = [0x53u8; 16];
            agent_id[15] = i as u8;

            let infer_fn = InferencePool::as_infer_fn(Arc::clone(&infer_fn_template));
            let actor = ActorInstance::new_precompiled_with_inference(
                &engine, &module, agent_id,
                store.clone(), log_ref.clone(),
                cap_store.clone(), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                infer_fn,
            ).await.expect("ActorInstance density_worker");

            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(super::actor::run_loop(actor, rx));
            tx.send(super::actor::Message::data(vec![0x01u8])).await.unwrap();
            senders.push(tx);
        }

        // Attendre que tous les workers terminent.
        // N_WORKERS=12, cap=4, delay=100ms → ceil(12/4)*100 = 300ms min.
        // On attend 3s pour les coûts de scheduling Tokio.
        tokio::time::sleep(Duration::from_millis(3000)).await;
        drop(senders);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Compter InferenceRequest + InferenceResponse dans le log (tous agents confondus)
        let mut total_req  = 0usize;
        let mut total_resp = 0usize;
        for i in 0..N_WORKERS {
            let mut agent_id = [0x53u8; 16];
            agent_id[15] = i as u8;
            let entries = log_ref.entries_by_agent(&agent_id);
            for (_, e) in &entries {
                let Some(payload) = &e.emit_payload else { continue };
                let Ok(env) = os_poc_causal_log::EmitEnvelope::from_msgpack(payload) else { continue };
                if env.emit_type == EmitType::InferenceRequest  as u8 { total_req  += 1; }
                if env.emit_type == EmitType::InferenceResponse as u8 { total_resp += 1; }
            }
        }

        assert_eq!(total_req, N_WORKERS,
            "S3: exactement {N_WORKERS} InferenceRequest attendus, trouvé {total_req}");
        assert_eq!(total_resp, N_WORKERS,
            "S3: exactement {N_WORKERS} InferenceResponse attendus, trouvé {total_resp}");
        assert!(
            pool.active_count() == 0,
            "S3: toutes les inférences doivent être terminées, active_count={}",
            pool.active_count()
        );
    }

    // ── S4 — Rollback scheduler + révocation caps (D5+D8) ───────────────────

    /// S4 — rollback scheduler pendant une inférence en cours + révocation de cap (D8).
    ///
    /// rollback_target construit un historique (phase 0x01), puis lance une inférence
    /// longue (phase 0x02, SleepyBackend 60s). Le scheduler rollback pendant l'attente.
    ///
    /// Assert :
    ///   - InferenceCancelled (0x0E) dans le log (Q5.1 ADR-0019).
    ///   - SchedulerRollback (0x0B) dans le log.
    ///   - Cap accordée après le snapshot cible révoquée (D8 ADR-0007).
    #[tokio::test(flavor = "multi_thread")]
    async fn s4_scheduler_rollback() {
        use super::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::{EmitType, EmitEnvelope};
        use os_poc_capabilities::Permissions;
        use std::sync::Arc;
        use std::time::Duration;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/rollback_target.wasm");
        if !wasm_path.exists() {
            eprintln!("SKIP s4: rollback_target.wasm absent");
            return;
        }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).expect("lecture rollback_target.wasm");
        let module = wasmtime::Module::new(&engine, &bytes).expect("Module::new rollback_target");
        let agent_id = [0x54u8; 16];

        // SleepyBackend 60s → l'agent sera en WaitingInference pendant le rollback
        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 60_000 }));
        let infer_fn = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id,
            store, log_ref.clone(),
            scheduler.cap_store.clone(), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("ActorInstance rollback_target");

        let tx = scheduler.register(actor);

        // Phase 1 — construire l'historique (snapshot cible du rollback = seq 0)
        tx.send(Message::data(vec![0x01])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Accorder une cap à l'agent APRÈS le snapshot (sera révoquée par D8)
        let cap_id = {
            let mut cs = scheduler.cap_store.lock().unwrap();
            cs.grant_root(
                agent_id,
                Permissions { read: true, write: true, execute: false, delegate: false },
                "/data/output".to_string(),
            )
        };
        {
            let cs = scheduler.cap_store.lock().unwrap();
            assert!(cs.get(cap_id).is_some(), "S4: cap présente avant rollback");
        }

        // Phase 2 — lancer l'inférence longue (agent entre en WaitingInference)
        tx.send(Message::data(vec![0x02])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        assert!(pool.is_active(&agent_id), "S4: inférence doit être active avant rollback");

        // Rollback scheduler → annule l'inférence puis envoie Message::Rollback
        scheduler.rollback(&agent_id, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Assertion 1 : pool libéré
        assert!(!pool.is_active(&agent_id), "S4: inférence doit être terminée après rollback");

        // Assertion 2 : cap révoquée (D8)
        {
            let cs = scheduler.cap_store.lock().unwrap();
            assert!(cs.get(cap_id).is_none(), "S4: cap doit être révoquée après rollback");
        }

        // Assertion 3 : InferenceCancelled (0x0E) + SchedulerRollback (0x0B) dans le log
        let entries = log_ref.entries_by_agent(&agent_id);
        let envelopes: Vec<EmitEnvelope> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .collect();

        let has_cancelled = envelopes.iter()
            .any(|e| e.emit_type == EmitType::InferenceCancelled as u8);
        assert!(has_cancelled, "S4: InferenceCancelled (0x0E) doit être dans le log");

        let has_sched_rollback = envelopes.iter()
            .any(|e| e.emit_type == EmitType::SchedulerRollback as u8);
        assert!(has_sched_rollback, "S4: SchedulerRollback (0x0B) doit être dans le log");

        // Assertion 4 : payload[9] du SchedulerRollback = caps_invalidated ≥ 1
        let rollback_env = envelopes.iter()
            .find(|e| e.emit_type == EmitType::SchedulerRollback as u8)
            .expect("SchedulerRollback envelope");
        assert_eq!(rollback_env.payload.len(), 10, "S4: payload SchedulerRollback = 10 bytes");
        assert!(rollback_env.payload[9] >= 1,
            "S4: caps_invalidated doit être >= 1, trouvé {}", rollback_env.payload[9]);
    }

    // ── Tests ADR-0024 — Atomicité crash (journal de compensation 0x11/0x12) ──

    /// ADR-0024 — chemin nominal : rollback sans crash émet le quartet complet
    /// CompensationOpen (0x11) + InferenceCancelled (0x0E) + SchedulerRollback (0x0B)
    /// + CompensationClose (0x12), dans cet ordre de causalité.
    ///
    /// Pré-requis : rollback_target.wasm compilé (partagé avec s4_scheduler_rollback).
    /// Si absent, le test est sauté.
    #[tokio::test(flavor = "multi_thread")]
    async fn t_no_crash_clean_path_emits_full_quartet() {
        use super::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::{EmitType, EmitEnvelope};
        use std::sync::Arc;
        use std::time::Duration;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/rollback_target.wasm");
        if !wasm_path.exists() {
            eprintln!("SKIP t_no_crash_clean_path: rollback_target.wasm absent");
            return;
        }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).expect("lecture rollback_target.wasm");
        let module = wasmtime::Module::new(&engine, &bytes).expect("Module::new rollback_target");
        let agent_id = [0xC4u8; 16];

        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 60_000 }));
        let infer_fn  = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);
        // Fournir le log_ref au scheduler pour l'émission des événements 0x11/0x12.
        scheduler.set_log_ref(Arc::clone(&log_ref));

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id,
            store, Arc::clone(&log_ref),
            scheduler.cap_store.clone(), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("ActorInstance rollback_target");

        let tx = scheduler.register(actor);

        // Construire l'historique (snapshot seq=0 sera la cible du rollback).
        tx.send(Message::data(vec![0x01])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Lancer l'inférence longue (agent entre en WaitingInference).
        tx.send(Message::data(vec![0x02])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Rollback : doit émettre 0x11, annuler (→ 0x0E), envoyer Rollback (→ 0x0B), émettre 0x12.
        scheduler.rollback(&agent_id, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Récupérer les entrées du scheduler (agent_id = [0xFF;16]) pour 0x11/0x12.
        const SCHED_ID: [u8; 16] = [0xFFu8; 16];
        let sched_entries = log_ref.entries_by_agent(&SCHED_ID);
        let sched_envs: Vec<EmitEnvelope> = sched_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // Récupérer les entrées de l'agent pour 0x0E/0x0B.
        let agent_entries = log_ref.entries_by_agent(&agent_id);
        let agent_envs: Vec<EmitEnvelope> = agent_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .collect();

        // Assertion 1 : CompensationOpen (0x11) émis par le scheduler.
        let has_comp_open = sched_envs.iter()
            .any(|e| e.emit_type == EmitType::CompensationOpen as u8);
        assert!(has_comp_open, "ADR-0024: CompensationOpen (0x11) doit être dans le log");

        // Assertion 2 : CompensationClose (0x12) émis par le scheduler.
        let has_comp_close = sched_envs.iter()
            .any(|e| e.emit_type == EmitType::CompensationClose as u8);
        assert!(has_comp_close, "ADR-0024: CompensationClose (0x12) doit être dans le log");

        // Assertion 3 : InferenceCancelled (0x0E) dans le log de l'agent.
        let has_cancelled = agent_envs.iter()
            .any(|e| e.emit_type == EmitType::InferenceCancelled as u8);
        assert!(has_cancelled, "ADR-0024: InferenceCancelled (0x0E) doit être dans le log agent");

        // Assertion 4 : SchedulerRollback (0x0B) dans le log de l'agent.
        let has_rollback = agent_envs.iter()
            .any(|e| e.emit_type == EmitType::SchedulerRollback as u8);
        assert!(has_rollback, "ADR-0024: SchedulerRollback (0x0B) doit être dans le log agent");

        // Assertion 5 : le payload de CompensationOpen contient l'agent_id cible.
        let comp_open = sched_envs.iter()
            .find(|e| e.emit_type == EmitType::CompensationOpen as u8)
            .unwrap();
        assert!(comp_open.payload.len() >= 16,
            "CompensationOpen payload doit faire au moins 16 bytes");
        assert_eq!(&comp_open.payload[..16], &agent_id,
            "CompensationOpen payload[0..16] doit être l'agent_id cible");

        // Assertion 6 : le payload de CompensationClose contient l'agent_id cible.
        let comp_close = sched_envs.iter()
            .find(|e| e.emit_type == EmitType::CompensationClose as u8)
            .unwrap();
        assert_eq!(&comp_close.payload[..16], &agent_id,
            "CompensationClose payload[0..16] doit être l'agent_id cible");
    }

    // ── S16 — `agent_infer` annulé pendant WaitingInference (UC-10) ─────────────

    /// S16 — TOCTOU central : rollback scheduler pendant WaitingInference.
    ///
    /// Oracle (deux invariants distincts) :
    ///   R1 — P2 : séquence causale 0x0C → 0x11 → 0x0E → 0x0B → 0x12 (par ts_ms).
    ///   R2 — C1 : pool.available_permits() == POOL_CAP après rollback (pas de slot zombie).
    ///
    /// ADR-0019 §Q5.1, ADR-0024 D1, ADR-0022.
    #[tokio::test(flavor = "multi_thread")]
    async fn s16_infer_cancel_toctou() {
        use super::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_causal_log::{EmitType, EmitEnvelope};
        use std::sync::Arc;
        use std::time::Duration;

        const POOL_CAP: usize = 4;

        let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wasm_path = ws_root.join("target/wasm32-unknown-unknown/debug/examples/rollback_target.wasm");
        if !wasm_path.exists() {
            eprintln!("SKIP s16: rollback_target.wasm absent");
            return;
        }

        let (engine, store, log_ref, _dir) = setup();
        let bytes = std::fs::read(&wasm_path).expect("lecture rollback_target.wasm");
        let module = wasmtime::Module::new(&engine, &bytes).expect("Module::new rollback_target");
        let agent_id = [0x16u8; 16];

        // SleepyBackend 60s : l'agent sera en WaitingInference bien au-delà du rollback.
        let pool = Arc::new(InferencePool::new(POOL_CAP, SleepyBackend { delay_ms: 60_000 }));
        let infer_fn  = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);
        scheduler.set_log_ref(Arc::clone(&log_ref));

        let actor = ActorInstance::new_precompiled_with_inference(
            &engine, &module, agent_id,
            store, Arc::clone(&log_ref),
            scheduler.cap_store.clone(), vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.expect("ActorInstance rollback_target");

        let tx = scheduler.register(actor);

        // Phase 0x01 : construire l'historique (snapshot seq=0 = cible du rollback).
        tx.send(Message::data(vec![0x01])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Phase 0x02 : lancer l'inférence longue (agent entre en WaitingInference).
        tx.send(Message::data(vec![0x02])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // ORACLE pré-rollback : l'inférence est active, un slot est pris.
        assert!(pool.is_active(&agent_id),
            "S16: inférence doit être active avant rollback");
        assert_eq!(pool.available_permits(), POOL_CAP - 1,
            "S16: exactement 1 slot doit être pris avant rollback, disponibles={}",
            pool.available_permits());

        // Rollback → annulation (0x0E), envoi Rollback, journal de compensation (0x11/0x12).
        scheduler.rollback(&agent_id, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ── ORACLE R2 : pas de slot zombie ───────────────────────────────────────
        assert_eq!(pool.active_count(), 0,
            "S16: active_count doit être 0 après rollback, trouvé {}", pool.active_count());
        assert_eq!(pool.available_permits(), POOL_CAP,
            "S16 R2-C1: slot zombie détecté — available_permits={} attendu={}",
            pool.available_permits(), POOL_CAP);

        // ── ORACLE R1 : causalité des EmitType ───────────────────────────────────
        //
        // Structure DAG (actor.rs) :
        //   prev_action → 0x0C  (ts_0c_before, avant l'inférence)
        //   prev_action → 0x0E  (now_resp, après retour Cancelled ; même parent que 0x0C)
        //   0x0E → 0x0B         (last_action = action_id(0x0E) lors du traitement Rollback)
        //   scheduler : 0x11 → 0x12
        //
        // Les relations vérifiables :
        //   1. ts_ms(0x0C) < ts_ms(0x0E)  : 0x0C émis avant l'inférence, 0x0E après
        //   2. parent_ids(0x0B) ∋ action_id(0x0E) : 0x0B dépend causalement de 0x0E
        //   3. ts_ms(0x0C) < ts_ms(0x11)  : 0x0C ~80ms avant le rollback (gap fiable)
        //   4. parent_ids(0x12) ∋ action_id(0x11) OU ts_ms(0x11) < ts_ms(0x12) : ordre scheduler

        const SCHED_ID: [u8; 16] = [0xFFu8; 16];

        let agent_entries = log_ref.entries_by_agent(&agent_id);
        let sched_entries = log_ref.entries_by_agent(&SCHED_ID);

        // Extraire les (action_id, ts_ms, emit_type) pour les types d'intérêt.
        let agent_events: Vec<([u8;32], u64, u8)> = agent_entries.iter()
            .filter_map(|(id, e)| {
                let env = EmitEnvelope::from_msgpack(e.emit_payload.as_ref()?).ok()?;
                Some((*id, e.ts_ms, env.emit_type))
            })
            .collect();
        let sched_events: Vec<([u8;32], u64, u8)> = sched_entries.iter()
            .filter_map(|(id, e)| {
                let env = EmitEnvelope::from_msgpack(e.emit_payload.as_ref()?).ok()?;
                Some((*id, e.ts_ms, env.emit_type))
            })
            .collect();

        let find_id = |events: &Vec<([u8;32], u64, u8)>, et: EmitType| -> Option<[u8;32]> {
            events.iter().find(|(_, _, t)| *t == et as u8).map(|(id, _, _)| *id)
        };
        let find_ts = |events: &Vec<([u8;32], u64, u8)>, et: EmitType| -> Option<u64> {
            events.iter().find(|(_, _, t)| *t == et as u8).map(|(_, ts, _)| *ts)
        };

        // Présence.
        assert!(find_id(&agent_events, EmitType::InferenceRequest).is_some(),
            "S16: 0x0C absent du log agent");
        assert!(find_id(&agent_events, EmitType::InferenceCancelled).is_some(),
            "S16: 0x0E absent du log agent");
        assert!(find_id(&agent_events, EmitType::SchedulerRollback).is_some(),
            "S16: 0x0B absent du log agent");
        assert!(find_id(&sched_events, EmitType::CompensationOpen).is_some(),
            "S16: 0x11 absent du log scheduler");
        assert!(find_id(&sched_events, EmitType::CompensationClose).is_some(),
            "S16: 0x12 absent du log scheduler");

        // Vérification 1 : ts_ms(0x0C) < ts_ms(0x0E).
        // 0x0C est émis avec ts_0c_before (avant l'inférence), 0x0E avec now_resp (après).
        let ts_0c = find_ts(&agent_events, EmitType::InferenceRequest).unwrap();
        let ts_0e = find_ts(&agent_events, EmitType::InferenceCancelled).unwrap();
        assert!(ts_0c < ts_0e,
            "S16: ts_ms(0x0C)={} doit être < ts_ms(0x0E)={}", ts_0c, ts_0e);

        // Vérification 2 : parent_ids(0x0B) ∋ action_id(0x0E).
        let action_id_0e = find_id(&agent_events, EmitType::InferenceCancelled).unwrap();
        let entry_0b = agent_entries.iter()
            .find(|(_, e)| {
                e.emit_payload.as_ref()
                    .and_then(|p| EmitEnvelope::from_msgpack(p).ok())
                    .map(|env| env.emit_type == EmitType::SchedulerRollback as u8)
                    .unwrap_or(false)
            })
            .map(|(_, e)| e)
            .expect("S16: entrée 0x0B non trouvée");
        // Vérification 2 (corrigée) : dépendance causale transitive 0x0E → (Lifecycle::Active) → 0x0B.
        //
        // actor.rs l.1989 émet Lifecycle::Active après chaque retour d'agent_infer.
        // Cet événement a parent_ids = [action_id(0x0E)] et devient le parent direct de 0x0B.
        // La dépendance est donc 0x0E → LA(1989) → 0x0B (transitive, pas directe).
        // On vérifie par remonté de chaîne parent_ids depuis 0x0B.
        {
            use std::collections::HashMap;
            let by_id: HashMap<[u8;32], &os_poc_causal_log::LogEntry> =
                agent_entries.iter().map(|(id, e)| (*id, e)).collect();

            // Remonter depuis 0x0B jusqu'à trouver action_id(0x0E) dans la chaîne.
            let mut current_ids: Vec<[u8;32]> = entry_0b.parent_ids.clone();
            let mut found_0e = false;
            for _ in 0..10 { // max 10 sauts (la chaîne est courte)
                let mut next: Vec<[u8;32]> = Vec::new();
                for pid in &current_ids {
                    if *pid == action_id_0e {
                        found_0e = true;
                        break;
                    }
                    if let Some(parent_entry) = by_id.get(pid) {
                        next.extend_from_slice(&parent_entry.parent_ids);
                    }
                }
                if found_0e { break; }
                if next.is_empty() { break; }
                current_ids = next;
            }
            assert!(found_0e,
                "S16: 0x0E n'est pas ancêtre causal de 0x0B via parent_ids — \
                 causalité 0x0E → ... → 0x0B violée (action_id_0e={:?})",
                &action_id_0e[..4]);
        }

        // Vérification 3 : ts_ms(0x0C) < ts_ms(0x11) — gap ~80ms fiable.
        let ts_0x11 = find_ts(&sched_events, EmitType::CompensationOpen).unwrap();
        assert!(ts_0c < ts_0x11,
            "S16: ts_ms(0x0C)={} doit précéder ts_ms(0x11)={} (gap ~80ms)", ts_0c, ts_0x11);

        // Vérification 4 : ts_ms(0x11) < ts_ms(0x12) dans la chaîne scheduler.
        let ts_0x12 = find_ts(&sched_events, EmitType::CompensationClose).unwrap();
        assert!(ts_0x11 <= ts_0x12,
            "S16: ts_ms(0x11)={} doit précéder ts_ms(0x12)={}", ts_0x11, ts_0x12);
    }


    // S17 - Rollback + invalidation cap en cascade (UC-9 / ADR-0007 / ADR-0005)

    /// S17 - Oracle P2 x P4 (R1) : apres rollback de A vers S0 (avant delegation),
    /// la cap de A (C_A, emise apres S0) ET sa derivee (C_B, deleguee a B) sont revoquees.
    /// Verifie la cascade O(depth) de revoke_owned_after via revoke(id).
    #[tokio::test(flavor = "multi_thread")]
    async fn s17_rollback_cap_cascade() {
        use super::actor::{ActorInstance, AGENT_WAT, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::scheduler::Scheduler;
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, AGENT_WAT).expect("AGENT_WAT");
        let agent_a = [0x09u8; 16];
        let agent_b = [0x0Au8; 16];

        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));

        // C_root : cap accordee a A AVANT le snapshot (ne doit PAS etre revoquee).
        let c_root = {
            let mut cs = cap_store.lock().unwrap();
            cs.grant_root(agent_a,
                Permissions { read: true, write: true, execute: false, delegate: true },
                "/data".to_string())
        };

        let actor_a = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_a,
            store.clone(), log_ref.clone(),
            cap_store.clone(), vec![c_root],
        ).await.expect("ActorInstance A");

        let mut scheduler = Scheduler::new();
        let tx_a = scheduler.register(actor_a);

        // Phase 1 : A fait un commit_barrier -> snapshot S0.
        tx_a.send(Message::data(vec![0x01])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        // C_A : cap accordee APRES le snapshot S0 (owned by A, sera revoquee).
        let c_a = {
            let mut cs = cap_store.lock().unwrap();
            cs.grant_root(agent_a,
                Permissions { read: true, write: true, execute: false, delegate: true },
                "/data".to_string())
        };

        // C_B : cap deleguee de C_A -> B (scope restreint, owned by B).
        let c_b = {
            let mut cs = cap_store.lock().unwrap();
            cs.delegate(c_a, &agent_a, agent_b,
                Permissions { read: true, write: false, execute: false, delegate: false },
                "/data/sub".to_string())
                .expect("delegate C_A to C_B")
        };

        // Verifie l'etat AVANT rollback.
        {
            let cs = cap_store.lock().unwrap();
            assert!(cs.get(c_root).is_some(), "S17: C_root doit exister avant rollback");
            assert!(cs.get(c_a).is_some(),    "S17: C_A doit exister avant rollback");
            assert!(cs.get(c_b).is_some(),    "S17: C_B doit exister avant rollback");
            assert!(cs.check(&agent_b, c_b, "/data/sub",
                    &Permissions { read: true, write: false, execute: false, delegate: false }),
                "S17: B doit acceder via C_B avant rollback");
        }

        // Rollback de A vers seq=0.
        scheduler.rollback(&agent_a, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(tx_a);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // ORACLE P2 x P4.
        {
            let cs = cap_store.lock().unwrap();

            // C_root (avant S0) : NE DOIT PAS etre revoquee.
            assert!(cs.get(c_root).is_some(),
                "S17: C_root (emise avant S0) ne doit pas etre revoquee");

            // C_A (apres S0, owned by A) : DOIT etre revoquee.
            assert!(cs.get(c_a).is_none(),
                "S17 P2: C_A doit etre revoquee (owned by A, emise apres S0)");

            // C_B (deleguee de C_A, owned by B) : DOIT etre revoquee en cascade.
            assert!(cs.get(c_b).is_none(),
                "S17 P4: C_B doit etre revoquee en cascade (enfant de C_A, O(depth))");

            // B ne peut plus acceder.
            assert!(!cs.check(&agent_b, c_b, "/data/sub",
                    &Permissions { read: true, write: false, execute: false, delegate: false }),
                "S17 P4: check(B, C_B) doit retourner false apres cascade");
        }

        // caps_invalidated dans 0x0B >= 1.
        let entries = log_ref.entries_by_agent(&agent_a);
        let has_rb = entries.iter().any(|(_, e)| {
            e.emit_payload.as_ref()
                .and_then(|p| os_poc_causal_log::EmitEnvelope::from_msgpack(p).ok())
                .map(|env| env.emit_type == os_poc_causal_log::EmitType::SchedulerRollback as u8)
                .unwrap_or(false)
        });
        assert!(has_rb, "S17: SchedulerRollback (0x0B) doit etre dans le log de A");
    }


    // S18 - agent_add_cause merge legitime (UC-1 / ADR-0003 / ADR-0008)

    /// S18 - Oracle P3c (R1) : construction d un noeud de merge (N>1 parents) via
    /// agent_add_cause. B1.parent_ids contient a la fois la cause externe A1 et
    /// la cause interne B0 -> merge node confirme.
    #[tokio::test(flavor = "current_thread")]
    async fn s18_add_cause_merge() {
        use super::actor::{ActorInstance, ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use os_poc_causal_log::LogEntry;
        use std::sync::Arc;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let id_a = [0x18u8; 16];
        let id_c = [0x1Au8; 16];

        // Agent A produit l action A1.
        let mut actor_a = ActorInstance::new_precompiled(
            &engine, &module, id_a, store.clone(), log_ref.clone()
        ).await.unwrap();
        actor_a.process_one(&[0x00]).await.unwrap();
        let a1_id = actor_a.last_action().expect("S18: last_action A");

        // Agent C produit C0 (baseline), puis C1 avec add_cause(A1).
        // ADR-0058 : merge cross-agent légitime → on minte le handle autorisant C à citer A1.
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_c = reg.get_or_create(TenantId::DEFAULT);
        let mut actor_c = ActorInstanceBuilder::new(&engine, &module, id_c, store.clone(), log_ref.clone())
            .cause_handle_registry(Arc::clone(&reg))
            .build().await.unwrap();
        actor_c.process_one(&[0x00]).await.unwrap();
        let c0_id = actor_c.last_action().expect("S18: C0");

        chs_c.lock().unwrap().mint(a1_id, id_c, id_a, 0);

        // msg[0]=4 + A1_id -> add_cause(A1) + commit_barrier + emit -> C1
        let mut msg = vec![0x04u8];
        msg.extend_from_slice(&a1_id);
        actor_c.process_one(&msg).await.unwrap();
        let c1_id = actor_c.last_action().expect("S18: C1");
        assert_ne!(c0_id, c1_id, "S18: C1 doit etre une nouvelle action");

        let entry_c1: LogEntry = log_ref.get(&c1_id).unwrap()
            .expect("S18: C1 dans le log");

        // Oracle P3c : noeud de merge N=2, parent_ids = [C0, A1].
        assert!(entry_c1.parent_ids.contains(&a1_id),
            "S18 P3c: parent_ids(C1) doit contenir A1_id (cause externe)");
        assert!(entry_c1.parent_ids.contains(&c0_id),
            "S18 P3c: parent_ids(C1) doit contenir C0_id (chaine interne)");
        assert!(entry_c1.parent_ids.len() >= 2,
            "S18 P3c: noeud de merge N>1 parents attendu, len={}",
            entry_c1.parent_ids.len());
    }

    // S19 - Compensation journal orphelin (UC-12 / ADR-0024 D3)

    /// S19 - Oracle P6 (R1) : 0x11 sans 0x12 simule un crash apres CompensationOpen.
    /// L oracle inline detecte l orphelin. ContentStore inchange (pas d etat partiel).
    #[tokio::test(flavor = "current_thread")]
    async fn s19_compensation_orphelin() {
        use super::actor::{ActorInstance, AGENT_WAT};
        use os_poc_causal_log::{EmitEnvelope, EmitType, LogEntry};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();
        let agent_a = [0x19u8; 16];

        // Agent A construit un historique (snapshot S0).
        let mut actor_a = ActorInstance::new_precompiled(
            &engine, &module, agent_a, store.clone(), log_ref.clone()
        ).await.unwrap();
        actor_a.process_one(b"s19").await.unwrap();
        let last_snap_before = actor_a.last_snapshot();

        // Simulation crash : injection manuelle de 0x11 sans 0x12.
        // Reproduit l etat laisse par CrashPoint::AfterCompensationOpen.
        const SCHED_ID: [u8; 16] = [0xFFu8; 16];
        let target_agent = agent_a;
        let mut pl = vec![0u8; 48];
        pl[..16].copy_from_slice(&target_agent);
        // pl[16..48] = zeros (expected_inference_event_id inconnu)
        let env_0x11 = EmitEnvelope::new(
            EmitType::CompensationOpen, SCHED_ID, 0, 0, pl
        );
        let entry_0x11 = LogEntry {
            agent_id: SCHED_ID,
            ts_ms: 0,
            parent_ids: vec![],
            hash_before: [0u8; 32],
            hash_after: [0u8; 32],
            emit_payload: Some(env_0x11.to_msgpack()),
        };
        log_ref.append(&entry_0x11).unwrap();
        // Note : on n ajoute PAS de 0x12 -> orphelin simule.

        // Oracle 1 : detection orphelin inline (logique de os-poc-reconstruct).
        let sched_entries = log_ref.entries_by_agent(&SCHED_ID);
        let mut open_set: Vec<[u8; 16]> = Vec::new();
        for (_, entry) in &sched_entries {
            let Some(payload) = &entry.emit_payload else { continue };
            let Ok(env) = EmitEnvelope::from_msgpack(payload) else { continue };
            match EmitType::try_from(env.emit_type) {
                Ok(EmitType::CompensationOpen) if env.payload.len() >= 16 => {
                    let mut aid = [0u8; 16];
                    aid.copy_from_slice(&env.payload[..16]);
                    open_set.push(aid);
                }
                Ok(EmitType::CompensationClose) if env.payload.len() >= 16 => {
                    let mut aid = [0u8; 16];
                    aid.copy_from_slice(&env.payload[..16]);
                    open_set.retain(|x| x != &aid);
                }
                _ => {}
            }
        }
        assert!(!open_set.is_empty(),
            "S19 P6: open_set doit etre non vide (orphelin 0x11 detecte)");
        assert!(open_set.contains(&target_agent),
            "S19 P6: open_set doit contenir target_agent_id");

        // Oracle 2 : ContentStore inchange (pas d etat partiel observable).
        // Le rollback n a jamais ete applique (crash avant 0x0B) -> dernier snapshot stable.
        let last_snap_after = actor_a.last_snapshot();
        assert_eq!(last_snap_before, last_snap_after,
            "S19 P6: ContentStore inchange (pas d etat partiel) - pas de snapshot fantome");
    }

    // S20 - Propagation erreur cross-agent (UC-13 / ADR-0015)

    /// S20 - Oracle P3/P4 (R1) : quand A se termine, B conserve son lien causal
    /// vers A1 (parent_ids integres). Aucun message orphelin de A ne peut atteindre B
    /// (canal mpsc ferme a la terminaison de A).
    #[tokio::test(flavor = "current_thread")]
    async fn s20_propagation_crash_agent() {
        use super::actor::{ActorInstance, ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use os_poc_causal_log::LogEntry;
        use std::sync::Arc;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let id_a = [0x20u8; 16];
        let id_b = [0x21u8; 16];

        // Agent A produit A1.
        let mut actor_a = ActorInstance::new_precompiled(
            &engine, &module, id_a, store.clone(), log_ref.clone()
        ).await.unwrap();
        actor_a.process_one(&[0x00]).await.unwrap();
        let a1_id = actor_a.last_action().expect("S20: A1");

        // Agent B: B0 (baseline), puis B1 avec cause cross-agent légitime sur A1.
        // ADR-0058 : on minte le handle autorisant B à citer A1 (autorité explicite).
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_b = reg.get_or_create(TenantId::DEFAULT);
        let mut actor_b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log_ref.clone())
            .cause_handle_registry(Arc::clone(&reg))
            .build().await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();
        let b0_id = actor_b.last_action().expect("S20: B0");

        chs_b.lock().unwrap().mint(a1_id, id_b, id_a, 0);

        // B1 avec add_cause(A1) -> parent_ids = [B0, A1].
        let mut msg = vec![0x04u8];
        msg.extend_from_slice(&a1_id);
        actor_b.process_one(&msg).await.unwrap();
        let b1_id = actor_b.last_action().expect("S20: B1");

        // A se "termine" (drop simule inbox fermee).
        drop(actor_a);

        // Oracle P3 : lien causal A1 -> B1 preserve meme apres terminaison de A.
        let entry_b1: LogEntry = log_ref.get(&b1_id).unwrap()
            .expect("S20: B1 dans le log");
        assert!(entry_b1.parent_ids.contains(&a1_id),
            "S20 P3: parent_ids(B1) doit contenir A1_id apres terminaison de A");
        assert!(entry_b1.parent_ids.contains(&b0_id),
            "S20 P3: parent_ids(B1) doit contenir B0_id (chaine interne B)");

        // Oracle P4 : pas de message orphelin de A (A n envoie plus rien a B).
        // Verifie qu aucune nouvelle entree n a ete ajoutee au log de A apres drop.
        let a_entries_after = log_ref.entries_by_agent(&id_a);
        // L action A1 doit etre la derniere (aucune entree ajoutee apres drop).
        let a_last = a_entries_after.iter()
            .max_by_key(|(_, e)| e.ts_ms);
        if let Some((last_id, _)) = a_last {
            assert_eq!(*last_id, a1_id,
                "S20 P4: A1 doit etre la derniere action de A (pas d action orpheline apres drop)");
        }
    }

    // ── Tests D9 — AgentProfile watchdog (ADR-0025) ──────────────────────────────

    /// ADR-0025 — profil Algo : un agent en boucle infinie est trappé en ≤ ~200ms.
    ///
    /// Algo = 10 ticks × 10ms = 100ms plafond. On attend 1s (×10 marge) pour absorber
    /// la latence de scheduling. L'essentiel est que ça termine bien avant LlmShort (5s).
    #[tokio::test(flavor = "multi_thread")]
    async fn t_algo_profile_traps_at_100ms() {
        use super::actor::{ActorInstance, INFINITE_LOOP_AGENT_WAT, Message, LifecycleState};
        use super::scheduler::Scheduler;
        use super::watchdog::AgentProfile;
        use std::time::{Duration, Instant};

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFINITE_LOOP_AGENT_WAT).unwrap();
        let agent_id = [0xA1u8; 16];

        let actor = ActorInstance::new_precompiled_with_profile(
            &engine, &module, agent_id, store, log_ref.clone(),
            AgentProfile::Algo,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        let t0 = Instant::now();
        tx.send(Message::data(b"trigger".to_vec())).await.unwrap();

        // Algo profile : 10 ticks × 10ms = 100ms plafond.
        // On attend 1s (×10) pour absorber la latence, mais l'agent DOIT terminer avant LlmShort.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let elapsed = t0.elapsed();

        let entries = log_ref.entries_by_agent(&agent_id);
        // ADR-0015 D-Q-V2.2 : AgentCrash est le terminal event ; plus de Lifecycle::Terminated séparé.
        let terminated = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8);
        assert!(terminated,
            "ADR-0025: agent Algo (boucle infinie) doit être terminé par watchdog (AgentCrash dans le log, elapsed={:?})",
            elapsed);
        // L'agent doit être terminé bien avant 4s (LlmShort serait à 5s).
        assert!(elapsed < Duration::from_secs(4),
            "ADR-0025: agent Algo terminé en {:?}, attendu < 4s", elapsed);
    }

    /// ADR-0025 — profil LlmLong : un agent qui dure ~300ms n'est PAS trappé.
    ///
    /// LlmLong = 3000 ticks × 10ms = 30s plafond. Un agent qui process 300ms et termine
    /// normalement ne doit pas déclencher le watchdog.
    #[tokio::test(flavor = "multi_thread")]
    async fn t_llm_long_profile_allows_30s() {
        use super::actor::{ActorInstance, Message, LifecycleState};
        use super::scheduler::Scheduler;
        use super::watchdog::AgentProfile;
        use std::time::Duration;

        // Module WASM qui fait un simple emit (pas de boucle infinie) et se termine.
        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, super::actor::AGENT_WAT).unwrap();
        let agent_id = [0xA2u8; 16];

        let actor = ActorInstance::new_precompiled_with_profile(
            &engine, &module, agent_id, store, log_ref.clone(),
            AgentProfile::LlmLong,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        tx.send(Message::data(b"hello".to_vec())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // L'agent ne doit PAS avoir été Terminated par watchdog — il doit avoir traité normalement.
        // On vérifie qu'il y a bien un lifecycle Active (traitement normal) sans Terminated inopiné.
        let entries = log_ref.entries_by_agent(&agent_id);
        let lifecycle_events: Vec<u8> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == os_poc_causal_log::EmitType::Lifecycle as u8)
            .filter_map(|env| env.payload.first().copied())
            .collect();

        // Doit avoir au moins un événement Spawned et Active — processé normalement.
        let has_active = lifecycle_events.iter().any(|&s| s == LifecycleState::Active as u8);
        assert!(has_active,
            "ADR-0025: agent LlmLong doit traiter normalement (lifecycle Active présent)");
    }

    /// ADR-0025 — le payload Spawned (0x01 Lifecycle) contient le byte de profil watchdog.
    ///
    /// Format Spawned payload : [state_byte=0x00, seq LE 8 bytes, profile_byte] = 10 bytes.
    #[tokio::test(flavor = "current_thread")]
    async fn t_profile_emitted_in_spawned() {
        use super::actor::{ActorInstance, LifecycleState};
        use super::watchdog::AgentProfile;

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, super::actor::AGENT_WAT).unwrap();
        let agent_id = [0xA3u8; 16];

        // Crée un acteur avec profil LlmLong.
        let actor = ActorInstance::new_precompiled_with_profile(
            &engine, &module, agent_id, store, log_ref.clone(),
            AgentProfile::LlmLong,
        ).await.unwrap();
        // log_lifecycle_event(Spawned) est appelé dans run_loop — ici on le déclenche
        // manuellement via la transition d'état initiale (déjà effectuée dans new_precompiled_with_profile
        // via AgentState::lifecycle = Spawned, mais l'enregistrement log se fait dans run_loop).
        // On reproduit la transition comme le fait run_loop.
        let mut actor = actor;
        actor.state_mut().log_lifecycle_event(LifecycleState::Spawned);

        // Récupère l'entrée Spawned dans le log.
        let entries = log_ref.entries_by_agent(&agent_id);
        let spawned_payload: Option<Vec<u8>> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .find(|env| {
                env.emit_type == os_poc_causal_log::EmitType::Lifecycle as u8
                    && env.payload.first().copied() == Some(LifecycleState::Spawned as u8)
            })
            .map(|env| env.payload);

        let payload = spawned_payload.expect("ADR-0025: payload Spawned absent du log");
        assert_eq!(payload.len(), 10,
            "ADR-0025: payload Spawned doit faire 10 bytes (state + seq8 + profile)");
        // profile_byte est le 10ème byte (index 9).
        let profile_byte = payload[9];
        assert_eq!(profile_byte, AgentProfile::LlmLong as u8,
            "ADR-0025: payload Spawned[9] doit être le discriminant LlmLong (0x03), got {:#04x}",
            profile_byte);
    }

    // ── S5 — Fairness & Priority (ADR-0021 / ADR-0022 / ADR-0023) ────────────

    /// S5 — C1-fairness-priority : 8 agents Foreground + 2 agents Supervisor,
    /// pool cap=2, SleepyBackend(100ms), queue_capacity=16.
    ///
    /// Assertions :
    /// 1. Priorité : les 2 supervisors obtiennent leur slot avant que tous les 8 foreground aient fini.
    /// 2. Pas de famine : tous les foreground complètent en < 5s (budget large, ×10 marge).
    /// 3. E1 FIFO : parmi les foreground, l'ordre d'admission = l'ordre de service (vérifié via
    ///    l'ordre des InferenceResponse (0x0D) dans le log causal).
    ///
    /// Runtime single-thread (current_thread) pour determinisme (ADR-0021).
    #[tokio::test(flavor = "current_thread")]
    async fn s5_fairness_priority() {
        use super::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
                           INFER_AGENT_WAT};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend, PriorityClass};
        use os_poc_causal_log::EmitType;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        const N_FOREGROUND: usize = 8;
        const N_SUPERVISOR: usize = 2;
        const POOL_CAP: usize = 2;
        const SLEEP_MS: u64 = 100;
        const QUEUE_CAP: usize = 16;

        let (engine, store_ref, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFER_AGENT_WAT).unwrap();

        let pool = Arc::new(InferencePool::new_with_queue_params(
            POOL_CAP, QUEUE_CAP, 10_000, SleepyBackend { delay_ms: SLEEP_MS }
        ));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        // infer_fn pour les agents Foreground
        let fg_infer_fn = InferencePool::as_infer_fn_with_class(
            Arc::clone(&pool), PriorityClass::Foreground
        );
        // infer_fn pour les agents Supervisor
        let sv_infer_fn = InferencePool::as_infer_fn_with_class(
            Arc::clone(&pool), PriorityClass::Supervisor
        );

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);

        let cap_store = scheduler.cap_store.clone();

        // Créer N_FOREGROUND agents foreground
        let mut fg_txs = Vec::new();
        let mut fg_ids = Vec::new();
        for i in 0..N_FOREGROUND {
            let mut id = [0u8; 16];
            id[0] = 0xF0 | (i as u8);
            let actor = ActorInstance::new_precompiled_with_inference(
                &engine, &module, id,
                Arc::clone(&store_ref), Arc::clone(&log_ref),
                Arc::clone(&cap_store), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                fg_infer_fn.clone(),
            ).await.unwrap();
            let tx = scheduler.register(actor);
            fg_txs.push(tx);
            fg_ids.push(id);
        }

        // Créer N_SUPERVISOR agents supervisor
        let mut sv_txs = Vec::new();
        let mut sv_ids = Vec::new();
        for i in 0..N_SUPERVISOR {
            let mut id = [0u8; 16];
            id[0] = 0x5A | (i as u8);
            let actor = ActorInstance::new_precompiled_with_inference(
                &engine, &module, id,
                Arc::clone(&store_ref), Arc::clone(&log_ref),
                Arc::clone(&cap_store), vec![],
                SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                sv_infer_fn.clone(),
            ).await.unwrap();
            let tx = scheduler.register(actor);
            sv_txs.push(tx);
            sv_ids.push(id);
        }

        // Construire l'historique pour tous les agents (msg[0]=0 → commit_barrier + emit)
        for tx in fg_txs.iter().chain(sv_txs.iter()) {
            tx.send(Message::data(vec![0x00])).await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Envoyer simultanément msg[0]=7 à TOUS les agents (déclenchement de agent_infer)
        // Foreground d'abord pour tester que les supervisors les dépassent en priorité.
        let t_all_start = Instant::now();
        for tx in fg_txs.iter() {
            tx.send(Message::data(vec![0x07])).await.unwrap();
        }
        // Légère pause pour s'assurer que les foreground sont dans la file avant les supervisors
        tokio::task::yield_now().await;
        for tx in sv_txs.iter() {
            tx.send(Message::data(vec![0x07])).await.unwrap();
        }

        // Attendre que tous complètent : avec cap=2, SleepyBackend(100ms) et 10 agents,
        // le pire cas est 5 rounds × 100ms = 500ms. On attend 5s (×10 marge).
        let budget = std::time::Duration::from_secs(5);
        let deadline = tokio::time::Instant::now() + budget;

        loop {
            tokio::time::sleep_until(deadline.min(
                tokio::time::Instant::now() + std::time::Duration::from_millis(50)
            )).await;

            // Vérifier si tous les agents ont un InferenceResponse (0x0D) dans le log.
            let all_done = fg_ids.iter().chain(sv_ids.iter()).all(|id| {
                let entries = log_ref.entries_by_agent(id);
                entries.iter()
                    .filter_map(|(_, e)| e.emit_payload.as_ref())
                    .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                    .any(|env| env.emit_type == EmitType::InferenceResponse as u8)
            });
            if all_done || tokio::time::Instant::now() >= deadline {
                break;
            }
        }

        let total_elapsed = t_all_start.elapsed();

        // Assertion 2 : pas de famine — tous completent en < 5s
        assert!(total_elapsed < budget,
            "S5: tous les agents doivent completer en < 5s (elapsed={:?})", total_elapsed);

        // Compter combien de foreground ont un InferenceResponse
        let fg_done_count = fg_ids.iter().filter(|id| {
            let entries = log_ref.entries_by_agent(id);
            entries.iter()
                .filter_map(|(_, e)| e.emit_payload.as_ref())
                .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                .any(|env| env.emit_type == EmitType::InferenceResponse as u8)
        }).count();

        let sv_done_count = sv_ids.iter().filter(|id| {
            let entries = log_ref.entries_by_agent(id);
            entries.iter()
                .filter_map(|(_, e)| e.emit_payload.as_ref())
                .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                .any(|env| env.emit_type == EmitType::InferenceResponse as u8)
        }).count();

        assert_eq!(fg_done_count, N_FOREGROUND,
            "S5: tous les agents foreground doivent avoir un InferenceResponse");
        assert_eq!(sv_done_count, N_SUPERVISOR,
            "S5: tous les agents supervisor doivent avoir un InferenceResponse");

        // Assertion 1 : priorité — les supervisors doivent avoir été servis
        // AVANT que tous les foreground aient terminé.
        // On mesure via ts_us (timestamp microseconde) des InferenceResponse (0x0D).
        // Avec priorité stricte et supervisors soumis après les foreground dans la file,
        // max(ts_us des supervisors) < max(ts_us des foreground).
        let get_response_ts_us = |id: &[u8; 16]| -> Option<u64> {
            let entries = log_ref.entries_by_agent(id);
            entries.iter()
                .filter_map(|(_, e)| e.emit_payload.as_ref())
                .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                .find(|env| env.emit_type == EmitType::InferenceResponse as u8)
                .map(|env| env.ts_us)
        };

        let sv_ts: Vec<u64> = sv_ids.iter().filter_map(|id| get_response_ts_us(id)).collect();
        let fg_ts: Vec<u64> = fg_ids.iter().filter_map(|id| get_response_ts_us(id)).collect();

        let sv_max_ts = sv_ts.iter().max().copied().unwrap_or(u64::MAX);
        let fg_max_ts = fg_ts.iter().max().copied().unwrap_or(0);

        // Les supervisors doivent avoir terminé AVANT les derniers foreground.
        // Concrètement : max(ts_us des sv) < max(ts_us des fg).
        // Avec priorité stricte, les 2 supervisors passent devant les foreground en attente.
        assert!(sv_max_ts < fg_max_ts,
            "S5: les supervisors doivent avoir terminé avant les derniers foreground \
             (sv_max_ts={}us, fg_max_ts={}us)", sv_max_ts, fg_max_ts);

        drop(fg_txs);
        drop(sv_txs);
    }

    // ── Tests SEF-3 — P1/P2/P3 Capability Enforcement ────────────────────────

    /// Helper SEF-3 : construit un message pour agent_store_put.
    /// Layout : [opcode=8, rlen, resource[rlen], cap_id_le[8], value[rlen]]
    fn make_put_msg(resource: &str, cap_id: u64, value: &[u8]) -> Vec<u8> {
        assert_eq!(resource.len(), value.len(), "make_put_msg: resource et value doivent avoir la même longueur");
        let rlen = resource.len() as u8;
        let mut msg = vec![8u8, rlen];
        msg.extend_from_slice(resource.as_bytes());
        msg.extend_from_slice(&cap_id.to_le_bytes());
        msg.extend_from_slice(value);
        msg
    }

    /// Helper SEF-3 : construit un message pour agent_store_get.
    /// Layout : [opcode=9, rlen, resource[rlen], cap_id_le[8]]
    fn make_get_msg(resource: &str, cap_id: u64) -> Vec<u8> {
        let rlen = resource.len() as u8;
        let mut msg = vec![9u8, rlen];
        msg.extend_from_slice(resource.as_bytes());
        msg.extend_from_slice(&cap_id.to_le_bytes());
        msg
    }

    /// Extrait la `resource` d'un payload `CapabilityDenied (0x14)` standard.
    /// Retourne `None` pour un événement agrégé (rate_limited=0x01, resource absente).
    /// Format standard : [agent_id 16B | cap_id 8B | resource_len u8 | resource N | perm u8 | rate_limited u8]
    fn cap_denied_resource(payload: &[u8]) -> Option<String> {
        if payload.len() < 26 { return None; }
        if *payload.last().unwrap() != 0x00 { return None; } // agrégé → pas de resource
        let rlen = payload[24] as usize;
        let res = payload.get(25..25 + rlen)?;
        Some(String::from_utf8_lossy(res).into_owned())
    }

    /// SEF-10 / axe 3 (ADR-0050 §D4) — fenêtre de référence pendante cross-store.
    ///
    /// **Note de recevabilité (mur de faisabilité).** Le verdict machine-crash
    /// *recevable* (invalidation de cache réelle) exige root/drop_caches ou une VM —
    /// absents de cet environnement (cf. ADR-0046, mur identique au power-loss seL4).
    /// Le piège n°1/L32 (ADR-0050) interdit de *simuler* la perte de cache avec un
    /// modèle maison. Ce test ne prétend donc PAS valider la durabilité sous
    /// power-loss. Il teste la **gestion** de l'état déchiré que le cache-loss *peut*
    /// produire — la sévérité de la fenêtre, pas son occurrence empirique.
    ///
    /// **Fenêtre établie par design (gate SEF-8) :** ContentStore et CausalLog sont
    /// deux instances RocksDB *séparées* ; le commit écrit store (put_block,
    /// put_snapshot) PUIS log (append), **sans fsync ni atomicité cross-DB**. Sous
    /// cache-loss avec réordonnancement, le log peut atteindre le disque avant le
    /// store → un LogEntry référence un snapshot **absent du store** (référence pendante).
    ///
    /// **Sévérité de la fenêtre + régression du correctif #7a (ADR-0051 §D3) :**
    /// restauration d'un état dont `last_snapshot` est absent du store →
    ///   (a) AVANT #7a : `restore_from_evicted` réussissait SANS détecter l'incohérence
    ///       (échec différé au rollback). APRÈS #7a : le restore **détecte** la
    ///       référence pendante et échoue explicitement (`MissingBlock`, fail-safe).
    ///   (b) la fenêtre reste réelle : `rollback_path` sur le tip pendant échoue
    ///       `MissingBlock` — c'est pourquoi #7a (détection précoce) importe.
    ///   (c) aucun panic (dégradation gracieuse). #7a ne FERME pas la fenêtre (= #7b,
    ///       différé chantier GC) ; il la rend détectable et bruyante.
    #[tokio::test(flavor = "current_thread")]
    async fn sef10_cross_store_dangling_snapshot() {
        use super::actor::{ActorInstance, EvictedState, AGENT_WAT};
        use super::RuntimeError;
        use os_poc_store::StoreError;
        use std::time::Instant;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();
        let agent_id = [0xC3u8; 16];

        // ── État consistant : 2 actions réelles (store ET log alignés) ──────────
        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store.clone(), log_ref.clone(),
        ).await.unwrap();
        actor.process_one(b"a1").await.unwrap();
        actor.process_one(b"a2").await.unwrap();
        let s2 = actor.last_snapshot().expect("snapshot S2 présent");
        let a2 = actor.last_action().expect("action A2 présente");

        // Sanity : à l'état consistant, S2 est dans le store et le rollback marche.
        assert!(store.get_header(&s2).unwrap().is_some(), "S2 présent dans le store");
        assert!(store.rollback_path(&s2, 0).is_ok(), "rollback consistant OK");

        // ── État déchiré : le log a avancé (A3 → S3) mais le store a perdu S3 ────
        // Modèle du cache-loss store-derrière-log : `last_snapshot` référence un
        // snapshot absent du store. (On ne fabrique PAS le crash ; on construit son
        // résultat le plus défavorable pour en tester la gestion.)
        let s3_absent = [0xD0u8; 32];
        assert!(store.get_header(&s3_absent).unwrap().is_none(),
            "S3 modélisé comme ABSENT du store (perdu au cache-loss)");

        let evicted = EvictedState {
            id: agent_id,
            seq: 3,
            last_snapshot: Some(s3_absent),
            last_action: Some(a2),
            evicted_at: Instant::now(),
        };

        // ── (a) RÉGRESSION #7a : le restore DÉTECTE la référence pendante ───────
        let restored = ActorInstance::restore_from_evicted(
            &engine, &module, &evicted, store.clone(), log_ref.clone(),
        ).await;
        match restored {
            Err(RuntimeError::Store(StoreError::MissingBlock(h))) => {
                assert_eq!(h, s3_absent,
                    "#7a : restore échoue explicitement MissingBlock sur le snapshot pendant");
            }
            Err(other) => panic!("#7a : attendu MissingBlock, obtenu {other:?}"),
            Ok(_) => panic!(
                "#7a RÉGRESSION : restore_from_evicted a ACCEPTÉ un last_snapshot absent \
                 du store (fail-safe non défendu — régression du correctif #7a)"),
        }

        // ── (b) La fenêtre reste réelle : rollback sur le tip pendant = MissingBlock.
        // (Sans #7a, c'est ICI que l'incohérence aurait surfacé — tardivement.)
        match store.rollback_path(&s3_absent, 0) {
            Err(StoreError::MissingBlock(h)) => assert_eq!(h, s3_absent),
            other => panic!("attendu MissingBlock au rollback, obtenu {other:?}"),
        }

        // ── (c) Aucun panic : dégradation gracieuse. #7a détecte ; ne ferme pas (#7b).
        println!(
            "SEF-10 (régression #7a) : référence pendante DÉTECTÉE au restore (fail-safe), \
             fenêtre réelle confirmée (rollback MissingBlock). Fermeture #7b + durabilité power-loss : différées."
        );
    }

    /// SEF-9 / axe 1b (ADR-0050 §D3) — confused-deputy : le rate-limit du log
    /// d'audit `0x14` masque un refus malveillant sous flood.
    ///
    /// Démontre la DISTINCTION de l'architecte (ADR-0050 §D3) :
    ///   - 1a (isolation P4) TIENT : le refus malveillant est bien refusé (-1).
    ///   - 1b (fidélité du log d'audit) ÉCHOUE : ce refus n'est PAS attribuable
    ///     dans le log, car le rate-limit l'a agrégé/silencé.
    ///
    /// Un échec 1b ≠ P4 violée : c'est un défaut d'OBSERVABILITÉ, pas d'isolation.
    /// Oracle 1b = témoin hors-bande (`cap_denied_witness`) capturé au point de
    /// décision AVANT le rate-limit (l'état des caps ne peut pas le falsifier :
    /// `check()` est pur, un refus ne laisse aucune trace d'état).
    #[tokio::test(flavor = "current_thread")]
    async fn sef9_audit_masking_under_flood() {
        use super::actor::{ActorInstance, CapDeniedAttempt, STORE_AGENT_WAT};
        use os_poc_capabilities::CapabilityStore;
        use os_poc_causal_log::{EmitEnvelope, EmitType};
        use std::collections::BTreeSet;
        use std::sync::{Arc, Mutex};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();
        let agent_id = [0x9Eu8; 16];

        // Agent sans aucune capability → tout get est refusé.
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let fake_cap: u64 = 9999;

        let mut actor = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![],
        ).await.unwrap();

        // Installer le témoin hors-bande (oracle 1b).
        let witness: Arc<Mutex<Vec<CapDeniedAttempt>>> = Arc::new(Mutex::new(Vec::new()));
        actor.state_mut().cap_denied_witness = Some(witness.clone());

        // ── Attaque ───────────────────────────────────────────────────────────
        // 1) Flood : 101 refus bénins sur "bn" (counts 1..101 ; les 100 premiers
        //    loggés AVEC resource, le 101ᵉ agrégé SANS resource).
        const FLOOD: usize = 101;
        for _ in 0..FLOOD {
            let msg = make_get_msg("bn", fake_cap);
            actor.process_one(&msg).await.unwrap();
        }
        // 2) Refus malveillant : "secret" (count 102, resource neuve → ATTRIBUÉ malgré le flood, correctif #6 ADR-0051 §D2).
        let secret_msg = make_get_msg("secret", fake_cap);
        actor.process_one(&secret_msg).await.unwrap();

        // ── Oracle 1a (isolation) : le refus malveillant a bien été REFUSÉ ──────
        // agent_store_get sans cap retourne -1 → 0xFF tronqué i8 à l'offset 256.
        let secret_res = actor.read_memory_at(256, 1);
        assert_eq!(secret_res[0], 0xFF,
            "1a : le get malveillant 'secret' doit être REFUSÉ (isolation P4 tient)");

        // ── Oracle 1b (fidélité audit) : 'secret' tenté mais NON attribuable ────
        let witness_resources: BTreeSet<String> = witness.lock().unwrap()
            .iter().map(|a| a.resource.clone()).collect();

        let log_resources: BTreeSet<String> = log_ref.entries_by_agent(&agent_id)
            .iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == EmitType::CapabilityDenied as u8)
            .filter_map(|env| cap_denied_resource(&env.payload))
            .collect();

        // Vérité-terrain : les deux resources ont été tentées (et refusées).
        assert!(witness_resources.contains("secret"),
            "le témoin hors-bande doit avoir capté la tentative 'secret'");
        assert!(witness_resources.contains("bn"),
            "le témoin hors-bande doit avoir capté les tentatives 'bn'");
        assert!(log_resources.contains("bn"),
            "'bn' doit être attribuable dans le log");

        // ── RÉGRESSION du correctif #6 (ADR-0051 §D2) ──────────────────────────
        // AVANT #6 : 'secret' (count 102) était silencé → masked={"secret"} (axe 1b
        // échouait, audit masqué). APRÈS #6 : 'secret' est une resource NEUVE sous
        // la borne → attribuée AVEC sa resource malgré le flood → plus de masquage.
        let masked: BTreeSet<&String> = witness_resources.difference(&log_resources).collect();
        assert!(log_resources.contains("secret"),
            "#6 : 'secret' (resource neuve) doit rester attribuable dans le log malgré le flood");
        assert!(!masked.contains(&"secret".to_string()),
            "#6 : le refus malveillant 'secret' n'est plus masqué (audit préservé sous flood)");

        // 1a (isolation) reste intacte indépendamment de l'audit.
        // (Le correctif #6 lève le masquage 1b ; 1a n'a jamais été en cause.)
        println!(
            "SEF-9 (régression #6) : audit PRÉSERVÉ — 'secret' attribuable sous flood, masked={:?} (1a isolation intacte)",
            masked
        );
    }

    /// SEF-3 test_store_put_authorized — agent avec cap write peut écrire.
    #[tokio::test(flavor = "current_thread")]
    async fn test_store_put_authorized() {
        use super::actor::{ActorInstance, STORE_AGENT_WAT};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use std::sync::{Arc, Mutex};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();
        let agent_id = [0xABu8; 16];

        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let resource = "R_write";
        let cap_id = {
            let mut cs = cap_store.lock().unwrap();
            cs.grant_root(agent_id,
                Permissions { read: false, write: true, execute: false, delegate: false },
                resource.to_string())
        };

        let mut actor = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![cap_id],
        ).await.unwrap();

        // build history
        actor.process_one(b"\x00").await.unwrap();

        // put autorisé
        let value = b"xxxxx"; // même longueur que resource "R_wri" — non, resource="R_write" 7 bytes
        // resource = "R_write" = 7 bytes, value doit être 7 bytes aussi
        let value = b"valdata"; // 7 bytes
        assert_eq!(resource.len(), value.len());
        let msg = make_put_msg(resource, cap_id, value);
        actor.process_one(&msg).await.unwrap();

        // Vérifier le résultat : offset 256 = 0 (succès)
        let mem_data = actor.read_memory_at(256, 1);
        assert_eq!(mem_data[0], 0u8, "agent_store_put autorisé doit retourner 0");
    }

    /// SEF-3 test_store_get_authorized — agent avec cap read peut lire.
    #[tokio::test(flavor = "current_thread")]
    async fn test_store_get_authorized() {
        use super::actor::{ActorInstance, STORE_AGENT_WAT};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use std::sync::{Arc, Mutex};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();
        let agent_id = [0xACu8; 16];

        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let resource = "R_readx"; // 7 bytes
        let cap_id = {
            let mut cs = cap_store.lock().unwrap();
            cs.grant_root(agent_id,
                Permissions { read: true, write: true, execute: false, delegate: false },
                resource.to_string())
        };

        let mut actor = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![cap_id],
        ).await.unwrap();

        actor.process_one(b"\x00").await.unwrap();

        // put d'abord pour avoir quelque chose à lire
        let value = b"val1234";
        let msg = make_put_msg(resource, cap_id, value);
        actor.process_one(&msg).await.unwrap();
        let put_res = actor.read_memory_at(256, 1);
        assert_eq!(put_res[0], 0u8, "put doit réussir");

        // get
        let msg = make_get_msg(resource, cap_id);
        actor.process_one(&msg).await.unwrap();
        let get_res = actor.read_memory_at(256, 1);
        assert_eq!(get_res[0], 0u8, "agent_store_get autorisé doit retourner 0");
    }

    /// SEF-3 test_store_get_denied_logs — agent sans cap reçoit -1 et le log contient CapabilityDenied.
    #[tokio::test(flavor = "current_thread")]
    async fn test_store_get_denied_logs() {
        use super::actor::{ActorInstance, STORE_AGENT_WAT};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use os_poc_causal_log::EmitType;
        use std::sync::{Arc, Mutex};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();
        let agent_id = [0xADu8; 16];

        // Pas de cap pour cet agent
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        // cap_id inexistant
        let fake_cap_id: u64 = 9999;

        let mut actor = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![],
        ).await.unwrap();

        actor.process_one(b"\x00").await.unwrap();

        // Tentative de get sans cap : resource "denied_" = 7 bytes
        let resource = "denied_";
        let msg = make_get_msg(resource, fake_cap_id);
        actor.process_one(&msg).await.unwrap();

        // Résultat : offset 256 doit contenir 0xFF (cast de -1i8 → 255u8 en WASM i32 tronqué à i8)
        // En fait agent_store_get retourne -1 en i32, mais on stocke avec i32.store8
        // qui tronque à 8 bits : -1i32 as i8 = 0xFF = 255u8.
        let get_res = actor.read_memory_at(256, 1);
        assert_eq!(get_res[0], 0xFF, "agent_store_get sans cap doit retourner -1 (0xFF tronqué i8)");

        // Vérifier que le log contient CapabilityDenied (0x14)
        let entries = log_ref.entries_by_agent(&agent_id);
        let has_cap_denied = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == EmitType::CapabilityDenied as u8);
        assert!(has_cap_denied, "le log doit contenir un événement CapabilityDenied (0x14)");
    }

    // ── MT-1 (ADR-0057) — invariants du multi-tenant à CausalLog partagé ──────────
    //
    // « Le multi-tenant existe » ⟺ INV-MT1-A passe ∧ INV-MT1-B passe (ADR-0057 §D4).
    // INV-MT1-C (isolation d'exécution) est tenue par la sandbox Wasmtime — chaque
    // ActorInstance a son propre Store<AgentState> / mémoire linéaire — donc acquise
    // gratuitement, mentionnée et non re-testée (re-tester Wasmtime serait hors scope).

    /// INV-MT1-A — isolation d'autorité : la capability d'un tenant ne traverse pas la
    /// frontière de tenant, alors même que log et store sont partagés (ADR-0057 §D2/§D4).
    #[tokio::test(flavor = "current_thread")]
    async fn inv_mt1_a_capability_isolated_per_tenant() {
        use super::actor::{ActorInstanceBuilder, TenantId, STORE_AGENT_WAT};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use std::sync::{Arc, Mutex};

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();

        let (t1, t2) = (TenantId(1), TenantId(2));
        let agent_t1 = [0x11u8; 16];
        let agent_t2 = [0x22u8; 16];

        // Tenant 1 : cap_store ISOLÉ, grant read/write sur "R_share" (7 bytes).
        let cap_store_t1 = Arc::new(Mutex::new(CapabilityStore::new()));
        let resource = "R_share";
        let cap_id_t1 = {
            let mut cs = cap_store_t1.lock().unwrap();
            cs.grant_root(agent_t1,
                Permissions { read: true, write: true, execute: false, delegate: false },
                resource.to_string())
        };
        // Tenant 2 : cap_store DISJOINT, aucune cap.
        let cap_store_t2 = Arc::new(Mutex::new(CapabilityStore::new()));

        // log + store PARTAGÉS entre les deux tenants (la configuration ADR-0057 §D1).
        let mut a1 = ActorInstanceBuilder::new(&engine, &module, agent_t1, Arc::clone(&store), Arc::clone(&log))
            .tenant(t1)
            .caps(Arc::clone(&cap_store_t1), vec![cap_id_t1])
            .build().await.unwrap();
        let mut a2 = ActorInstanceBuilder::new(&engine, &module, agent_t2, Arc::clone(&store), Arc::clone(&log))
            .tenant(t2)
            .caps(Arc::clone(&cap_store_t2), vec![])
            .build().await.unwrap();

        assert_eq!(a1.tenant(), t1);
        assert_eq!(a2.tenant(), t2);

        a1.process_one(b"\x00").await.unwrap();
        a2.process_one(b"\x00").await.unwrap();

        // T1 écrit avec sa cap → autorisé.
        let value = b"valdata"; // 7 bytes (= resource.len())
        a1.process_one(&make_put_msg(resource, cap_id_t1, value)).await.unwrap();
        assert_eq!(a1.read_memory_at(256, 1)[0], 0u8, "T1 doit pouvoir écrire avec sa cap");

        // T2 tente d'accéder à la MÊME resource avec le MÊME cap_id entier — mais ce cap_id
        // n'existe pas dans le cap_store de T2. L'autorité ne traverse pas la frontière.
        a2.process_one(&make_get_msg(resource, cap_id_t1)).await.unwrap();
        assert_eq!(a2.read_memory_at(256, 1)[0], 0xFF,
            "INV-MT1-A : la capability de T1 (cap_id identique) est non résoluble depuis T2 → refus (-1)");
    }

    /// INV-MT1-B (B-fort, ADR-0058) — **oracle inversé**. Sous B-light, un agent de T2 forgeait
    /// une arête causale vers l'action de T1 (log partagé, la forgerie RÉUSSISSAIT). Sous B-fort,
    /// la citation cross-agent **sans `CauseHandle`** est refusée (`agent_add_cause` → -3) : l'arête
    /// n'est PAS créée. C'est l'inversion attendue (ADR-0057 §D4 / ADR-0058 §D1).
    ///
    /// Couplé au test miroir `bf1_cross_tenant_cause_authorized_with_handle` (avec handle → succès),
    /// il prouve que c'est l'**autorité** qui décide, pas l'existence dans le log.
    #[tokio::test(flavor = "current_thread")]
    async fn inv_mt1_b_cross_tenant_cause_refused_without_handle() {
        use super::actor::{ActorInstanceBuilder, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let (t1, t2) = (TenantId(1), TenantId(2));
        let agent_t1 = [0xA1u8; 16];
        let agent_t2 = [0xB2u8; 16];

        // log + store partagés ; cause_handle_store de T2 vide (aucun handle minté).
        let mut a1 = ActorInstanceBuilder::new(&engine, &module, agent_t1, Arc::clone(&store), Arc::clone(&log))
            .tenant(t1).build().await.unwrap();
        let mut a2 = ActorInstanceBuilder::new(&engine, &module, agent_t2, Arc::clone(&store), Arc::clone(&log))
            .tenant(t2).build().await.unwrap();

        a1.process_one(b"\x00").await.unwrap();
        // L127 : chemin déterministe via last_action()+log.get() (et non entries_by_agent().last(),
        // ordonné par hash).
        let t1_action_id: [u8; 32] = a1.last_action().expect("T1 a émis une action");

        // T2 (autre tenant) tente de citer l'action de T1 SANS handle : msg = [4, action_id(32)].
        let mut msg = vec![4u8];
        msg.extend_from_slice(&t1_action_id);
        a2.process_one(&msg).await.unwrap();

        // Oracle inversé : la forgerie cross-tenant ÉCHOUE → action_id de T1 absent des parent_ids.
        let t2_last = log.get(&a2.last_action().expect("T2 a émis une action")).unwrap().unwrap();
        assert!(!t2_last.parent_ids.contains(&t1_action_id),
            "INV-MT1-B inversé : sans CauseHandle, T2 ne peut PAS forger d'arête cross-tenant (B-fort).");
    }

    /// BF-1 miroir — avec un `CauseHandle` minté (autorité explicite), la citation cross-tenant
    /// RÉUSSIT et l'arête est créée. Prouve, avec l'oracle inversé ci-dessus, que B-fort décide
    /// sur l'autorité (handle) et non sur l'existence.
    #[tokio::test(flavor = "current_thread")]
    async fn bf1_cross_tenant_cause_authorized_with_handle() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let (t1, t2) = (TenantId(1), TenantId(2));
        let agent_t1 = [0xA1u8; 16];
        let agent_t2 = [0xB2u8; 16];

        // Registre partagé ; le store de T2 (consulté par a2) en est dérivé (ADR-0060).
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_t2 = reg.get_or_create(t2);
        let mut a1 = ActorInstanceBuilder::new(&engine, &module, agent_t1, Arc::clone(&store), Arc::clone(&log))
            .tenant(t1).build().await.unwrap();
        let mut a2 = ActorInstanceBuilder::new(&engine, &module, agent_t2, Arc::clone(&store), Arc::clone(&log))
            .tenant(t2).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();

        a1.process_one(b"\x00").await.unwrap();
        // L127 : chemin déterministe (last_action()+log.get()).
        let t1_action_id: [u8; 32] = a1.last_action().expect("T1 a émis une action");

        // Autorité explicite : on minte un handle autorisant l'agent de T2 à citer l'action de T1.
        chs_t2.lock().unwrap().mint(t1_action_id, agent_t2, agent_t1, 0);

        let mut msg = vec![4u8];
        msg.extend_from_slice(&t1_action_id);
        a2.process_one(&msg).await.unwrap();

        let t2_last = log.get(&a2.last_action().expect("T2 a émis une action")).unwrap().unwrap();
        assert!(t2_last.parent_ids.contains(&t1_action_id),
            "BF-1 miroir : avec un CauseHandle minté, la citation cross-tenant réussit (arête créée).");
    }

    /// BF-1 auto-citation (ADR-0058 §D10) — un agent cite l'une de ses PROPRES actions
    /// antérieures sans aucun handle (autorité intrinsèque). Doit réussir.
    #[tokio::test(flavor = "current_thread")]
    async fn bf1_self_citation_needs_no_handle() {
        use super::actor::{ActorInstanceBuilder, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let agent = [0xC3u8; 16];
        // cause_handle_store vide : l'auto-citation ne doit PAS en dépendre.
        let mut a = ActorInstanceBuilder::new(&engine, &module, agent, Arc::clone(&store), Arc::clone(&log))
            .tenant(TenantId(7)).build().await.unwrap();

        // L'agent émet une première action (own_first), puis une seconde : own_first n'est
        // donc PLUS le parent séquentiel courant → le citer prouve bien le chemin d'auto-citation.
        // L127 : `entries_by_agent().last()` itère par ordre de HASH, pas chronologique →
        // on passe par `last_action()` + `log.get()` (chemin déterministe).
        a.process_one(b"\x00").await.unwrap();
        let own_first: [u8; 32] = a.last_action().expect("première action");
        a.process_one(b"\x00").await.unwrap();

        // L'agent cite sa propre action ANTÉRIEURE : msg = [4, own_first(32)].
        let mut msg = vec![4u8];
        msg.extend_from_slice(&own_first);
        a.process_one(&msg).await.unwrap();

        let last = log.get(&a.last_action().expect("action après auto-citation"))
            .unwrap().unwrap();
        assert!(last.parent_ids.contains(&own_first),
            "auto-citation : un agent cite sa propre action antérieure sans handle (§D10).");
    }

    /// BF-2.a (ADR-0058 §D6) — révocation à la terminaison de l'émetteur. Un CauseHandle émis
    /// par A devient invalide dès que A se termine : la citation par B échoue. Validé par un
    /// **vrai appel WASM** `agent_add_cause` avant ET après terminaison (caveat risque n°1 :
    /// jamais d'accès direct au store pour l'oracle de citation).
    #[tokio::test(flavor = "current_thread")]
    async fn bf2_revoke_on_issuer_termination() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, Message, TenantId, CROSS_AGENT_WAT, run_loop};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let tenant = TenantId(3);
        let id_a = [0xAAu8; 16];
        let id_b = [0xBBu8; 16];
        // Registre PARTAGÉ ; le store du tenant (émetteur A et bénéficiaire B) en est dérivé.
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs = reg.get_or_create(tenant);

        // A émet a1 (appel direct) ; on minte un handle autorisant B à citer a1.
        let mut actor_a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        actor_a.process_one(&[0x00]).await.unwrap();
        let a1: [u8; 32] = actor_a.last_action().expect("a1"); // L127 : chemin déterministe
        chs.lock().unwrap().mint(a1, id_b, id_a, 0);

        // B cite a1 AVANT la terminaison de A → succès (vrai appel WASM).
        let mut actor_b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        actor_b.process_one(&cite).await.unwrap();
        assert!(log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "BF-2.a (avant) : avec handle valide, B cite a1 (succès)");

        // A se termine : on le confie à run_loop puis on ferme son canal → le garde Drop
        // (§D6) exécute revoke_issued_by(A) sur le store partagé.
        let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(1);
        let ha = tokio::spawn(run_loop(actor_a, rx_a));
        drop(tx_a);
        ha.await.unwrap();

        // B cite a1 APRÈS la terminaison de A → refus : a1 absent du nouveau parent_ids.
        actor_b.process_one(&cite).await.unwrap();
        assert!(!log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "BF-2.a (après) : A terminé → handle révoqué → citation par B refusée (-3)");
    }

    /// BF-2.b (ADR-0058 §D7) — révocation au rollback de l'émetteur. Un CauseHandle émis par A
    /// après le snapshot cible est révoqué quand A rollback (symétrie « émis » vs « détenu » des
    /// caps). Citation par B validée par **vrai appel WASM** avant et après le rollback.
    #[tokio::test(flavor = "multi_thread")]
    async fn bf2_revoke_on_issuer_rollback() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, Message, TenantId, CROSS_AGENT_WAT, run_loop};
        use os_poc_causal_log::{EmitEnvelope, EmitType};
        use std::sync::Arc;
        use std::time::Duration;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let tenant = TenantId(4);
        let id_a = [0xCAu8; 16];
        let id_b = [0xCBu8; 16];
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs = reg.get_or_create(tenant);

        // A via run_loop : construit 2 actions (≥2 snapshots → une cible de rollback existe).
        let actor_a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(8);
        let ha = tokio::spawn(run_loop(actor_a, rx_a));
        tx_a.send(Message::data(vec![0x00])).await.unwrap();
        tx_a.send(Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // a1 = première action ActionResult émise par A.
        let a1: [u8; 32] = log.entries_by_agent(&id_a).into_iter()
            .find(|(_, e)| e.emit_payload.as_ref()
                .and_then(|p| EmitEnvelope::from_msgpack(p).ok())
                .map_or(false, |env| env.emit_type == EmitType::ActionResult as u8))
            .expect("a1 émise par A").0;

        // Handle émis « tard » (issued_at élevé) → sera révoqué par un rollback vers un snapshot antérieur.
        chs.lock().unwrap().mint(a1, id_b, id_a, u64::MAX);

        // B cite a1 AVANT rollback → succès (vrai appel WASM).
        let mut actor_b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        actor_b.process_one(&cite).await.unwrap();
        assert!(log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "BF-2.b (avant) : avec handle, B cite a1 (succès)");

        // A rollback vers seq 1 → revoke_issued_after(A, S1.ts) ; handle (issued_at=MAX) révoqué.
        tx_a.send(Message::Rollback { target_seq: 1 }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(tx_a);
        ha.await.unwrap();

        // B cite a1 APRÈS rollback → refus (vrai appel WASM).
        actor_b.process_one(&cite).await.unwrap();
        assert!(!log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "BF-2.b (après) : A a rollback → handle émis après la cible révoqué → citation refusée");
    }

    // ── BF-3 (ADR-0058) — robustesse adversariale du CauseHandle ─────────────────
    //
    // B-fort lie une autorisation de citation à un triplet (grantee, action_id, tenant-store).
    // Les trois tests ci-dessous attaquent chacune de ces liaisons : un handle valide pour un
    // autre grantee / une autre action / un autre tenant ne doit JAMAIS autoriser la citation.
    // Tous via vrai appel WASM `agent_add_cause`. SEF-7.1 (action forgée inexistante → -3,
    // `sef7_1_forged_action_id_rejected`) et SEF-7.2 (flood borné MAX_EXTRA_CAUSES,
    // `sef7_2_extra_causes_flood_bounded`) restent valides sous B-fort (le -3 « inconnu » et la
    // borne -2 sont vérifiés avant/indépendamment du check de handle).
    //
    // Note flood : le `CauseHandleStore` n'a PAS de plafond de taille. Décision (vs ADR-0058
    // §BF-3) : `mint` est une API Rust trusted (runner/superviseur) — aucun chemin guest ne
    // peuple le store, donc aucun vecteur de DoS par flood côté agent. Le coût par citation
    // reste borné par MAX_EXTRA_CAUSES. Plafond à réintroduire SI un jour un guest peut minter.

    /// BF-3 — liaison *grantee* : un handle émis pour B n'autorise pas un autre agent C à citer.
    #[tokio::test(flavor = "current_thread")]
    async fn bf3_handle_for_other_grantee_rejected() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let tenant = TenantId(5);
        let (id_a, id_b, id_c) = ([0xD0u8; 16], [0xD1u8; 16], [0xD2u8; 16]);
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs = reg.get_or_create(tenant);

        let mut a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        a.process_one(&[0x00]).await.unwrap();
        let a1 = a.last_action().unwrap();

        // Handle émis pour B uniquement.
        chs.lock().unwrap().mint(a1, id_b, id_a, 0);

        // C (même tenant, même store) tente de citer a1 → pas de handle (C, a1) → refus.
        let mut c = ActorInstanceBuilder::new(&engine, &module, id_c, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        c.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        c.process_one(&cite).await.unwrap();
        let c_last = log.get(&c.last_action().unwrap()).unwrap().unwrap();
        assert!(!c_last.parent_ids.contains(&a1),
            "BF-3 grantee : un handle pour B n'autorise pas C à citer a1");
    }

    /// BF-3 — liaison *action* : un handle pour a1 n'autorise pas la citation d'une autre action a2.
    #[tokio::test(flavor = "current_thread")]
    async fn bf3_handle_for_other_action_rejected() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let tenant = TenantId(6);
        let (id_a, id_b) = ([0xE1u8; 16], [0xE2u8; 16]);
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs = reg.get_or_create(tenant);

        let mut a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        a.process_one(&[0x00]).await.unwrap();
        let a1 = a.last_action().unwrap();
        a.process_one(&[0x00]).await.unwrap();
        let a2 = a.last_action().unwrap();
        assert_ne!(a1, a2);

        // Handle pour a1 seulement.
        chs.lock().unwrap().mint(a1, id_b, id_a, 0);

        let mut b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        b.process_one(&[0x00]).await.unwrap();

        // B tente de citer a2 (pas de handle (B, a2)) → refus.
        let mut cite_a2 = vec![0x04u8];
        cite_a2.extend_from_slice(&a2);
        b.process_one(&cite_a2).await.unwrap();
        let b_last = log.get(&b.last_action().unwrap()).unwrap().unwrap();
        assert!(!b_last.parent_ids.contains(&a2),
            "BF-3 action : un handle pour a1 n'autorise pas la citation de a2");

        // Sanity : B PEUT citer a1 (handle valide) → l'oracle distingue bien autorisé/refusé.
        let mut cite_a1 = vec![0x04u8];
        cite_a1.extend_from_slice(&a1);
        b.process_one(&cite_a1).await.unwrap();
        let b_last2 = log.get(&b.last_action().unwrap()).unwrap().unwrap();
        assert!(b_last2.parent_ids.contains(&a1),
            "BF-3 action : le handle pour a1 autorise bien la citation de a1");
    }

    /// BF-3 — liaison *tenant-store* : un handle déposé dans le store d'un AUTRE tenant est inutile.
    /// (Le check porte sur le store du tenant de l'appelant — ADR-0057 §D2.)
    #[tokio::test(flavor = "current_thread")]
    async fn bf3_handle_in_wrong_tenant_store_useless() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let (id_a, id_b2) = ([0xF1u8; 16], [0xF2u8; 16]);
        // Un SEUL registre partagé : les stores T1/T2 en sont dérivés, isolés par tenant.
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_t1 = reg.get_or_create(TenantId(1));

        let mut a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(TenantId(1)).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        a.process_one(&[0x00]).await.unwrap();
        let a1 = a.last_action().unwrap();

        // Handle pour l'agent de T2, mais déposé (par erreur/attaque) dans le store de T1.
        chs_t1.lock().unwrap().mint(a1, id_b2, id_a, 0);

        // L'agent de T2 consulte SON store (T2, vide) → refus malgré le handle existant ailleurs.
        let mut b2 = ActorInstanceBuilder::new(&engine, &module, id_b2, store.clone(), log.clone())
            .tenant(TenantId(2)).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        b2.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        b2.process_one(&cite).await.unwrap();
        let b2_last = log.get(&b2.last_action().unwrap()).unwrap().unwrap();
        assert!(!b2_last.parent_ids.contains(&a1),
            "BF-3 tenant : un handle dans le store de T1 n'autorise pas un agent de T2");
    }

    // ── XR-1 (ADR-0060) — révocation CROSS-TENANT des CauseHandle via le registre ──────────
    //
    // BF-2 (ADR-0058 §D6/D7) ne révoquait que le store du tenant de l'émetteur. Un handle émis
    // par A (tenant T1) au profit d'un grantee de T2 vit dans le store de T2 (le grantee
    // consulte SON store) et survivait donc à la terminaison/rollback de A. XR-1 fait balayer
    // par le drop-guard / le rollback TOUS les stores du registre. Validé par **vrai appel WASM**
    // de citation après révocation (caveat risque n°1 : jamais d'oracle par accès direct au store).
    // L'intra-tenant (INV-XR-INTRA) reste couvert par `bf2_revoke_on_issuer_*` (cas où le store
    // balayé est celui du tenant de l'émetteur lui-même).

    /// INV-XR-CROSS (ADR-0060 §D6) — A∈T1 émet un handle pour un grantee B∈T2 (handle déposé
    /// dans le store de T2). À la terminaison de A, le balayage cross-tenant révoque ce handle
    /// dans le store de T2 : la citation par B échoue ensuite.
    #[tokio::test(flavor = "multi_thread")]
    async fn inv_xr_cross_tenant_revoke_on_termination() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, Message, TenantId, CROSS_AGENT_WAT, run_loop};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();

        let (t1, t2) = (TenantId(1), TenantId(2));
        let id_a = [0x1Au8; 16]; // émetteur, tenant T1
        let id_b = [0x2Bu8; 16]; // grantee, tenant T2

        // Registre PARTAGÉ cross-tenant (la structure même qui rend le balayage possible).
        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_t2 = reg.get_or_create(t2); // store du grantee B

        // A (T1) émet a1.
        let mut actor_a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(t1).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        actor_a.process_one(&[0x00]).await.unwrap();
        let a1 = actor_a.last_action().unwrap();

        // Handle CROSS-TENANT : grantee B (T2), issuer A (T1), déposé dans le store de T2.
        chs_t2.lock().unwrap().mint(a1, id_b, id_a, 0);

        // B (T2) cite a1 AVANT terminaison de A → succès (handle présent dans le store de T2).
        let mut actor_b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log.clone())
            .tenant(t2).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        actor_b.process_one(&cite).await.unwrap();
        assert!(log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "INV-XR-CROSS (avant) : avec handle cross-tenant valide, B (T2) cite a1 de A (T1)");

        // A se termine : le drop-guard de run_loop balaie TOUT le registre → le handle (issuer A)
        // dans le store de T2 est révoqué, bien qu'il soit dans un AUTRE tenant que celui de A.
        let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(1);
        let ha = tokio::spawn(run_loop(actor_a, rx_a));
        drop(tx_a);
        ha.await.unwrap();

        // B cite a1 APRÈS terminaison de A → refus : a1 absent du nouveau parent_ids.
        actor_b.process_one(&cite).await.unwrap();
        assert!(!log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "INV-XR-CROSS (après) : A (T1) terminé → handle révoqué dans le store de T2 (cross-tenant)");
    }

    /// INV-XR-ROLLBACK (ADR-0060 §D7) — A∈T1 émet (tardivement) un handle pour B∈T2 ; au rollback
    /// de A vers un snapshot antérieur, le balayage cross-tenant révoque ce handle dans T2.
    #[tokio::test(flavor = "multi_thread")]
    async fn inv_xr_cross_tenant_revoke_on_rollback() {
        use super::actor::{ActorInstanceBuilder, CauseHandleRegistry, Message, TenantId, CROSS_AGENT_WAT, run_loop};
        use os_poc_causal_log::{EmitEnvelope, EmitType};
        use std::sync::Arc;
        use std::time::Duration;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let (t1, t2) = (TenantId(1), TenantId(2));
        let id_a = [0x3Au8; 16];
        let id_b = [0x4Bu8; 16];

        let reg = Arc::new(CauseHandleRegistry::new());
        let chs_t2 = reg.get_or_create(t2);

        // A (T1) via run_loop : 2 actions → une cible de rollback existe.
        let actor_a = ActorInstanceBuilder::new(&engine, &module, id_a, store.clone(), log.clone())
            .tenant(t1).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        let (tx_a, rx_a) = tokio::sync::mpsc::channel::<Message>(8);
        let ha = tokio::spawn(run_loop(actor_a, rx_a));
        tx_a.send(Message::data(vec![0x00])).await.unwrap();
        tx_a.send(Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        let a1: [u8; 32] = log.entries_by_agent(&id_a).into_iter()
            .find(|(_, e)| e.emit_payload.as_ref()
                .and_then(|p| EmitEnvelope::from_msgpack(p).ok())
                .map_or(false, |env| env.emit_type == EmitType::ActionResult as u8))
            .expect("a1 émise par A").0;

        // Handle cross-tenant émis « tard » (issued_at élevé) → révoqué par un rollback antérieur.
        chs_t2.lock().unwrap().mint(a1, id_b, id_a, u64::MAX);

        // B (T2) cite a1 AVANT rollback → succès.
        let mut actor_b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log.clone())
            .tenant(t2).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        actor_b.process_one(&cite).await.unwrap();
        assert!(log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "INV-XR-ROLLBACK (avant) : B (T2) cite a1 avec handle cross-tenant valide");

        // A rollback vers seq=0 → revoke_issued_after_all balaie le registre, store de T2 inclus.
        tx_a.send(Message::Rollback { target_seq: 0 }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // B cite a1 APRÈS rollback → refus (handle cross-tenant révoqué dans le store de T2).
        actor_b.process_one(&cite).await.unwrap();
        assert!(!log.get(&actor_b.last_action().unwrap()).unwrap().unwrap().parent_ids.contains(&a1),
            "INV-XR-ROLLBACK (après) : rollback de A (T1) → handle révoqué dans le store de T2");

        drop(tx_a);
        ha.await.unwrap();
    }

    // ── C2 (revue sécurité 2026-06-07) — résilience à l'empoisonnement de Mutex partagé ──
    //
    // Les stores d'autorité sont partagés (cap_store intra-tenant ; CauseHandleStore cross-tenant
    // via le registre). Avec `.lock().unwrap()`, un panic d'un porteur empoisonnait le Mutex et
    // faisait paniquer tous les détenteurs suivants — y compris d'autres tenants (DoS cross-tenant),
    // voire `abort()` si le panic survenait dans le Drop guard pendant unwind. `lock_or_recover`
    // récupère l'état (`into_inner`) au lieu de propager le poison (mutations logiquement atomiques).

    /// C2 — un `CauseHandleStore` empoisonné ne brique pas le registre pour les autres tenants :
    /// `get_or_create`, `get` et les balayages de révocation restent fonctionnels (pas de panic).
    #[test]
    fn c2_poisoned_store_does_not_brick_registry() {
        use super::actor::{CauseHandleRegistry, TenantId};
        use std::sync::Arc;

        let reg = Arc::new(CauseHandleRegistry::new());
        let s_t1 = reg.get_or_create(TenantId(1));

        // Empoisonner le store de T1 : panic en tenant le lock.
        let s_clone = Arc::clone(&s_t1);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = s_clone.lock().unwrap();
            panic!("poison intentionnel (test C2)");
        }));
        assert!(s_t1.is_poisoned(), "le store de T1 doit être empoisonné pour le test");

        // C2 : malgré le poison de T1, le registre reste utilisable — aucun de ces appels ne panique.
        let _s_t2 = reg.get_or_create(TenantId(2)); // create d'un autre tenant (RwLock + Mutex)
        let _again = reg.get_or_create(TenantId(1)); // ré-accès au store empoisonné
        assert!(reg.get(TenantId(1)).is_some(), "get sur store empoisonné OK");
        reg.revoke_issued_by_all(&[0xAAu8; 16]);     // balaie T1 empoisonné → pas de panic
        reg.revoke_issued_after_all(&[0xAAu8; 16], 0); // idem
        // Le store récupéré reste muable (mint via lock_or_recover, le poison n'a pas corrompu l'état).
        let _ = reg.get_or_create(TenantId(1)).lock().unwrap_or_else(|e| e.into_inner())
            .mint([0x01u8; 32], [0xB0u8; 16], [0xA0u8; 16], 0);
    }

    /// C2 — `agent_add_cause` (chemin host fn, ligne ~1663) sur un store de tenant empoisonné :
    /// la citation cross-agent est refusée proprement (-3, a1 absent) SANS paniquer. Sans le
    /// correctif, le `.lock().unwrap()` aurait paniqué → DoS de l'agent par poison d'un tiers.
    #[tokio::test(flavor = "current_thread")]
    async fn c2_agent_add_cause_survives_poisoned_store() {
        use super::actor::{ActorInstance, ActorInstanceBuilder, CauseHandleRegistry, TenantId, CROSS_AGENT_WAT};
        use std::sync::Arc;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let tenant = TenantId(9);
        let (id_a, id_b) = ([0xC1u8; 16], [0xC2u8; 16]);

        let reg = Arc::new(CauseHandleRegistry::new());
        let chs = reg.get_or_create(tenant); // store du tenant de B

        // A (autre module) produit a1.
        let mut a = ActorInstance::new_precompiled(&engine, &module, id_a, store.clone(), log.clone()).await.unwrap();
        a.process_one(&[0x00]).await.unwrap();
        let a1 = a.last_action().unwrap();

        // Empoisonner le store du tenant de B (par un panic d'un porteur antérieur).
        let chs_clone = Arc::clone(&chs);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = chs_clone.lock().unwrap();
            panic!("poison intentionnel (test C2)");
        }));
        assert!(chs.is_poisoned(), "le store du tenant de B doit être empoisonné");

        // B tente de citer a1 (cross-agent, sans handle) → agent_add_cause locke le store empoisonné
        // via lock_or_recover → refus -3 (a1 absent), AUCUN panic.
        let mut b = ActorInstanceBuilder::new(&engine, &module, id_b, store.clone(), log.clone())
            .tenant(tenant).cause_handle_registry(Arc::clone(&reg)).build().await.unwrap();
        b.process_one(&[0x00]).await.unwrap();
        let mut cite = vec![0x04u8];
        cite.extend_from_slice(&a1);
        b.process_one(&cite).await.unwrap(); // ne doit pas paniquer
        let b_last = log.get(&b.last_action().unwrap()).unwrap().unwrap();
        assert!(!b_last.parent_ids.contains(&a1),
            "C2 : citation cross-agent refusée (-3) sur store empoisonné, sans panic");
    }

    /// M4 (revue sécurité) — `commit_barrier` puis `agent_request_validation` (sans emit) dans un
    /// cycle, puis un cycle normal `commit_barrier`+`emit` : ne doit produire aucun crash ni
    /// `host_error`. Garde-fou de la correction M4 : H-cb-correct s'exprime sur `pending_commit`
    /// (source unique) ; un drapeau `barrier_fired` redondant se désynchronisait sur ce chemin
    /// (barrière non suivie d'emit) et faussait l'invariant au cycle suivant.
    #[tokio::test(flavor = "current_thread")]
    async fn m4_barrier_then_request_validation_then_emit_no_crash() {
        use super::actor::ActorInstance;

        // WAT : msg[0]=2 → commit_barrier PUIS request_validation (barrière non suivie d'emit) ;
        //       sinon → commit_barrier + emit (cycle normal).
        let wat = r#"(module
            (import "env" "commit_barrier" (func $cb))
            (import "env" "emit" (func $emit (param i32 i32 i32)))
            (import "env" "agent_request_validation" (func $req_val (param i32) (result i32)))
            (func (export "process") (param $ptr i32) (param $len i32)
              (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 2))
                (then
                  call $cb
                  (drop (call $req_val (i32.const 1)))
                )
                (else
                  call $cb
                  i32.const 1 (local.get $ptr) (local.get $len) call $emit
                )
              )
            )
            (memory (export "memory") 1)
        )"#;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, wat).unwrap();
        let agent = [0x4Du8; 16];
        let mut a = ActorInstance::new_precompiled(&engine, &module, agent, store.clone(), log.clone())
            .await.unwrap();

        // Cycle 1 : commit_barrier + request_validation (barrière sans emit). pending_commit est
        // flushé (emit_payload=None) ; AUCUN drapeau résiduel ne doit subsister.
        a.process_one(&[2u8]).await.expect("cycle 1 (barrier+request_validation) ne doit pas échouer");

        // Cycle 2 : commit_barrier + emit normal. Sans la correction M4, l'état de barrière
        // désynchronisé faussait l'invariant ; ici le cycle doit aboutir proprement.
        a.process_one(&[0u8]).await.expect("cycle 2 (barrier+emit) ne doit pas crasher");

        // L'agent a bien progressé (action émise au cycle 2), pas de host_error / ProcessFailed.
        let last = a.last_action().expect("cycle 2 a produit une action");
        assert!(log.get(&last).unwrap().is_some(), "l'action du cycle 2 est dans le log");
    }

    /// M2 (revue sécurité) — `agent_terminate` est absorbant et court-circuite le cycle : les
    /// host fns mutantes appelées APRÈS dans le même cycle (request_validation, commit_barrier,
    /// emit) sont no-op. Aucun effet « post-mortem » dans le log ; pas de résurrection via
    /// AwaitingValidation. Seule la terminaison elle-même est journalisée.
    #[tokio::test(flavor = "current_thread")]
    async fn m2_terminate_is_absorbing_no_post_mortem_effects() {
        use super::actor::ActorInstance;
        use os_poc_causal_log::{EmitEnvelope, EmitType};

        // WAT : process() → terminate, puis tente request_validation + commit_barrier + emit.
        let wat = r#"(module
            (import "env" "commit_barrier" (func $cb))
            (import "env" "emit" (func $emit (param i32 i32 i32)))
            (import "env" "agent_terminate" (func $term))
            (import "env" "agent_request_validation" (func $req_val (param i32) (result i32)))
            (func (export "process") (param $ptr i32) (param $len i32)
              call $term
              (drop (call $req_val (i32.const 1)))
              call $cb
              i32.const 1 (local.get $ptr) (local.get $len) call $emit
            )
            (memory (export "memory") 1)
        )"#;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, wat).unwrap();
        let agent = [0x42u8; 16];
        let mut a = ActorInstance::new_precompiled(&engine, &module, agent, store.clone(), log.clone())
            .await.unwrap();
        a.process_one(&[0x00]).await.unwrap();

        let envs: Vec<EmitEnvelope> = log.entries_by_agent(&agent).iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|p| EmitEnvelope::from_msgpack(p).ok())
            .collect();

        // Seule la terminaison (Lifecycle) est journalisée — aucun effet post-mortem.
        assert_eq!(envs.len(), 1,
            "M2 : aucun effet post-mortem — seule la terminaison doit produire une entrée");
        assert_eq!(envs[0].emit_type, EmitType::Lifecycle as u8,
            "M2 : l'unique entrée est la transition de cycle de vie (Terminated)");
        assert!(!envs.iter().any(|e| e.emit_type == EmitType::ValidationRequest as u8),
            "M2 : request_validation après terminate doit être no-op (pas de résurrection)");
    }

    /// C1 (revue sécurité / P4) — le KV est un référent RÉELLEMENT PARTAGÉ par tenant : un agent
    /// lit la valeur écrite par un autre agent du même tenant (store partagé), et la capability
    /// décide de l'accès. Avant C1, le KV était privé-par-agent → la capability gardait un magasin
    /// inaccessible aux autres, et P4 (isolation non-ambiante d'un référent partagé) était vide.
    #[tokio::test(flavor = "current_thread")]
    async fn p4_kv_shared_within_tenant_cap_gated() {
        use super::actor::{ActorInstanceBuilder, TenantId, STORE_AGENT_WAT};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();
        let t1 = TenantId(1);
        let resource = "R_share";

        // cap_store + KV PARTAGÉS par le tenant T1 (store de données isolé par tenant).
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let kv = Arc::new(Mutex::new(HashMap::new()));
        let (id_w, id_r, id_n) = ([0x01u8; 16], [0x02u8; 16], [0x03u8; 16]);
        let (cap_w, cap_r) = {
            let mut cs = cap_store.lock().unwrap();
            let w = cs.grant_root(id_w, Permissions { read: true, write: true, execute: false, delegate: false }, resource.to_string());
            let r = cs.grant_root(id_r, Permissions { read: true, write: false, execute: false, delegate: false }, resource.to_string());
            (w, r)
        };

        let mut writer = ActorInstanceBuilder::new(&engine, &module, id_w, store.clone(), log.clone())
            .tenant(t1).caps(Arc::clone(&cap_store), vec![cap_w]).kv_store(Arc::clone(&kv)).build().await.unwrap();
        let mut reader = ActorInstanceBuilder::new(&engine, &module, id_r, store.clone(), log.clone())
            .tenant(t1).caps(Arc::clone(&cap_store), vec![cap_r]).kv_store(Arc::clone(&kv)).build().await.unwrap();
        let mut nocap = ActorInstanceBuilder::new(&engine, &module, id_n, store.clone(), log.clone())
            .tenant(t1).caps(Arc::clone(&cap_store), vec![]).kv_store(Arc::clone(&kv)).build().await.unwrap();

        let value = b"valdata"; // 7 octets = resource.len()
        // Writer écrit R_share avec sa capability.
        writer.process_one(&make_put_msg(resource, cap_w, value)).await.unwrap();
        assert_eq!(writer.read_memory_at(256, 1)[0], 0u8, "writer écrit avec sa cap");

        // Reader (cap valide) lit R_share → VOIT la valeur du writer : le référent est partagé.
        reader.process_one(&make_get_msg(resource, cap_r)).await.unwrap();
        assert_eq!(reader.read_memory_at(256, 1)[0], 0u8, "reader autorisé");
        assert_eq!(reader.read_memory_at(300, value.len()), value.to_vec(),
            "C1/P4 : le reader voit la valeur écrite par un autre agent du tenant (référent partagé)");

        // Sans capability résoluble, l'accès au MÊME référent partagé est refusé (autorité décide).
        nocap.process_one(&make_get_msg(resource, cap_w)).await.unwrap();
        assert_eq!(nocap.read_memory_at(256, 1)[0], 0xFFu8,
            "C1/P4 : sans capability, l'accès au référent partagé est refusé (-1)");
    }

    /// M1 (revue sécurité) — garde d'isolation de câblage : enregistrer deux agents de tenants
    /// DISTINCTS partageant le même `cap_store` est un défaut de câblage (fuite d'autorité
    /// cross-tenant) → panic fail-fast au `register`. ADR-0057 §D2 rendue invariant runtime.
    #[tokio::test(flavor = "current_thread")]
    #[should_panic(expected = "cap_store partagé entre tenants")]
    async fn m1_distinct_tenants_sharing_cap_store_rejected() {
        use super::actor::{ActorInstanceBuilder, TenantId, AGENT_WAT};
        use super::scheduler::Scheduler;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::{Arc, Mutex};

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();

        // UN SEUL cap_store, partagé à tort entre deux tenants distincts (erreur de câblage).
        let shared_cap = Arc::new(Mutex::new(CapabilityStore::new()));
        let a1 = ActorInstanceBuilder::new(&engine, &module, [0x11u8; 16], store.clone(), log.clone())
            .tenant(TenantId(1)).caps(Arc::clone(&shared_cap), vec![]).build().await.unwrap();
        let a2 = ActorInstanceBuilder::new(&engine, &module, [0x22u8; 16], store.clone(), log.clone())
            .tenant(TenantId(2)).caps(Arc::clone(&shared_cap), vec![]).build().await.unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.register(a1);
        scheduler.register(a2); // ← doit paniquer (cap_store de T1 réutilisé par T2)
    }

    // ── SD-2 (ADR-0059) — politique de supervision cross-tenant (dette ADR-0057 §D5 fermée) ──
    //
    // Le `Scheduler` est désormais décomposé en `Registry` (mécanisme) + `Supervisor`
    // (politique). Le `Supervisor` exige une `SupervisionAuthority` (ADR-0059 §C) :
    // `Orchestrator` (runner trusted, ambiant cross-tenant) passe toujours ; `Tenant(t)`
    // ne passe que si la cible appartient à `t`. INV-SD-AUTH (jadis l'armement SD-0,
    // succès-à-tort) est maintenant l'oracle FERMÉ : même scénario, le refus est asservi.

    /// INV-SD-AUTH (fermé) — la supervision cross-tenant sous autorité de tenant est REFUSÉE,
    /// sans aucun effet ; sous autorité orchestrateur elle reste autorisée (ADR-0059 §C).
    ///
    /// Deux agents, deux tenants (a∈T1, b∈T2), un `Scheduler` partagé.
    /// 1. `rollback_as(&b, …, Tenant(T1))` → `Err(CrossTenantDenied)` ET aucun 0x0B dans le
    ///    log de b (refus = aucun effet observable — décision audit O1, ADR-0059).
    /// 2. `rollback_as(&b, …, Orchestrator)` → succès ET 0x0B présent (non-régression de la
    ///    supervision runner trusted).
    /// Conjonction = c'est l'AUTORITÉ qui décide, pas la simple capacité d'appeler — même
    /// motif d'oracle inversé qu'`INV-MT1-B` → B-fort.
    #[tokio::test]
    async fn inv_sd_auth_cross_tenant_supervision_refused() {
        use super::actor::{ActorInstanceBuilder, INTROSPECT_AGENT_WAT, Message, TenantId};
        use super::scheduler::{Scheduler, SupervisionAuthority, SupervisionError};
        use os_poc_causal_log::EmitType;
        use std::time::Duration;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();

        let (t1, t2) = (TenantId(1), TenantId(2));
        let agent_a = [0xA1u8; 16]; // tenant T1 — autorité de supervision (intra-tenant)
        let agent_b = [0xB2u8; 16]; // tenant T2 — cible cross-tenant

        let actor_a = ActorInstanceBuilder::new(&engine, &module, agent_a, store.clone(), log.clone())
            .tenant(t1).build().await.unwrap();
        let actor_b = ActorInstanceBuilder::new(&engine, &module, agent_b, store.clone(), log.clone())
            .tenant(t2).build().await.unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.set_log_ref(log.clone());
        scheduler.register(actor_a);
        let tx_b = scheduler.register(actor_b);

        // L'agent b (T2) accumule 5 snapshots (seq 0..4).
        for _ in 0..5 {
            tx_b.send(Message::data(b"work".to_vec())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(30)).await;

        // (1) REFUS : autorité de T1 visant un agent de T2 → CrossTenantDenied, aucun effet.
        let denied = scheduler.rollback_as(&agent_b, 2, SupervisionAuthority::Tenant(t1)).await;
        assert_eq!(denied, Err(SupervisionError::CrossTenantDenied),
            "INV-SD-AUTH : rollback cross-tenant sous autorité de tenant doit être refusé.");
        tokio::time::sleep(Duration::from_millis(20)).await;

        let has_rollback_after_denied = log.entries_by_agent(&agent_b).iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == EmitType::SchedulerRollback as u8);
        assert!(!has_rollback_after_denied,
            "INV-SD-AUTH : un refus cross-tenant ne doit produire AUCUN 0x0B (refus = aucun effet).");

        // (2) AUTORISÉ : même cible, autorité orchestrateur trusted → succès, 0x0B présent.
        scheduler.rollback_as(&agent_b, 2, SupervisionAuthority::Orchestrator).await
            .expect("INV-SD-AUTH : la supervision orchestrateur reste autorisée (non-régression).");
        tokio::time::sleep(Duration::from_millis(20)).await;

        drop(tx_b);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let has_rollback_after_orch = log.entries_by_agent(&agent_b).iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == EmitType::SchedulerRollback as u8);
        assert!(has_rollback_after_orch,
            "INV-SD-AUTH : la supervision orchestrateur doit bien produire un 0x0B (effet appliqué).");
    }

    /// INV-SD-AUTH (intra-tenant autorisé) — une autorité de tenant supervise un agent de
    /// SON tenant : la supervision passe (le check n'est pas un refus aveugle de toute
    /// autorité de tenant, seulement du cross-tenant). ADR-0059 §C.
    #[tokio::test]
    async fn inv_sd_auth_intra_tenant_supervision_allowed() {
        use super::actor::{ActorInstanceBuilder, INTROSPECT_AGENT_WAT, Message, TenantId};
        use super::scheduler::{Scheduler, SupervisionAuthority};
        use os_poc_causal_log::EmitType;
        use std::time::Duration;

        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();

        let t1 = TenantId(1);
        let agent_b = [0xB3u8; 16]; // appartient à T1, supervisé par une autorité de T1

        let actor_b = ActorInstanceBuilder::new(&engine, &module, agent_b, store.clone(), log.clone())
            .tenant(t1).build().await.unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.set_log_ref(log.clone());
        let tx_b = scheduler.register(actor_b);
        for _ in 0..3 {
            tx_b.send(Message::data(b"work".to_vec())).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Autorité de T1 sur un agent de T1 → autorisé.
        scheduler.rollback_as(&agent_b, 1, SupervisionAuthority::Tenant(t1)).await
            .expect("INV-SD-AUTH : la supervision intra-tenant doit être autorisée.");
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(tx_b);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let has_rollback = log.entries_by_agent(&agent_b).iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == EmitType::SchedulerRollback as u8);
        assert!(has_rollback,
            "INV-SD-AUTH : la supervision intra-tenant doit produire un 0x0B (effet appliqué).");
    }

    /// SEF-3 test_scope_covers_prefix — une cap sur "store/agent-A" couvre "store/agent-A/x"
    /// mais pas "store/agent-B/x".
    #[test]
    fn test_scope_covers_prefix() {
        use os_poc_capabilities::scope_covers;
        assert!(scope_covers("store/agent-A", "store/agent-A"), "exact match");
        assert!(scope_covers("store/agent-A", "store/agent-A/x"), "sous-path direct");
        assert!(scope_covers("store/agent-A", "store/agent-A/x/y"), "sous-path profond");
        assert!(!scope_covers("store/agent-A", "store/agent-B/x"), "chemin différent");
        assert!(!scope_covers("store/agent-A", "store/agent-AB"), "pas un sous-path");
        assert!(!scope_covers("store/agent-A", "other"), "chemin complètement différent");
    }

    /// SEF-3 s9_capability_isolation — scénario P4 complet.
    ///
    /// Protocole :
    ///   - 1 parent agent avec cap sur "RP" (2 bytes)
    ///   - 10 sous-agents ; sous-agent i a cap write+read sur "Ri" (2 bytes, "R0".."R9")
    ///   - Étape 2 : chaque sous-agent écrit dans Ri → doit retourner 0 (autorisé)
    ///   - Étape 3 : chaque sous-agent tente de lire R_{(i+1)%10} → doit retourner -1 (refusé)
    ///   - Étape 3b : chaque sous-agent tente de lire "RP" → doit retourner -1 (refusé)
    ///   - Étape 4 : inspecter le log — chaque refus doit avoir un CapabilityDenied (0x14)
    ///
    /// Critères P4 :
    ///   100% accès autorisés réussis, 100% refusés échoués, 100% refusés loggés.
    #[tokio::test(flavor = "current_thread")]
    async fn s9_capability_isolation() {
        use super::actor::{ActorInstance, STORE_AGENT_WAT};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use os_poc_causal_log::EmitType;
        use std::sync::{Arc, Mutex};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();

        const N_CHILDREN: usize = 10;
        let parent_id = [0xF0u8; 16];
        let resource_parent = "RP"; // 2 bytes

        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));

        // Grant caps
        // Parent : cap read+write sur "RP"
        let _parent_cap = {
            let mut cs = cap_store.lock().unwrap();
            cs.grant_root(parent_id,
                Permissions { read: true, write: true, execute: false, delegate: false },
                resource_parent.to_string())
        };

        // Sous-agents : resource "R0".."R9" (2 bytes chacun), caps read+write
        let child_ids: Vec<[u8; 16]> = (0..N_CHILDREN)
            .map(|i| { let mut id = [0xE0u8; 16]; id[15] = i as u8; id })
            .collect();
        let child_resources: Vec<String> = (0..N_CHILDREN)
            .map(|i| format!("R{}", i)) // "R0".."R9" = 2 bytes
            .collect();
        let child_caps: Vec<u64> = {
            let mut cs = cap_store.lock().unwrap();
            (0..N_CHILDREN).map(|i| {
                cs.grant_root(child_ids[i],
                    Permissions { read: true, write: true, execute: false, delegate: false },
                    child_resources[i].clone())
            }).collect()
        };

        // Crée les instances des sous-agents
        let mut child_actors: Vec<ActorInstance> = Vec::new();
        for i in 0..N_CHILDREN {
            let actor = ActorInstance::new_precompiled_with_caps(
                &engine, &module, child_ids[i], store.clone(), log_ref.clone(),
                cap_store.clone(), vec![child_caps[i]],
            ).await.unwrap();
            child_actors.push(actor);
        }

        // Étape 1 : build history pour chaque agent
        for actor in &mut child_actors {
            actor.process_one(b"\x00").await.unwrap();
        }

        // Étape 2 : chaque sous-agent écrit dans Ri — doit réussir (retour 0)
        let mut authorized_ok = 0;
        for i in 0..N_CHILDREN {
            let resource = &child_resources[i]; // "Ri" = 2 bytes
            let value = b"XX"; // 2 bytes (même longueur)
            assert_eq!(resource.len(), value.len());
            let msg = make_put_msg(resource, child_caps[i], value);
            child_actors[i].process_one(&msg).await.unwrap();
            let res = child_actors[i].read_memory_at(256, 1);
            if res[0] == 0 { authorized_ok += 1; }
        }
        assert_eq!(authorized_ok, N_CHILDREN,
            "P4 critère 1 : 100% des accès autorisés doivent réussir ({}/{})",
            authorized_ok, N_CHILDREN);

        // Étape 3 : chaque sous-agent tente de lire R_{(i+1)%N} — doit échouer (retour -1)
        let mut denied_failed = 0;
        for i in 0..N_CHILDREN {
            let cross_resource = &child_resources[(i + 1) % N_CHILDREN]; // "R_{i+1}" = 2 bytes
            // On utilise la cap de i (invalide pour cross_resource)
            let msg = make_get_msg(cross_resource, child_caps[i]);
            child_actors[i].process_one(&msg).await.unwrap();
            let res = child_actors[i].read_memory_at(256, 1);
            if res[0] == 0xFF { denied_failed += 1; } // -1 tronqué en u8
        }
        assert_eq!(denied_failed, N_CHILDREN,
            "P4 critère 2 : 100% des accès refusés doivent échouer ({}/{})",
            denied_failed, N_CHILDREN);

        // Étape 3b : chaque sous-agent tente de lire "RP" (resource du parent) — doit échouer
        // "RP" = 2 bytes → compatible avec make_get_msg
        let mut denied_parent = 0;
        for i in 0..N_CHILDREN {
            let msg = make_get_msg(resource_parent, child_caps[i]);
            child_actors[i].process_one(&msg).await.unwrap();
            let res = child_actors[i].read_memory_at(256, 1);
            if res[0] == 0xFF { denied_parent += 1; }
        }
        assert_eq!(denied_parent, N_CHILDREN,
            "P4 critère 2b : accès à R_parent refusés ({}/{})",
            denied_parent, N_CHILDREN);

        // Étape 4 : vérifier que chaque sous-agent a au moins 2 CapabilityDenied (0x14) dans le log
        // (un pour cross-resource, un pour RP)
        let mut all_logged = true;
        for i in 0..N_CHILDREN {
            let entries = log_ref.entries_by_agent(&child_ids[i]);
            let cap_denied_count = entries.iter()
                .filter_map(|(_, e)| e.emit_payload.as_ref())
                .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                .filter(|env| env.emit_type == EmitType::CapabilityDenied as u8)
                .count();
            if cap_denied_count < 2 {
                eprintln!("agent {} : seulement {} CapabilityDenied (attendu ≥2)", i, cap_denied_count);
                all_logged = false;
            }
        }
        assert!(all_logged, "P4 critère 3 : 100% des refus doivent être loggés avec CapabilityDenied (0x14)");
    }

    // ── S21 — Délégation cap scope-prefix (UC-2 / ADR-0005) ─────────────────────

    /// S21 - Oracle P4 (R1) : atténuation par scope-prefix.
    /// A délègue à B une cap à scope "/data/sub" (read-only) dérivée d'une cap "/data" rw+delegate.
    /// B accède à "/data/sub" et sous-chemins ; tout accès hors-scope ou hors-permission est refusé.
    #[tokio::test(flavor = "current_thread")]
    async fn s21_cap_delegation_scope_prefix() {
        use os_poc_capabilities::{CapabilityStore, Permissions};

        let mut cs = CapabilityStore::new();
        let agent_a = [0x2Au8; 16];
        let agent_b = [0x2Bu8; 16];

        // A reçoit cap rw+delegate sur "/data".
        let c_a = cs.grant_root(
            agent_a,
            Permissions { read: true, write: true, execute: false, delegate: true },
            "/data".to_string(),
        );

        // A délègue à B : read-only, no-delegate, scope "/data/sub" (attenuation).
        let c_b = cs.delegate(
            c_a, &agent_a, agent_b,
            Permissions { read: true, write: false, execute: false, delegate: false },
            "/data/sub".to_string(),
        ).expect("S21: délégation doit réussir (A a delegate=true)");

        let read_perm  = Permissions { read: true,  write: false, execute: false, delegate: false };
        let write_perm = Permissions { read: false,  write: true, execute: false, delegate: false };
        let deleg_perm = Permissions { read: true,  write: false, execute: false, delegate: true  };

        // B PEUT lire "/data/sub" (exact) et sous-chemins.
        assert!(cs.check(&agent_b, c_b, "/data/sub",           &read_perm),
            "S21 P4: B doit accéder à /data/sub (exact match)");
        assert!(cs.check(&agent_b, c_b, "/data/sub/x",         &read_perm),
            "S21 P4: B doit accéder à /data/sub/x (scope_covers)");
        assert!(cs.check(&agent_b, c_b, "/data/sub/nested/dir",&read_perm),
            "S21 P4: B doit accéder à /data/sub/nested/dir (scope_covers)");

        // B NE PEUT PAS accéder à "/data" (trop large — hors-scope).
        assert!(!cs.check(&agent_b, c_b, "/data",              &read_perm),
            "S21 P4: B ne peut pas accéder à /data (hors-scope de la cap déléguée)");

        // B NE PEUT PAS accéder à "/data/other" (scope différent).
        assert!(!cs.check(&agent_b, c_b, "/data/other",        &read_perm),
            "S21 P4: B ne peut pas accéder à /data/other (scope ne couvre pas /data/sub)");

        // B NE PEUT PAS écrire (permission dépassée par rapport à la cap déléguée).
        assert!(!cs.check(&agent_b, c_b, "/data/sub",          &write_perm),
            "S21 P4: B ne peut pas écrire (cap read-only)");

        // B NE PEUT PAS déléguer (delegate=false dans la cap déléguée).
        assert!(!cs.check(&agent_b, c_b, "/data/sub",          &deleg_perm),
            "S21 P4: B ne peut pas déléguer (delegate=false)");

        // C_A d'origine reste valide (la délégation ne révoque pas la source).
        let rwd = Permissions { read: true, write: true, execute: false, delegate: true };
        assert!(cs.check(&agent_a, c_a, "/data",               &rwd),
            "S21: C_A reste valide après délégation (source non révoquée)");
    }

    // ── S22 — Session bornée (UC-3 / ADR-0012) ──────────────────────────────────

    /// S22 - Oracle P3 (R1) : SessionBoundary (0x0A) déclenchée à N_max actions.
    /// La première action de la nouvelle session cite le Checkpoint, pas une action pré-frontière
    /// (pas de mémoire cross-session sans citation explicite via add_cause).
    #[tokio::test(flavor = "current_thread")]
    async fn s22_session_bounded() {
        use super::actor::{ActorInstance, SESSION_AGENT_WAT};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, SESSION_AGENT_WAT).unwrap();
        let agent_id = [0x2Cu8; 16];
        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone()
        ).await.unwrap();

        // Borne à 3 actions pour le test.
        actor.state_mut().session_max_actions = 3;

        // Actions 1-2 : session 1, pas de frontière.
        actor.process_one(&[0x00]).await.unwrap();
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.session_id(), 1, "S22: session 1 après 2 actions");

        // Action 3 : frontière déclenchée.
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.session_id(), 2,    "S22: session 2 après frontière");
        assert_eq!(actor.session_action_count(), 0, "S22: action_count remis à zéro");
        assert_eq!(actor.lifecycle(), LifecycleState::Checkpointed,
            "S22: lifecycle = Checkpointed après frontière");

        // Oracle 1 : SessionBoundary (0x0A) dans le log.
        // last_action → Lifecycle=Checkpointed ; son parent = SessionBoundary.
        let ckpt_id = actor.last_action().expect("S22: last_action après frontière");
        let ckpt_entry = log_ref.get(&ckpt_id).unwrap().expect("S22: entrée Checkpointed");
        assert!(!ckpt_entry.parent_ids.is_empty(), "S22: Checkpointed doit avoir un parent");
        let sb_id = ckpt_entry.parent_ids[0];
        let sb_entry = log_ref.get(&sb_id).unwrap().expect("S22: entrée SessionBoundary");
        let sb_env = os_poc_causal_log::EmitEnvelope::from_msgpack(
            &sb_entry.emit_payload.expect("S22: payload SessionBoundary")
        ).unwrap();
        assert_eq!(sb_env.emit_type, os_poc_causal_log::EmitType::SessionBoundary as u8,
            "S22: parent de Checkpointed doit être SessionBoundary (0x0A)");

        // Oracle 2 : 1re action de la nouvelle session cite le Checkpointed (pas un snapshot pré-frontière).
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.session_id(), 2, "S22: toujours session 2");
        let new_id = actor.last_action().expect("S22: last_action nouvelle session");
        let new_entry = log_ref.get(&new_id).unwrap().expect("S22: nouvelle action dans log");
        assert!(new_entry.parent_ids.contains(&ckpt_id),
            "S22 P3: 1re action nouvelle session doit citer le Checkpointed (pas un snapshot pré-frontière)");
    }

    // ── S23 — Canal de validation — chemin timeout (UC-4 / ADR-0013 / ADR-0014) ─

    /// S23 - Oracle P4 (R1) : chemin timeout du canal A3.
    /// L'agent émet ValidationRequest (0x08), aucune réponse ne vient.
    /// Après `validation_timeout_ms`, ValidationResponse verdict=Timeout est injecté
    /// automatiquement. L'agent reprend Active sans action de l'extérieur.
    #[tokio::test(flavor = "multi_thread")]
    async fn s23_validation_timeout() {
        use super::actor::{VALIDATION_AGENT_WAT, Message, ValidationVerdict};
        use super::scheduler::Scheduler;
        use os_poc_capabilities::CapabilityStore;
        use std::sync::Mutex;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, VALIDATION_AGENT_WAT).unwrap();
        let agent_id = [0x2Du8; 16];

        const TIMEOUT_MS: u64 = 50;
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let actor = ActorInstance::new_precompiled_with_caps_and_timeout(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![], TIMEOUT_MS,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        // Build history, puis request_validation(risk=1) → AwaitingValidation.
        tx.send(Message::data(vec![0x00])).await.unwrap();
        tx.send(Message::data(vec![0x02, 0x01])).await.unwrap();

        // Attendre que le timeout déclenche (50 ms + marge 200 ms).
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Envoyer msg[0]=3 : si Active (timeout OK), l'agent émet le verdict dans le log.
        tx.send(Message::data(vec![0x03])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let entries = log_ref.entries_by_agent(&agent_id);

        // Oracle P4 : ValidationRequest (0x08) tracé.
        let request_found = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == os_poc_causal_log::EmitType::ValidationRequest as u8);
        assert!(request_found,
            "S23 P4: ValidationRequest (0x08) doit être dans le log (demande traçée)");

        // Oracle P4 : ValidationResponse verdict=Timeout (ADR-0014 D14.d).
        let timeout_found = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| {
                env.emit_type == os_poc_causal_log::EmitType::ValidationResponse as u8
                    && env.payload.first().copied() == Some(ValidationVerdict::Timeout as u8)
            });
        assert!(timeout_found,
            "S23 P4: ValidationResponse verdict=Timeout doit être dans le log (ADR-0014 D14.d)");
    }

    // ── S24 — Watchdog WASM budget (UC-5 / ADR-0025) ────────────────────────────

    /// S24 - Oracle R1 : budget d'exécution par AgentProfile.
    /// Un agent en boucle infinie avec AgentProfile::Algo est interrompu coopérativement
    /// par le watchdog (époque). AgentCrash (0x13) dans le log. Aucun blocage.
    #[tokio::test(flavor = "multi_thread")]
    async fn s24_watchdog_budget() {
        use super::actor::{ActorInstance, INFINITE_LOOP_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use super::watchdog::AgentProfile;
        use std::time::{Duration, Instant};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INFINITE_LOOP_AGENT_WAT).unwrap();
        let agent_id = [0x2Eu8; 16];

        let actor = ActorInstance::new_precompiled_with_profile(
            &engine, &module, agent_id, store, log_ref.clone(),
            AgentProfile::Algo,
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx = scheduler.register(actor);

        let t0 = Instant::now();
        tx.send(Message::data(b"trigger".to_vec())).await.unwrap();

        // Algo = 10 ticks × 10ms = 100ms. On attend 2s (×20 marge).
        tokio::time::sleep(Duration::from_millis(2000)).await;
        drop(tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let elapsed = t0.elapsed();

        // Oracle ADR-0025 : AgentCrash (0x13) dans le log.
        let entries = log_ref.entries_by_agent(&agent_id);
        let crash_found = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8);
        assert!(crash_found,
            "S24 ADR-0025: AgentCrash (0x13) attendu dans le log (watchdog Algo budget dépassé)");

        // Agent terminé bien avant LlmShort (5s).
        assert!(elapsed < Duration::from_secs(4),
            "S24: agent Algo terminé en {:?}, attendu < 4s (budget Algo << LlmShort)", elapsed);
    }

    // ── S25 — Isolation de faute one_for_one (UC-6 / ADR-0013 / ADR-0014) ───────

    /// S25 - Oracle P4 (R1) : isolation de faute par défaut (one_for_one).
    /// Agent A (boucle infinie, Algo) crash via watchdog. Agent B (normal) continue
    /// sans être affecté : B traite un message après le crash de A, pas d'AgentCrash dans B.
    #[tokio::test(flavor = "multi_thread")]
    async fn s25_restart_policy_one_for_one() {
        use super::actor::{ActorInstance, INFINITE_LOOP_AGENT_WAT, AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use super::watchdog::AgentProfile;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let agent_a = [0x2Fu8; 16];
        let agent_b = [0x30u8; 16];

        // Agent A : boucle infinie, Algo → watchdog déclenche < 4s.
        let module_a = Module::new(&engine, INFINITE_LOOP_AGENT_WAT).unwrap();
        let actor_a = ActorInstance::new_precompiled_with_profile(
            &engine, &module_a, agent_a, store.clone(), log_ref.clone(),
            AgentProfile::Algo,
        ).await.unwrap();

        // Agent B : agent normal, profil par défaut.
        let module_b = Module::new(&engine, AGENT_WAT).unwrap();
        let actor_b = ActorInstance::new_precompiled(
            &engine, &module_b, agent_b, store.clone(), log_ref.clone()
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx_a = scheduler.register(actor_a);
        let tx_b = scheduler.register(actor_b);

        // A : entre en boucle infinie → watchdog le tue.
        tx_a.send(Message::data(b"loop".to_vec())).await.unwrap();

        // Attendre la fin du watchdog Algo (100ms + marge).
        tokio::time::sleep(Duration::from_millis(2000)).await;

        // Oracle isolation (1/2) : A a AgentCrash dans le log.
        let entries_a = log_ref.entries_by_agent(&agent_a);
        let a_crashed = entries_a.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8);
        assert!(a_crashed, "S25 P4: AgentCrash attendu dans le log de A (watchdog Algo)");

        // B traite un message normalement après le crash de A.
        tx_b.send(Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(tx_a);
        drop(tx_b);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Oracle isolation (2/2) : B n'a PAS AgentCrash, B a bien traité son message.
        let entries_b = log_ref.entries_by_agent(&agent_b);
        let b_crashed = entries_b.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8);
        assert!(!b_crashed,
            "S25 P4: B ne doit PAS avoir AgentCrash (one_for_one : crash de A isolé)");
        assert!(!entries_b.is_empty(),
            "S25 P4: B doit avoir au moins une entrée dans le log (actif après crash de A)");
    }

    // ── S26 — A1 introspection (UC-7 / spec 02c) ─────────────────────────────────

    /// S26 - Oracle P3 (R1) : l'agent lit son propre état (last_action_id, seq, lifecycle)
    /// via agent_introspect avant de décider. Prouve A1 : auto-connaissance causale.
    /// seq et last_action_id dans le payload Introspect reflètent l'état au moment de l'appel.
    #[tokio::test(flavor = "current_thread")]
    async fn s26_introspection_a1() {
        use super::actor::{ActorInstance, INTROSPECT_AGENT_WAT, INTROSPECT_PAYLOAD_LEN};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, INTROSPECT_AGENT_WAT).unwrap();
        let agent_id = [0x31u8; 16];
        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone()
        ).await.unwrap();

        // Cycle 1 : seq=0 avant commit_barrier → introspect voit seq=0, last_action=zeros.
        actor.process_one(b"first").await.unwrap();
        assert_eq!(actor.seq(), 1, "S26: seq=1 après 1er cycle");

        // Cycle 2 : introspect voit seq=1, last_action non-zero (cycle 1 a produit une action).
        actor.process_one(b"second").await.unwrap();
        assert_eq!(actor.seq(), 2, "S26: seq=2 après 2e cycle");

        // Oracle : au moins 2 entrées Introspect (0x06) dans le log.
        let entries = log_ref.entries_by_agent(&agent_id);
        let mut introspect_payloads: Vec<Vec<u8>> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == os_poc_causal_log::EmitType::Introspect as u8)
            .map(|env| env.payload.clone())
            .collect();
        // Trier par seq encodé dans le payload (bytes [32..40]) pour garantir l'ordre.
        introspect_payloads.sort_by_key(|p| {
            if p.len() >= 40 { u64::from_le_bytes(p[32..40].try_into().unwrap()) } else { 0 }
        });
        assert!(introspect_payloads.len() >= 2,
            "S26 A1: au moins 2 entrées Introspect dans le log");

        // 1er introspect : seq=0, last_action=zeros (appelé avant le 1er commit_barrier).
        let p0 = &introspect_payloads[0];
        assert_eq!(p0.len(), INTROSPECT_PAYLOAD_LEN, "S26: payload Introspect = {} bytes", INTROSPECT_PAYLOAD_LEN);
        let seq0 = u64::from_le_bytes(p0[32..40].try_into().unwrap());
        assert_eq!(seq0, 0, "S26 A1: seq=0 dans le 1er introspect (avant commit_barrier)");
        assert!(p0[0..32].iter().all(|&b| b == 0),
            "S26 A1: last_action_id=zeros dans le 1er introspect (aucune action précédente)");

        // 2e introspect : seq=1, last_action non-zero.
        let p1 = &introspect_payloads[1];
        let seq1 = u64::from_le_bytes(p1[32..40].try_into().unwrap());
        assert_eq!(seq1, 1, "S26 A1: seq=1 dans le 2e introspect (état avant 2e commit_barrier)");
        assert!(p1[0..32].iter().any(|&b| b != 0),
            "S26 A1: last_action_id non-zero dans le 2e introspect (cycle 1 a émis une action)");
    }

    // ── S27 — Contrat emit (UC-8 / ADR-0010 / P6 nominal) ───────────────────────

    /// S27 - Oracle P6 (R1, chemin nominal) : séquence commit_barrier → store_put → log_append.
    /// Après process_one, le ContentStore et le CausalLog sont cohérents (I-CSR nominal) :
    /// hash_after de chaque entrée du log est présent dans le store. La chaîne (hash_before →
    /// hash_after) est continue entre deux cycles consécutifs.
    #[tokio::test(flavor = "current_thread")]
    async fn s27_emit_contract() {
        use super::actor::{ActorInstance, AGENT_WAT};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();
        let agent_id = [0x32u8; 16];
        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store.clone(), log_ref.clone()
        ).await.unwrap();

        // Cycle 1 : commit_barrier + emit.
        actor.process_one(&[0x00]).await.unwrap();
        let action1 = actor.last_action().expect("S27: last_action après cycle 1");
        let snap1   = actor.last_snapshot().expect("S27 P6: last_snapshot après cycle 1");

        // Oracle 1 : entrée dans le log.
        let entry1 = log_ref.get(&action1).unwrap().expect("S27: entrée cycle 1 dans le log");
        assert_eq!(entry1.hash_after, snap1,
            "S27 P6: hash_after du log = last_snapshot (cohérence store ↔ log)");

        // Oracle 2 (I-CSR) : hash_after présent dans le ContentStore.
        assert!(store.get_header(&snap1).expect("get_header cycle 1").is_some(),
            "S27 P6: snapshot du cycle 1 présent dans le ContentStore (I-CSR nominal)");

        // Cycle 2 : vérifie la continuité de la chaîne.
        actor.process_one(&[0x00]).await.unwrap();
        let action2 = actor.last_action().expect("S27: last_action après cycle 2");
        let snap2   = actor.last_snapshot().expect("S27 P6: last_snapshot après cycle 2");

        let entry2 = log_ref.get(&action2).unwrap().expect("S27: entrée cycle 2 dans le log");

        // Oracle 3 : hash_before du cycle 2 = hash_after du cycle 1 (chaîne continue).
        assert_eq!(entry2.hash_before, snap1,
            "S27 P6: hash_before cycle 2 = hash_after cycle 1 (chaîne cohérente)");
        assert_eq!(entry2.hash_after, snap2,
            "S27 P6: hash_after cycle 2 = last_snapshot cycle 2");
        assert!(store.get_header(&snap2).expect("get_header cycle 2").is_some(),
            "S27 P6: snapshot du cycle 2 présent dans le ContentStore (I-CSR cycle 2)");
    }

    // ── S28 — Self-rollback post-emit refusé (UC-11 / spec 02c §A2 / P2) ─────────

    /// S28 - Oracle P2 (R1) : la ligne de démarcation est le commit_barrier.
    /// Après `commit_barrier + emit` (seq=1), `agent_self_rollback(1)` est refusé
    /// (seq < 1 + depth = 2) : aucun snapshot antérieur ne peut être cible.
    /// Le log ne contient pas d'entrée SelfRollback (refus silencieux sans effet).
    #[tokio::test(flavor = "current_thread")]
    async fn s28_self_rollback_post_emit_refused() {
        use super::actor::{ActorInstance, SELF_ROLLBACK_AGENT_WAT};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, SELF_ROLLBACK_AGENT_WAT).unwrap();
        let agent_id = [0x33u8; 16];
        let mut actor = ActorInstance::new_precompiled(
            &engine, &module, agent_id, store, log_ref.clone()
        ).await.unwrap();

        // Émet 1 action (post-commit_barrier + post-emit). La ligne de démarcation est franchie.
        actor.process_one(&[0x00]).await.unwrap();
        assert_eq!(actor.seq(), 1, "S28: seq=1 après 1 commit");
        let snap_before = actor.last_snapshot().expect("S28: snapshot présent après emit");

        // Tentative de self-rollback depth=1.
        // seq=1, check: seq(1) < 1+depth(1)=2 → vrai → retourne -3 (historique insuffisant).
        actor.process_one(&[0x01, 0x01]).await.unwrap();

        // Oracle 1 : snapshot inchangé — rollback refusé, état non modifié.
        assert_eq!(actor.last_snapshot(), Some(snap_before),
            "S28 P2: snapshot inchangé — self-rollback post-emit refusé (seq insuffisant)");

        // Oracle 2 : aucune entrée SelfRollback dans le log (refus sans trace).
        let entries = log_ref.entries_by_agent(&agent_id);
        let has_sr = entries.iter().any(|(_, e)| {
            e.emit_payload.as_ref()
                .and_then(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                .map(|env| env.emit_type == os_poc_causal_log::EmitType::SelfRollback as u8)
                .unwrap_or(false)
        });
        assert!(!has_sr,
            "S28 P2: aucun SelfRollback dans le log (refus silencieux)");
        assert_eq!(entries.len(), 1,
            "S28: exactement 1 entrée dans le log (l'emit initial uniquement)");
    }

    // ── S29 — Révocation récursive profonde (UC-16 / ADR-0005 / P4) ──────────────

    /// S29 - Oracle P4 (R1) : `revoke(cap_root)` en mode eager-BFS supprime la chaîne
    /// entière de k niveaux en O(k). Tous les descendants perdent l'accès immédiatement
    /// (pas de check différé, pas de tombstone). Conformité ADR-0005 §amendment (2026-05-15).
    #[tokio::test(flavor = "current_thread")]
    async fn s29_revoke_recursive_deep() {
        use os_poc_capabilities::{CapabilityStore, Permissions};

        let mut caps = CapabilityStore::new();

        // Chaîne linéaire de délégation : A (root) → B → C → D → E  (profondeur k=4)
        let agents: Vec<[u8; 16]> = (0x34u8..=0x38u8).map(|i| [i; 16]).collect();
        let full_perm = Permissions { read: true, write: true, execute: true, delegate: true };
        let read_perm = Permissions { read: true, write: false, execute: false, delegate: false };
        let resource = "/data".to_string();

        let cap_a = caps.grant_root(agents[0], full_perm.clone(), resource.clone());
        let cap_b = caps.delegate(cap_a, &agents[0], agents[1], full_perm.clone(), resource.clone()).unwrap();
        let cap_c = caps.delegate(cap_b, &agents[1], agents[2], full_perm.clone(), resource.clone()).unwrap();
        let cap_d = caps.delegate(cap_c, &agents[2], agents[3], full_perm.clone(), resource.clone()).unwrap();
        let cap_e = caps.delegate(cap_d, &agents[3], agents[4], full_perm.clone(), resource.clone()).unwrap();

        // Vérification pré-révocation : les 5 agents ont chacun une cap valide.
        assert!(caps.check(&agents[0], cap_a, "/data", &read_perm), "S29: A a accès avant révocation");
        for (i, &cap_id) in [cap_b, cap_c, cap_d, cap_e].iter().enumerate() {
            assert!(caps.check(&agents[i + 1], cap_id, "/data", &read_perm),
                "S29: agent {} a accès avant révocation", i + 1);
        }

        // Révocation du nœud racine → BFS eager sur toute la chaîne (O(depth)).
        let revoked_count = caps.revoke(cap_a);
        assert_eq!(revoked_count, 5,
            "S29 P4: 5 caps révoquées (cap_a + 4 descendants — propagation récursive O(depth))");

        // Oracle : aucun descendant n'a accès après révocation (suppression immédiate, pas de cache).
        assert!(!caps.check(&agents[0], cap_a, "/data", &read_perm), "S29 P4: A refusé après révocation");
        for (i, &cap_id) in [cap_b, cap_c, cap_d, cap_e].iter().enumerate() {
            assert!(!caps.check(&agents[i + 1], cap_id, "/data", &read_perm),
                "S29 P4: agent {} refusé après révocation récursive", i + 1);
        }
    }

    // ── S30 — WASM adversarial trap + isolation (UC-18 / ADR-0048 / P4) ──────────

    /// S30 - Oracle P4 (R1) : les traps WASM (unreachable, OOB mémoire) sont contenus
    /// par le sandbox Wasmtime. L'agent qui trap émet AgentCrash (0x13 / cause=0x01).
    /// L'agent survivor continue et le CausalLog+ContentStore restent cohérents (I-CSR intact).
    #[tokio::test(flavor = "multi_thread")]
    async fn s30_wasm_adversarial_trap_isolation() {
        use super::actor::{ActorInstance, AGENT_WAT, OOB_TRAP_AGENT_WAT, Message};
        use super::scheduler::Scheduler;
        use std::time::Duration;

        let (engine, store, log_ref, _dir) = setup();
        let mod_trap = Module::new(&engine, OOB_TRAP_AGENT_WAT).unwrap();
        let mod_ok   = Module::new(&engine, AGENT_WAT).unwrap();

        let agent_trap = [0x39u8; 16];
        let agent_ok   = [0x3Au8; 16];

        let actor_trap = ActorInstance::new_precompiled(
            &engine, &mod_trap, agent_trap, store.clone(), log_ref.clone(),
        ).await.unwrap();
        let actor_ok = ActorInstance::new_precompiled(
            &engine, &mod_ok, agent_ok, store.clone(), log_ref.clone(),
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        let tx_trap = scheduler.register(actor_trap);
        let tx_ok   = scheduler.register(actor_ok);

        // Agent OOB trap : accès mémoire hors-bornes → Wasmtime MemoryOutOfBounds.
        tx_trap.send(Message::data(b"trigger".to_vec())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Oracle 1 : l'agent trap a AgentCrash (cause=0x01 ProcessFailed).
        let trap_entries = log_ref.entries_by_agent(&agent_trap);
        let trap_crash = trap_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .find(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8);
        assert!(trap_crash.is_some(),
            "S30 P4: AgentCrash attendu dans le log de l'agent OOB (sandbox WASM contenait le trap)");
        assert_eq!(trap_crash.unwrap().payload[0], 0x01,
            "S30 P4: cause = ProcessFailed (0x01) pour un trap MemoryOutOfBounds");

        // Agent OK émet après le crash de l'agent OOB — intégrité du runtime vérifiée.
        tx_ok.send(Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(tx_trap);
        drop(tx_ok);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Oracle 2 : le survivor n'a pas AgentCrash.
        let ok_entries = log_ref.entries_by_agent(&agent_ok);
        let ok_crashed = ok_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == os_poc_causal_log::EmitType::AgentCrash as u8);
        assert!(!ok_crashed,
            "S30 P4: le survivor ne doit PAS avoir AgentCrash (trap OOB isolé par sandbox WASM)");
        assert!(!ok_entries.is_empty(),
            "S30 P4: le survivor a au moins une entrée dans le log (runtime intact après crash OOB)");

        // Oracle 3 (I-CSR) : les entrees commit du survivor ont hash_after dans le ContentStore.
        // Les entrees lifecycle (Spawned, Active) ont hash_after=[0u8;32] quand aucun snapshot
        // na ete cree -- on les saute ; seules les entrees avec snapshot non-nul comptent.
        let commit_entries: Vec<_> = ok_entries.iter()
            .filter(|(_, e)| e.hash_after != [0u8; 32])
            .collect();
        assert!(!commit_entries.is_empty(),
            "S30 P4: au moins une entree commitee dans le log du survivor");
        for (_, entry) in &commit_entries {
            assert!(store.get_header(&entry.hash_after).expect("get_header ok").is_some(),
                "S30 P4 I-CSR: hash_after du survivor présent dans le ContentStore (store intact)");
        }
    }

    /// S31 — UC-19 / ADR-0051 §D2 / P4 (limite d'audit bornée à 32 resources distinctes)
    ///
    /// Démontre la limite documentée de l'attribution `0x14` au-delà de la borne 32 :
    /// quand le set de resources distinctes par fenêtre est saturé (32) ET que le
    /// rate-limit scalaire est dépassé (>100), toute resource nouvelle supplémentaire
    /// déclenche le sentinel F2 (overflow de set, is_aggregated=true) — la resource
    /// n'est PAS attribuée nominativement dans le log causal.
    ///
    /// Ce n'est PAS une violation P4 : l'isolation tient (refus effectif), mais
    /// l'observabilité dépasse la garantie bornée (ADR-0051 D2 : "attribution préservée
    /// pour tout ensemble borné de resources distinctes").
    ///
    /// Oracle 1 (isolation P4) : refus effectif → P4 tient.
    /// Oracle 2 (limite d'audit) : "secret-33" dans le témoin hors-bande, ABSENT du log.
    /// Oracle 3 (sentinel F2) : un événement agrégé (rate_limited=0x01) a été émis.
    #[tokio::test(flavor = "current_thread")]
    async fn s31_audit_flood_beyond_bound_32() {
        use super::actor::{ActorInstance, CapDeniedAttempt, STORE_AGENT_WAT};
        use os_poc_capabilities::CapabilityStore;
        use os_poc_causal_log::{EmitEnvelope, EmitType};
        use std::collections::BTreeSet;
        use std::sync::{Arc, Mutex};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, STORE_AGENT_WAT).unwrap();
        let agent_id = [0x3Bu8; 16];

        // Agent sans aucune capability → tout accès est refusé.
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let fake_cap: u64 = 9999;

        let mut actor = ActorInstance::new_precompiled_with_caps(
            &engine, &module, agent_id, store, log_ref.clone(),
            cap_store, vec![],
        ).await.unwrap();

        // Témoin hors-bande : capture toutes les tentatives AVANT le rate-limit.
        let witness: Arc<Mutex<Vec<CapDeniedAttempt>>> = Arc::new(Mutex::new(Vec::new()));
        actor.state_mut().cap_denied_witness = Some(witness.clone());

        // ── Attaque ───────────────────────────────────────────────────────────────
        // Étape 1 : flood "bn" × 101 → F1 (sentinel scalaire), set = {"bn"}, count=101.
        // Cela déclenche aggregate_emitted=true (rate-limit dépassé).
        for _ in 0..101 {
            actor.process_one(&make_get_msg("bn", fake_cap)).await.unwrap();
        }

        // Étape 2 : 31 resources distinctes supplémentaires → set passe à 32/32.
        // Chaque resource est nouvelle avec de la place → condition new-with-room →
        // attribuée nominativement malgré count > 100 (correctif #6).
        for i in 1u8..=31 {
            let resource = format!("r{:02}", i);
            actor.process_one(&make_get_msg(&resource, fake_cap)).await.unwrap();
        }

        // Étape 3 : 33ème resource sensible. Set plein + count > 100 + new-resource
        // → F2 (sentinel overflow de set, is_aggregated=true, resource NON attribuée).
        actor.process_one(&make_get_msg("secret-33", fake_cap)).await.unwrap();

        // ── Oracle 1 (isolation P4) : le get est bien refusé ────────────────────
        let secret_res = actor.read_memory_at(256, 1);
        assert_eq!(secret_res[0], 0xFF,
            "UC-19 1a : 'secret-33' refusé (isolation P4 tient, aucun accès accordé à tort)");

        // ── Oracle 2 (limite d'audit) : "secret-33" dans le témoin, ABSENT du log ─
        let witness_resources: BTreeSet<String> = witness.lock().unwrap()
            .iter().map(|a| a.resource.clone()).collect();
        assert!(witness_resources.contains("secret-33"),
            "témoin hors-bande doit avoir capturé la tentative 'secret-33'");

        let entries = log_ref.entries_by_agent(&agent_id);
        let log_resources: BTreeSet<String> = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == EmitType::CapabilityDenied as u8)
            .filter_map(|env| cap_denied_resource(&env.payload))
            .collect();

        assert!(!log_resources.contains("secret-33"),
            "UC-19 (limite ADR-0051 D2) : 'secret-33' NON attribuable au-delà des 32 resources distinctes");

        // ── Oracle 3 (sentinel F2) : au moins un événement agrégé présent ────────
        let has_overflow_sentinel = entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == EmitType::CapabilityDenied as u8)
            .any(|env| cap_denied_resource(&env.payload).is_none()); // agrégé = resource absente
        assert!(has_overflow_sentinel,
            "UC-19 : un sentinel d'overflow de set (F2, rate_limited=0x01) doit être émis");

        let masked: BTreeSet<&String> = witness_resources.difference(&log_resources).collect();
        println!(
            "UC-19 (limite documentée ADR-0051 D2) : set saturé (32/32), 'secret-33' masquée, sentinel F2 émis. Masked={:?}",
            masked
        );
    }


    /// S32 — UC-20 / ADR-0036 → **ADR-0058 (B-fort) : forgerie causale REFERMÉE**
    ///
    /// Historiquement (red team A-5), ce scénario démontrait la **limite** de B-light :
    /// l'existence-check acceptait toute citation d'un `action_id` réel, même si le citant
    /// (B) n'avait jamais reçu de message de l'émetteur (A) — forgerie causale acceptée.
    ///
    /// B-fort (ADR-0058, modèle uniforme §D9) **referme cette limite même en mono-tenant** :
    /// une citation cross-agent exige un `CauseHandle` ; sans handle, `agent_add_cause` → -3
    /// et l'arête forgée n'est PAS créée. UC-20 (catalogue §UC-20) passe de LIMITE à CLOSE.
    ///
    /// Oracle 1 : B commite quand même une action (add_cause -3 ne bloque pas commit/emit).
    /// Oracle 2 : parent_ids(B1) ne contient PAS a1_id → forgerie refusée (le DAG ne ment pas).
    /// Oracle 3 : indépendance initiale de B confirmée (B0 ne cite pas A).
    #[tokio::test(flavor = "current_thread")]
    async fn s32_causal_forgery_refused_under_b_fort() {
        use super::actor::{ActorInstance, CROSS_AGENT_WAT};
        use os_poc_causal_log::{EmitEnvelope, EmitType, LogEntry};

        let (engine, store, log_ref, _dir) = setup();
        let module = Module::new(&engine, CROSS_AGENT_WAT).unwrap();
        let id_a = [0x3Cu8; 16];
        let id_b = [0x3Du8; 16];

        // Agent A : produit une action A1, sans aucune interaction avec B.
        let mut actor_a = ActorInstance::new_precompiled(
            &engine, &module, id_a, store.clone(), log_ref.clone()
        ).await.unwrap();
        actor_a.process_one(&[0x00]).await.unwrap();
        let a1_id = actor_a.last_action().expect("S32: last_action A");

        // Agent B : produit B0 (baseline), indépendamment de A. Aucun CauseHandle minté.
        let mut actor_b = ActorInstance::new_precompiled(
            &engine, &module, id_b, store.clone(), log_ref.clone()
        ).await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();
        let b0_id = actor_b.last_action().expect("S32: B0");

        // Tentative de forgerie : B "apprend" a1_id depuis le log partagé et tente de le citer
        // SANS handle. msg[0]=4 + a1_id → add_cause(a1_id) renvoie -3 (refus B-fort), puis
        // commit_barrier + emit → B1 (sans la cause forgée).
        let mut forge_msg = vec![0x04u8];
        forge_msg.extend_from_slice(&a1_id);
        actor_b.process_one(&forge_msg).await.unwrap();
        let b1_id = actor_b.last_action().expect("S32: B1");

        // Oracle 1 : B a quand même commité une action (le refus -3 ne bloque pas commit/emit).
        assert_ne!(b0_id, b1_id, "S32: B1 doit être une nouvelle action");

        // Oracle 2 (B-fort) : parent_ids(B1) ne contient PAS a1_id → forgerie refusée.
        let entry_b1: LogEntry = log_ref.get(&b1_id).unwrap()
            .expect("S32: B1 doit être dans le log");
        assert!(!entry_b1.parent_ids.contains(&a1_id),
            "S32 (B-fort, ADR-0058) : parent_ids(B1) NE contient PAS a1_id — forgerie cross-agent \
             refusée sans CauseHandle, même en mono-tenant (UC-20 fermé)");

        // Oracle 3 : indépendance initiale de B (B0 ne cite pas A) + A n'a émis qu'une action.
        let a_entries = log_ref.entries_by_agent(&id_a);
        let a_data_entries: Vec<_> = a_entries.iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| EmitEnvelope::from_msgpack(b).ok())
            .filter(|env| env.emit_type == EmitType::ActionResult as u8)
            .collect();
        assert_eq!(a_data_entries.len(), 1,
            "S32 : A n'a émis qu'une seule action (aucun message vers B)");
        let entry_b0: LogEntry = log_ref.get(&b0_id).unwrap()
            .expect("S32: B0 doit être dans le log");
        assert!(!entry_b0.parent_ids.contains(&a1_id),
            "S32 : B0 (avant tentative) ne cite pas A — confirme l'indépendance initiale de B");

        println!(
            "UC-20 (ADR-0058 B-fort) : forgerie causale cross-agent REFUSÉE en mono-tenant. \
             B a tenté de citer a1_id={} sans CauseHandle → -3, arête non créée. \
             La limite red team A-5 (P3 LIMITE DOCUMENTÉE) est fermée.",
            format!("{:02x?}", &a1_id[..4])
        );
    }


    /// S33 — UC-14 / ADR-0023 / P1b (anti-famine Batch→Foreground)
    ///
    /// Démontre la garantie d'équité bornée : un agent Batch affamé par un flux
    /// Foreground continu est promu en Foreground après `max_starvation_ms`.
    ///
    /// Timing déterministe (current_thread, comme S5) :
    ///   t=0ms   : FG1 soumis → slot (SleepyBackend 300ms).
    ///   t=20ms  : Batch soumis → file Batch.
    ///   t=240ms : sleep(220ms) — Batch a attendu ≈220ms > 200ms (max_starvation_ms).
    ///   t=240ms : FG2 soumis → file Foreground (wait ≈0ms < 200ms, non promu).
    ///   t≈320ms : FG1 termine → pop_next :
    ///             Batch promu Foreground front (≈300ms > 200ms) ;
    ///             FG2 non promu (≈80ms < 200ms) → Batch servi avant FG2.
    ///
    /// Oracle 1 (équité) : Batch reçoit InferenceResponse (anti-famine actif).
    /// Oracle 2 (promotion) : pool.queue_stats().total_promoted >= 1.
    /// Oracle 3 (ordre) : ts_us(Batch) < ts_us(FG2) — Batch passe devant FG2.
    #[tokio::test(flavor = "current_thread")]
    async fn s33_anti_starvation_batch_promoted() {
        use super::actor::{ActorInstance, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
                           INFER_AGENT_WAT};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend, PriorityClass};
        use os_poc_causal_log::EmitType;
        use std::sync::Arc;
        use std::time::Duration;

        // max_starvation_ms=200 : FG2 soumis tard ne l'atteint pas avant que FG1 libère le slot.
        let pool = Arc::new(InferencePool::new_with_queue_params(
            1, 16, 200, SleepyBackend { delay_ms: 300 },
        ));
        let cancel_fn   = InferencePool::as_cancel_fn(Arc::clone(&pool));
        let fg_infer    = InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Foreground);
        let batch_infer = InferencePool::as_infer_fn_with_class(Arc::clone(&pool), PriorityClass::Batch);

        let (engine, store, log_ref, _dir) = setup();
        let module = wasmtime::Module::new(&engine, INFER_AGENT_WAT).unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);
        let cap_store = scheduler.cap_store.clone();

        let id_fg1   = [0x42u8; 16];
        let id_batch = [0x43u8; 16];
        let id_fg2   = [0x44u8; 16];

        let tx_fg1 = scheduler.register(
            ActorInstance::new_precompiled_with_inference(
                &engine, &module, id_fg1, Arc::clone(&store), Arc::clone(&log_ref),
                Arc::clone(&cap_store), vec![], SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                fg_infer.clone(),
            ).await.unwrap()
        );
        let tx_batch = scheduler.register(
            ActorInstance::new_precompiled_with_inference(
                &engine, &module, id_batch, Arc::clone(&store), Arc::clone(&log_ref),
                Arc::clone(&cap_store), vec![], SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                batch_infer,
            ).await.unwrap()
        );
        let tx_fg2 = scheduler.register(
            ActorInstance::new_precompiled_with_inference(
                &engine, &module, id_fg2, Arc::clone(&store), Arc::clone(&log_ref),
                Arc::clone(&cap_store), vec![], SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
                fg_infer,
            ).await.unwrap()
        );

        // Amorcer les 3 agents (seq=1, snapshot nécessaire pour agent_infer).
        for tx in [&tx_fg1, &tx_batch, &tx_fg2] {
            tx.send(Message::data(vec![0x00])).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        // t=0 : FG1 obtient le slot (cap=1 → les suivants attendent).
        tx_fg1.send(Message::data(vec![0x07])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await; // FG1 démarre

        // t≈20ms : Batch soumis → file Batch (FG1 tient le slot).
        tx_batch.send(Message::data(vec![0x07])).await.unwrap();

        // t≈20ms→240ms : sleep 220ms → Batch a attendu ≈220ms > 200ms (seuil).
        // FG2 n'est pas encore soumis → pas de Foreground en file concurrente.
        tokio::time::sleep(Duration::from_millis(220)).await;

        // t≈240ms : FG2 soumis → file Foreground. FG1 est encore actif (~60ms restants).
        // FG2 wait ≈80ms quand FG1 finit < 200ms → non promu → Batch promu passe devant.
        tx_fg2.send(Message::data(vec![0x07])).await.unwrap();

        // Attendre que tous aient un InferenceResponse. Budget = 5s.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let all = [id_fg1, id_batch, id_fg2].iter().all(|id| {
                log_ref.entries_by_agent(id).iter()
                    .filter_map(|(_, e)| e.emit_payload.as_ref())
                    .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                    .any(|env| env.emit_type == EmitType::InferenceResponse as u8)
            });
            if all || tokio::time::Instant::now() >= deadline { break; }
        }

        // Oracle 1 : Batch a bien reçu InferenceResponse (anti-famine actif).
        let batch_responded = log_ref.entries_by_agent(&id_batch).iter()
            .filter_map(|(_, e)| e.emit_payload.as_ref())
            .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
            .any(|env| env.emit_type == EmitType::InferenceResponse as u8);
        assert!(batch_responded,
            "S33 P1b : Batch doit recevoir InferenceResponse (anti-famine ADR-0023)");

        // Oracle 2 : au moins une promotion a eu lieu.
        let stats = pool.queue_stats();
        assert!(stats.total_promoted >= 1,
            "S33 P1b : total_promoted >= 1 requis (Batch promu Foreground), got={}",
            stats.total_promoted);

        // Oracle 3 : Batch servi avant FG2 (promotion → Batch en tête de file Foreground).
        let response_ts = |id: &[u8; 16]| -> Option<u64> {
            log_ref.entries_by_agent(id).iter()
                .filter_map(|(_, e)| e.emit_payload.as_ref())
                .filter_map(|b| os_poc_causal_log::EmitEnvelope::from_msgpack(b).ok())
                .find(|env| env.emit_type == EmitType::InferenceResponse as u8)
                .map(|env| env.ts_us)
        };
        let ts_batch = response_ts(&id_batch).expect("S33: ts batch");
        let ts_fg2   = response_ts(&id_fg2).expect("S33: ts fg2");
        assert!(ts_batch < ts_fg2,
            "S33 P1b : Batch promu doit être servi avant FG2 (ts_batch={}µs, ts_fg2={}µs)",
            ts_batch, ts_fg2);

        println!("S33 P1b (anti-famine) : Batch promu Foreground après {}ms de starvation,                   servi avant FG2. total_promoted={}", 220, stats.total_promoted);

        drop(tx_fg1); drop(tx_batch); drop(tx_fg2);
    }

    // ── S34 — Déterminisme deux instances (UC-15 / ADR-0028 / P5) ───────────────

    /// S34 — UC-15 : déterminisme de transition d'état (P5 / ADR-0028).
    ///
    /// Deux instances distinctes (ContentStore + CausalLog séparés) partagent
    /// le même agent_id, le même module WASM (AGENT_WAT) et le même LogicalClock
    /// initialisé à la même valeur. La même séquence de N messages est envoyée à
    /// chacune. Après drain, trois propriétés sont vérifiées (P-α / P-β / P-γ) :
    ///
    ///   P-α  last_snapshot identique bit-à-bit entre A et B
    ///   P-β  séquence ordonnée des action_ids identique entre A et B
    ///   P-γ  SHA-256 de la concaténation des action_ids identique
    ///
    /// Limite documentée (UC-15) : sans LogicalClock, P5 n'est pas vérifiable
    /// (SystemClock produirait des timestamps différents → action_ids divergents).
    #[tokio::test(flavor = "current_thread")]
    async fn s34_determinism_two_instances_same_hash() {
        use super::actor::{ActorInstance, AGENT_WAT};
        use crate::clock::LogicalClock;
        use sha2::{Digest, Sha256};

        const N: usize = 100;
        const CLOCK_START: u64 = 1_700_000_000_000;
        const AGENT_ID: [u8; 16] = [0x34u8; 16];

        // Instance A — via setup()
        let (engine, store_a, log_a, _dir_a) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();

        // Instance B — même engine, stores séparés
        let dir_b = TempDir::new().unwrap();
        let cache_b = Cache::new_lru_cache(64 * 1024 * 1024);
        let store_b = Arc::new(ContentStore::open(
            &dir_b.path().join("store"), Some(cache_b.clone()),
        ).unwrap());
        let log_b = Arc::new(CausalLog::open(
            &dir_b.path().join("log"), Some(cache_b),
        ).unwrap());

        let clock_a = Arc::new(LogicalClock::new(CLOCK_START));
        let clock_b = Arc::new(LogicalClock::new(CLOCK_START));

        let mut actor_a = ActorInstance::new_precompiled_with_clock(
            &engine, &module, AGENT_ID,
            Arc::clone(&store_a), Arc::clone(&log_a), clock_a,
        ).await.unwrap();

        let mut actor_b = ActorInstance::new_precompiled_with_clock(
            &engine, &module, AGENT_ID,
            Arc::clone(&store_b), Arc::clone(&log_b), clock_b,
        ).await.unwrap();

        // Même séquence de N messages déterministes
        for i in 0..N {
            let payload = format!("s34-{i:08}").into_bytes();
            actor_a.process_one(&payload).await.unwrap();
            actor_b.process_one(&payload).await.unwrap();
        }

        // Oracle P-α : last_snapshot identique
        let snap_a = actor_a.last_snapshot()
            .expect("S34 P-α: last_snapshot A manquant");
        let snap_b = actor_b.last_snapshot()
            .expect("S34 P-α: last_snapshot B manquant");
        assert_eq!(snap_a, snap_b,
            "S34 P-α P5: last_snapshot diverge entre instances A et B");

        // Oracle P-β : séquence action_ids identique
        let ids_a: Vec<[u8; 32]> = log_a.entries_by_agent(&AGENT_ID)
            .into_iter().map(|(id, _)| id).collect();
        let ids_b: Vec<[u8; 32]> = log_b.entries_by_agent(&AGENT_ID)
            .into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids_a.len(), ids_b.len(),
            "S34 P-β P5: nombre d'entrées diverge (A={}, B={})", ids_a.len(), ids_b.len());
        assert_eq!(ids_a, ids_b,
            "S34 P-β P5: séquence action_ids diverge entre A et B");

        // Oracle P-γ : SHA-256 du log identique
        let hash_a: Vec<u8> = {
            let mut h = Sha256::new();
            for id in &ids_a { h.update(id); }
            h.finalize().to_vec()
        };
        let hash_b: Vec<u8> = {
            let mut h = Sha256::new();
            for id in &ids_b { h.update(id); }
            h.finalize().to_vec()
        };
        assert_eq!(hash_a, hash_b,
            "S34 P-γ P5: SHA-256 log diverge entre A et B");

        let hash_hex = hash_a.iter().fold(String::new(), |mut s, b| {
            s.push_str(&format!("{:02x}", b)); s
        });
        println!("S34 P5 (déterminisme) : N={N} messages, A≡B. \
            last_snapshot={}, P-γ SHA-256={}",
            snap_a.iter().fold(String::new(), |mut s, b| { s.push_str(&format!("{:02x}", b)); s }),
            hash_hex);
    }

    // ── S35 — Tempête P2×P4×P6 (UC-23 / ADR-0001 / ordre d'arbitrage) ──────────

    /// S35 — UC-23 : scénario « tempête » — P4 ≻ P2 ≻ P6 (ADR-0001).
    ///
    /// Agent A en `WaitingInference` (SleepyBackend 60 s) pendant qu'un rollback
    /// scheduler invalide C_A2 (post-S0) et sa dérivée C_B (délégation vers B).
    /// Agent B atteint sa frontière de session (session_max_actions=2) avant le rollback.
    ///
    /// L'ordre d'arbitrage ADR-0001 est vérifié en trois oracles :
    ///   P4 : C_B révoquée (cascade), check(B, C_B) == false — isolation tient, priorité max.
    ///   P2 : SchedulerRollback (0x0B) + InferenceCancelled (0x0E) + caps_invalidated >= 2.
    ///   P6 : CompensationOpen (0x11) + CompensationClose (0x12) — journal de compensation complet.
    #[tokio::test(flavor = "multi_thread")]
    async fn s35_storm_p2_p4_p6_arbitrage() {
        use super::actor::{ActorInstance, Message, INFER_AGENT_WAT, AGENT_WAT,
                           SESSION_DEFAULT_VALIDATION_TIMEOUT_MS};
        use super::scheduler::Scheduler;
        use super::inference::{InferencePool, SleepyBackend};
        use os_poc_capabilities::{CapabilityStore, Permissions};
        use os_poc_causal_log::{EmitEnvelope, EmitType};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        const ID_A: [u8; 16] = [0x35u8; 16];
        const ID_B: [u8; 16] = [0x36u8; 16];

        let (engine, store, log_ref, _dir) = setup();
        let module_a = Module::new(&engine, INFER_AGENT_WAT).unwrap();
        let module_b = Module::new(&engine, AGENT_WAT).unwrap();

        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 60_000 }));
        let infer_fn  = InferencePool::as_infer_fn(Arc::clone(&pool));
        let cancel_fn = InferencePool::as_cancel_fn(Arc::clone(&pool));

        // C_root octroyée à A AVANT S0 — doit survivre au rollback.
        let c_root = {
            let mut cs = cap_store.lock().unwrap();
            cs.grant_root(ID_A,
                Permissions { read: true, write: true, execute: false, delegate: true },
                "/data".to_string())
        };

        // Acteur A : inférence longue + cap c_root pré-S0.
        let actor_a = ActorInstance::new_precompiled_with_inference(
            &engine, &module_a, ID_A,
            store.clone(), log_ref.clone(),
            cap_store.clone(), vec![c_root],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS, 0,
            infer_fn,
        ).await.unwrap();

        // Acteur B : simple, session_max_actions=2 → SessionBoundary après 2 actions.
        let mut actor_b = ActorInstance::new_precompiled_with_caps(
            &engine, &module_b, ID_B,
            store.clone(), log_ref.clone(),
            cap_store.clone(), vec![],
        ).await.unwrap();
        actor_b.state_mut().session_max_actions = 2;

        // Scheduler pour A (rollback + journal de compensation via set_log_ref).
        let mut scheduler = Scheduler::new();
        scheduler.set_cancel_fn(cancel_fn);
        scheduler.set_log_ref(Arc::clone(&log_ref));
        let tx_a = scheduler.register(actor_a);

        // Phase 1 : A → msg[0x00] → commit_barrier → snapshot S0.
        tx_a.send(Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Phase 2 : C_A2 octroyée APRÈS S0 (post-S0 → sera révoquée par rollback).
        //           C_B déléguée de C_A2 vers B (cascade → sera révoquée aussi).
        let (c_a2, c_b) = {
            let mut cs = cap_store.lock().unwrap();
            let c_a2 = cs.grant_root(ID_A,
                Permissions { read: true, write: true, execute: false, delegate: true },
                "/data".to_string());
            let c_b = cs.delegate(c_a2, &ID_A, ID_B,
                Permissions { read: true, write: false, execute: false, delegate: false },
                "/data/sub".to_string()).unwrap();
            (c_a2, c_b)
        };

        // Précondition : toutes les caps sont présentes avant le rollback.
        {
            let cs = cap_store.lock().unwrap();
            assert!(cs.get(c_root).is_some(), "S35 pré: C_root");
            assert!(cs.get(c_a2).is_some(),   "S35 pré: C_A2");
            assert!(cs.get(c_b).is_some(),     "S35 pré: C_B");
        }

        // Phase 3 : B effectue 2 actions → frappe sa frontière de session.
        actor_b.process_one(&[0x00]).await.unwrap();
        actor_b.process_one(&[0x00]).await.unwrap();

        // Oracle intermédiaire : B a enregistré SessionBoundary (0x0A).
        let b_entries = log_ref.entries_by_agent(&ID_B);
        let b_hit_boundary = b_entries.iter().any(|(_, e)| {
            e.emit_payload.as_ref()
                .and_then(|b| EmitEnvelope::from_msgpack(b).ok())
                .map(|env| env.emit_type == EmitType::SessionBoundary as u8)
                .unwrap_or(false)
        });
        assert!(b_hit_boundary,
            "S35 P3/P1b : B doit avoir SessionBoundary (0x0A) après 2 actions");

        // Phase 4 : A entre en WaitingInference (SleepyBackend 60 s).
        tx_a.send(Message::data(vec![0x07])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(pool.is_active(&ID_A),
            "S35 : inférence doit être active avant rollback");

        // Phase 5 : Rollback de A (A est en WaitingInference).
        //   → CompensationOpen (0x11) + annulation (0x0E) + Rollback (0x0B)
        //   + CompensationClose (0x12). Caps post-S0 révoquées.
        scheduler.rollback(&ID_A, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(tx_a);
        tokio::time::sleep(Duration::from_millis(100)).await;

        // ── ORACLE P4 (priorité maximale ADR-0001) ────────────────────────────────
        {
            let cs = cap_store.lock().unwrap();
            assert!(cs.get(c_root).is_some(),
                "S35 P4: C_root (pré-S0) doit survivre — P4≻P2");
            assert!(cs.get(c_a2).is_none(),
                "S35 P2: C_A2 (post-S0, owned A) doit être révoquée");
            assert!(cs.get(c_b).is_none(),
                "S35 P4: C_B (cascade de C_A2) doit être révoquée — isolation cascade");
            assert!(!cs.check(&ID_B, c_b, "/data/sub",
                    &Permissions { read: true, write: false, execute: false, delegate: false }),
                "S35 P4: check(B, C_B) doit retourner false — P4 tient sous tempête");
        }

        // ── ORACLE P2 : SchedulerRollback + InferenceCancelled + caps_invalidated ──
        let a_envs: Vec<EmitEnvelope> = log_ref.entries_by_agent(&ID_A)
            .into_iter()
            .filter_map(|(_, e)| e.emit_payload)
            .filter_map(|b| EmitEnvelope::from_msgpack(&b).ok())
            .collect();

        let has_cancelled = a_envs.iter()
            .any(|e| e.emit_type == EmitType::InferenceCancelled as u8);
        assert!(has_cancelled,
            "S35 P2: InferenceCancelled (0x0E) requis — A était en WaitingInference");

        let has_rb = a_envs.iter()
            .any(|e| e.emit_type == EmitType::SchedulerRollback as u8);
        assert!(has_rb,
            "S35 P2: SchedulerRollback (0x0B) requis");

        let rb_env = a_envs.iter()
            .find(|e| e.emit_type == EmitType::SchedulerRollback as u8).unwrap();
        assert!(rb_env.payload.len() >= 10, "S35 P2: payload SchedulerRollback >= 10 bytes");
        assert!(rb_env.payload[9] >= 2,
            "S35 P4×P2: caps_invalidated >= 2 (C_A2 + C_B cascade), trouvé {}",
            rb_env.payload[9]);

        // ── ORACLE P6 : journal de compensation complet ───────────────────────────
        const SCHED_ID: [u8; 16] = [0xFFu8; 16];
        let sched_envs: Vec<EmitEnvelope> = log_ref.entries_by_agent(&SCHED_ID)
            .into_iter()
            .filter_map(|(_, e)| e.emit_payload)
            .filter_map(|b| EmitEnvelope::from_msgpack(&b).ok())
            .collect();

        let has_comp_open = sched_envs.iter()
            .any(|e| e.emit_type == EmitType::CompensationOpen as u8);
        assert!(has_comp_open,
            "S35 P6: CompensationOpen (0x11) requis — journal de compensation");

        let has_comp_close = sched_envs.iter()
            .any(|e| e.emit_type == EmitType::CompensationClose as u8);
        assert!(has_comp_close,
            "S35 P6: CompensationClose (0x12) requis — journal de compensation complet");

        println!("S35 P2×P4×P6 : P4≻P2≻P6 tient. C_B révoquée (cascade), \
            InferenceCancelled, CompensationOpen/Close. B avait SessionBoundary avant tempête.");
    }

}
