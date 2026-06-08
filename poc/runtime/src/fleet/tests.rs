//! Tests de la bibliothèque de Routers (ADR-0063).
//!
//! - `inv_router_mono_tenant_no_cross_fanin` : invariant de validation D4 — la garde `tenant_of`
//!   du driver est la seule frontière inter-tenant. Oracle sur l'absence d'effet via vrai cycle
//!   WASM (`process_one` côté run_loop), avec **contrôle positif miroir** obligatoire (ADR-0063).
//! - `fan_in_router_*` / `quorum_router_*` : logique de routage pure (`on_event`), déterministe.

use super::*;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use wasmtime::{Engine, Module};

use os_poc_causal_log::{ActionId, CausalLog};
use os_poc_store::{Cache, ContentStore};

use crate::actor::{ActorInstanceBuilder, AGENT_WAT};
use crate::make_engine;

fn setup() -> (Engine, Arc<ContentStore>, Arc<CausalLog>, TempDir) {
    let dir = TempDir::new().unwrap();
    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
    let store = Arc::new(
        ContentStore::open(&dir.path().join("store"), Some(shared_cache.clone())).unwrap(),
    );
    let log = Arc::new(CausalLog::open(&dir.path().join("log"), Some(shared_cache)).unwrap());
    let engine = make_engine();
    (engine, store, log, dir)
}

/// Vrai si une entrée quelconque de `agent` a `parent` dans ses `parent_ids`. Robuste au fait que
/// l'arête peut être portée par l'entrée de barrière ou par l'`ActionResult` (on cherche partout).
fn agent_has_parent(log: &CausalLog, agent: &super::AgentId, parent: &ActionId) -> bool {
    log.query_by_agent_range(agent, None, None)
        .unwrap_or_default()
        .iter()
        .any(|aid| {
            log.get(aid)
                .ok()
                .flatten()
                .map(|e| e.parent_ids.contains(parent))
                .unwrap_or(false)
        })
}

/// Invariant D4 (ADR-0063) — un driver de flotte mono-tenant (T1) ne peut PAS établir d'arête
/// causale vers un agent d'un autre tenant (T2). Trois conditions toutes obligatoires :
///   (a) contrôle positif miroir : le MÊME SendCaused autorisé (cible T1) crée bien l'arête ;
///   (b) refus observé à la source (Err) ET absence à la destination (B n'a rien reçu) ;
///   (c) identité de l'arête : dans (a) l'arête présente est bien l'action_id passé.
#[tokio::test(flavor = "current_thread")]
async fn inv_router_mono_tenant_no_cross_fanin() {
    let (engine, store, log, _dir) = setup();
    let module = Module::new(&engine, AGENT_WAT).unwrap();

    let (t1, t2) = (TenantId(1), TenantId(2));
    let producer = [0xA0u8; 16]; // T1 — produit l'action citée
    let same_tenant = [0xA1u8; 16]; // T1 — cible autorisée (miroir positif)
    let cross_tenant = [0xB2u8; 16]; // T2 — cible refusée

    // ── Producteur : action standalone dans le log partagé (déterministe, hors run_loop) ──
    let mut prod = ActorInstanceBuilder::new(&engine, &module, producer, Arc::clone(&store), Arc::clone(&log))
        .tenant(t1)
        .build()
        .await
        .unwrap();
    prod.process_one(b"produce").await.unwrap();
    let cause_action: ActionId = prod.last_action().expect("le producteur a émis une action");

    // ── Cibles enregistrées dans le Scheduler (run_loop réel) ──
    let mut scheduler = Scheduler::new();
    let a1 = ActorInstanceBuilder::new(&engine, &module, same_tenant, Arc::clone(&store), Arc::clone(&log))
        .tenant(t1)
        .build()
        .await
        .unwrap();
    let b = ActorInstanceBuilder::new(&engine, &module, cross_tenant, Arc::clone(&store), Arc::clone(&log))
        .tenant(t2)
        .build()
        .await
        .unwrap();
    scheduler.register(a1);
    scheduler.register(b);

    let mut driver = FleetDriver::new(t1, Arc::clone(&log), vec![same_tenant, cross_tenant]);
    let cause = driver.cause_for(cause_action).expect("action présente dans le log");

    // ── (a) + (c) Miroir positif : SendCaused autorisé (cible T1) → Ok, arête créée ──
    let ok = driver
        .execute(
            Route::SendCaused { to: same_tenant, payload: b"mirror".to_vec(), cause },
            &mut scheduler,
        )
        .await;
    assert_eq!(ok, Ok(()), "SendCaused same-tenant doit être autorisé");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        agent_has_parent(&log, &same_tenant, &cause_action),
        "(a/c) miroir : l'arête cross-agent (action du producteur) doit apparaître dans le DAG de la cible T1"
    );

    // ── (b) source : SendCaused cross-tenant → refus AVANT tout effet ──
    let refused = driver
        .execute(
            Route::SendCaused { to: cross_tenant, payload: b"cross".to_vec(), cause },
            &mut scheduler,
        )
        .await;
    assert_eq!(
        refused,
        Err(FleetError::CrossTenantRefused),
        "(b/source) la garde tenant_of doit refuser le SendCaused cross-tenant"
    );

    // ── (b) destination : laisser tourner, l'agent T2 ne doit AVOIR aucune arête vers l'action de T1 ──
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !agent_has_parent(&log, &cross_tenant, &cause_action),
        "(b/dest) l'agent T2 ne doit recevoir aucune arête cross-tenant (refus driver = aucun Message::caused)"
    );
}

/// `cause_for` refuse une action absente du log (garde de provenance D3).
#[tokio::test(flavor = "current_thread")]
async fn cause_for_rejects_unknown_action() {
    let (_engine, _store, log, _dir) = setup();
    let mut driver = FleetDriver::new(TenantId(1), Arc::clone(&log), vec![]);
    assert!(
        driver.cause_for([0xCCu8; 32]).is_none(),
        "une action absente du log ne peut pas devenir une cause citable"
    );
}

// ── Logique de routage pure (déterministe, sans async ni LLM) ──────────────────

/// FanInRouter : transmet chaque rapport à l'agrégateur, puis émet FINALIZE quand tous les membres
/// attendus ont répondu ; le résultat de l'agrégateur termine la flotte.
#[test]
fn fan_in_router_emits_reports_then_finalize() {
    let agg = [0xAAu8; 16];
    let spec_a = [0x01u8; 16];
    let spec_b = [0x02u8; 16];

    // Driver uniquement pour fournir un Ctx (pas d'I/O log dans ce test).
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![spec_a, spec_b, agg]);
    let ctx = driver.ctx();

    let mut labels = std::collections::HashMap::new();
    labels.insert(spec_a, "infra".to_string());
    labels.insert(spec_b, "db".to_string());
    let mut router = FanInRouter::new(agg, CauseRef([0xFFu8; 32]), labels);

    // Premier spécialiste → un SendCaused REPORT:<label>:<text> vers l'agrégateur, pas de FINALIZE.
    let r1 = router.on_event(
        FleetEvent::Result { agent: spec_a, cause: CauseRef([0x11u8; 32]), text: "a".into() },
        ctx,
    );
    assert_eq!(r1.len(), 1, "1er rapport → 1 route (REPORT)");
    match &r1[0] {
        Route::SendCaused { to, cause, payload } => {
            assert_eq!(*to, agg);
            assert_eq!(cause.action_id(), [0x11u8; 32]);
            assert_eq!(payload, b"REPORT:infra:a", "le label du membre doit préfixer le rapport");
        }
        other => panic!("attendu SendCaused, obtenu {other:?}"),
    }

    // Second (et dernier) spécialiste → REPORT + FINALIZE.
    let r2 = router.on_event(
        FleetEvent::Result { agent: spec_b, cause: CauseRef([0x22u8; 32]), text: "b".into() },
        ctx,
    );
    assert_eq!(r2.len(), 2, "dernier rapport → REPORT + FINALIZE");
    assert!(
        r2.iter().any(|r| matches!(r, Route::SendCaused { payload, cause, .. }
            if payload == b"FINALIZE" && cause.action_id() == [0xFFu8; 32])),
        "le FINALIZE doit être lié à finalize_cause"
    );

    // Résultat de l'agrégateur → Done.
    let r3 = router.on_event(
        FleetEvent::Result { agent: agg, cause: CauseRef([0x33u8; 32]), text: "final".into() },
        ctx,
    );
    assert!(matches!(r3.as_slice(), [Route::Done]), "le résultat de l'agrégateur termine la flotte");
}

/// QuorumRouter : transmet chaque vote au secrétaire, émet TALLY au seuil ; le résultat du
/// secrétaire termine la flotte.
#[test]
fn quorum_router_emits_tally_at_threshold() {
    let sec = [0x55u8; 16];
    let v1 = [0x01u8; 16];
    let v2 = [0x02u8; 16];

    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![v1, v2, sec]);
    let ctx = driver.ctx();

    let mut router = QuorumRouter::new(sec, CauseRef([0xFFu8; 32]), 2);

    let r1 = router.on_event(
        FleetEvent::Result { agent: v1, cause: CauseRef([0x11u8; 32]), text: "APPROVE".into() },
        ctx,
    );
    assert_eq!(r1.len(), 1, "1er vote (seuil non atteint) → VOTE seul");

    let r2 = router.on_event(
        FleetEvent::Result { agent: v2, cause: CauseRef([0x22u8; 32]), text: "REJECT".into() },
        ctx,
    );
    assert_eq!(r2.len(), 2, "2e vote (seuil atteint) → VOTE + TALLY");
    assert!(
        r2.iter().any(|r| matches!(r, Route::SendCaused { payload, cause, .. }
            if payload == b"TALLY:2" && cause.action_id() == [0xFFu8; 32])),
        "le TALLY doit être lié à la proposition et porter le seuil"
    );

    let r3 = router.on_event(
        FleetEvent::Result { agent: sec, cause: CauseRef([0x33u8; 32]), text: "APPROVED".into() },
        ctx,
    );
    assert!(matches!(r3.as_slice(), [Route::Done]), "le résultat du secrétaire termine la flotte");
}

/// FanInRouter : un `Deadline` finalise PARTIELLEMENT (FINALIZE avec les rapports reçus) — restaure
/// le comportement partiel-sur-timeout des runners pré-fleet.
#[test]
fn fan_in_router_finalizes_partial_on_deadline() {
    let agg = [0xAAu8; 16];
    let spec_a = [0x01u8; 16];
    let spec_b = [0x02u8; 16];

    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![spec_a, spec_b, agg]);
    let ctx = driver.ctx();

    let mut router = FanInRouter::new(agg, CauseRef([0xFFu8; 32]), std::collections::HashMap::new());

    // Un seul spécialiste répond (spec_b reste muet).
    let _ = router.on_event(
        FleetEvent::Result { agent: spec_a, cause: CauseRef([0x11u8; 32]), text: "a".into() },
        ctx,
    );
    // Deadline : finalisation partielle avec le seul rapport reçu.
    let d = router.on_event(FleetEvent::Deadline { agent: spec_b }, ctx);
    assert!(
        d.iter().any(|r| matches!(r, Route::SendCaused { payload, .. } if payload == b"FINALIZE")),
        "un Deadline doit finaliser partiellement (FINALIZE) si ≥1 rapport reçu"
    );
    // Idempotence : un second Deadline ne re-finalise pas.
    let d2 = router.on_event(FleetEvent::Deadline { agent: spec_b }, ctx);
    assert!(d2.is_empty(), "FINALIZE n'est émis qu'une fois");
}

/// FanInRouter : un `Deadline` sans aucun rapport reçu ne finalise PAS (rien à agréger).
#[test]
fn fan_in_router_deadline_without_results_is_noop() {
    let agg = [0xAAu8; 16];
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![[0x01u8; 16], agg]);
    let mut router = FanInRouter::new(agg, CauseRef([0xFFu8; 32]), std::collections::HashMap::new());
    let d = router.on_event(FleetEvent::Deadline { agent: [0x01u8; 16] }, driver.ctx());
    assert!(d.is_empty(), "aucun rapport reçu → pas de FINALIZE (rien à agréger)");
}

/// QuorumRouter : un `Deadline` déclenche un tally PARTIEL (TALLY:<threshold>) avec les votes reçus.
#[test]
fn quorum_router_tallies_partial_on_deadline() {
    let sec = [0x55u8; 16];
    let v1 = [0x01u8; 16];
    let v2 = [0x02u8; 16];

    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![v1, v2, sec]);
    let ctx = driver.ctx();

    let mut router = QuorumRouter::new(sec, CauseRef([0xFFu8; 32]), 2);

    // Un seul vote (seuil 2 non atteint).
    let _ = router.on_event(
        FleetEvent::Result { agent: v1, cause: CauseRef([0x11u8; 32]), text: "APPROVE".into() },
        ctx,
    );
    // Deadline : tally partiel avec le seul vote — TALLY porte le seuil (N attendu), pas le reçu.
    let d = router.on_event(FleetEvent::Deadline { agent: v2 }, ctx);
    assert!(
        d.iter().any(|r| matches!(r, Route::SendCaused { payload, .. } if payload == b"TALLY:2")),
        "un Deadline doit déclencher un tally partiel (TALLY:<N attendu>) si ≥1 vote"
    );
}

// ── Incrément 2c : Pipeline / Refine / Supervise ───────────────────────────────

fn wrap_stage(next: usize, text: &str) -> Vec<u8> {
    format!("STAGE{next}:{text}").into_bytes()
}

fn accept_on_ok(text: &str) -> bool {
    text.starts_with("OK")
}

/// PipelineRouter : transmet le résultat de l'étape i à l'étape i+1, puis Done à la dernière.
#[test]
fn pipeline_router_forwards_then_done() {
    let a = [0x0Au8; 16];
    let b = [0x0Bu8; 16];
    let c = [0x0Cu8; 16];
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![a, b, c]);
    let ctx = driver.ctx();

    let mut router = PipelineRouter::new(vec![a, b, c]);

    // Résultat de A → forward vers B.
    let r1 = router.on_event(
        FleetEvent::Result { agent: a, cause: CauseRef([0x11u8; 32]), text: "x".into() },
        ctx,
    );
    assert!(matches!(r1.as_slice(), [Route::SendCaused { to, .. }] if *to == b), "A → B");
    // Résultat de B → forward vers C.
    let r2 = router.on_event(
        FleetEvent::Result { agent: b, cause: CauseRef([0x22u8; 32]), text: "y".into() },
        ctx,
    );
    assert!(matches!(r2.as_slice(), [Route::SendCaused { to, .. }] if *to == c), "B → C");
    // Résultat de C (dernière étape) → Done.
    let r3 = router.on_event(
        FleetEvent::Result { agent: c, cause: CauseRef([0x33u8; 32]), text: "z".into() },
        ctx,
    );
    assert!(matches!(r3.as_slice(), [Route::Done]), "dernière étape → Done");
}

/// PipelineRouter::with_transform applique le transform de payload par-saut.
#[test]
fn pipeline_router_transform_wraps_payload() {
    let a = [0x0Au8; 16];
    let b = [0x0Bu8; 16];
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![a, b]);

    let mut router = PipelineRouter::with_transform(vec![a, b], wrap_stage);
    let r = router.on_event(
        FleetEvent::Result { agent: a, cause: CauseRef([0x11u8; 32]), text: "hi".into() },
        driver.ctx(),
    );
    match r.as_slice() {
        [Route::SendCaused { to, payload, .. }] => {
            assert_eq!(*to, b);
            assert_eq!(payload, b"STAGE1:hi", "le transform préfixe l'étape cible");
        }
        other => panic!("attendu un SendCaused transformé, obtenu {other:?}"),
    }
}

/// RefineRouter : re-soumet tant que non accepté, puis Done à l'acceptation.
#[test]
fn refine_router_loops_until_accepted() {
    let r = [0x0Eu8; 16];
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![r]);
    let ctx = driver.ctx();

    let mut router = RefineRouter::new(r, 5, accept_on_ok);
    // Tour 1 non accepté → re-soumission.
    let t1 = router.on_event(
        FleetEvent::Result { agent: r, cause: CauseRef([0x11u8; 32]), text: "draft".into() },
        ctx,
    );
    assert!(matches!(t1.as_slice(), [Route::SendCaused { to, .. }] if *to == r), "non accepté → re-raffine");
    // Tour 2 accepté → Done.
    let t2 = router.on_event(
        FleetEvent::Result { agent: r, cause: CauseRef([0x22u8; 32]), text: "OK final".into() },
        ctx,
    );
    assert!(matches!(t2.as_slice(), [Route::Done]), "accepté (OK…) → Done");
}

/// RefineRouter : Done à `max_iter` même sans acceptation (boucle bornée).
#[test]
fn refine_router_stops_at_max_iter() {
    let r = [0x0Eu8; 16];
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![r]);
    let ctx = driver.ctx();

    let mut router = RefineRouter::new(r, 2, accept_on_ok); // max_iter = 2
    let _ = router.on_event(
        FleetEvent::Result { agent: r, cause: CauseRef([0x11u8; 32]), text: "draft1".into() },
        ctx,
    );
    let t2 = router.on_event(
        FleetEvent::Result { agent: r, cause: CauseRef([0x22u8; 32]), text: "draft2".into() },
        ctx,
    );
    assert!(matches!(t2.as_slice(), [Route::Done]), "max_iter atteint → Done même sans acceptation");
}

/// SuperviseRouter : Done à réception de l'emit typé attendu du bon agent ; ignore le reste.
#[test]
fn supervise_router_done_on_awaited_emit() {
    let target = [0x5Au8; 16];
    let other = [0x99u8; 16];
    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![target]);
    let ctx = driver.ctx();

    let mut router = SuperviseRouter::new(target, 0x13); // attend AgentCrash (0x13)

    // Mauvais type → ignoré.
    let wrong_type = router.on_event(
        FleetEvent::Emit { agent: target, cause: CauseRef([0x00u8; 32]), emit_type: 0x02, payload: vec![] },
        ctx,
    );
    assert!(wrong_type.is_empty(), "emit d'un autre type → ignoré");
    // Bon type mais mauvais agent → ignoré.
    let wrong_agent = router.on_event(
        FleetEvent::Emit { agent: other, cause: CauseRef([0x00u8; 32]), emit_type: 0x13, payload: vec![] },
        ctx,
    );
    assert!(wrong_agent.is_empty(), "emit d'un autre agent → ignoré");
    // Bon agent + bon type → Done.
    let hit = router.on_event(
        FleetEvent::Emit { agent: target, cause: CauseRef([0x00u8; 32]), emit_type: 0x13, payload: vec![] },
        ctx,
    );
    assert!(matches!(hit.as_slice(), [Route::Done]), "emit attendu du bon agent → Done");
}

// ── Famille 4 (RFC-0002) — dispatch piloté par directive typée ──────────────────

/// TA-2 — `RouteDirective::decode` est fail-closed : tout en-tête invalide ou `kind` hors vocabulaire
/// fermé → `None`, jamais de défaut silencieux. Round-trip encode/decode pour le cas valide.
#[test]
fn route_directive_decode_fail_closed() {
    let d = RouteDirective { kind: DirectiveKind::Escalate, payload: b"hello".to_vec() };
    assert_eq!(RouteDirective::decode(&d.encode()), Some(d), "round-trip valide");

    assert_eq!(RouteDirective::decode(&[0xFD, 0x04, 0x01]), None, "en-tête incomplet → None");
    assert_eq!(RouteDirective::decode(&[0x00, 0x00, 0x01, 0x01]), None, "magic faux → None");
    assert_eq!(RouteDirective::decode(&[0xFD, 0x04, 0x99, 0x01]), None, "version fausse → None");
    assert_eq!(RouteDirective::decode(&[0xFD, 0x04, 0x01, 0x63]), None, "kind=99 hors vocabulaire → None (TA-2)");

    // Delegate sans payload : valide, payload vide.
    let dd = RouteDirective::decode(&[0xFD, 0x04, 0x01, 0x02]).expect("Delegate sans payload valide");
    assert_eq!(dd.kind, DirectiveKind::Delegate);
    assert!(dd.payload.is_empty());
}

/// TA-2 + logique de routage pure du `DispatchRouter` : directive décodable+mappée → `Route::Spawn` ;
/// non décodable OU intention non mappée en config → `Vec::new()` ; résultat d'un membre spawné → `Done`.
#[test]
fn dispatch_router_fail_closed_and_spawns() {
    let child_base = [0xC0u8; 16];
    let mut table = std::collections::HashMap::new();
    table.insert(DirectiveKind::Escalate, 0usize); // seul Escalate est mappé
    let mut router = DispatchRouter::new(table, child_base);

    let (_e, _s, log, _dir) = setup();
    let driver = FleetDriver::new(TenantId(1), log, vec![]);
    let ctx = driver.ctx();
    let cause = CauseRef([0x11u8; 32]);
    let agent = [0xA0u8; 16];

    // Directive valide+mappée → Route::Spawn{idx 0, payload de la directive, cause de l'emit}.
    let dir = RouteDirective { kind: DirectiveKind::Escalate, payload: b"q".to_vec() }.encode();
    let r = router.on_event(
        FleetEvent::Emit { agent, cause, emit_type: 0x03, payload: dir },
        ctx,
    );
    match r.as_slice() {
        [Route::Spawn { template_idx, payload, cause: c, .. }] => {
            assert_eq!(*template_idx, 0, "template résolu PAR LA CONFIG (kind→idx)");
            assert_eq!(payload, b"q", "le payload transmis est celui de la directive");
            assert_eq!(*c, cause, "la cause est celle de l'emit (TA-1), pas transportée");
        }
        other => panic!("attendu un Route::Spawn, obtenu {other:?}"),
    }

    // Directive non décodable → 0 effet (TA-2).
    let bad = router.on_event(
        FleetEvent::Emit { agent, cause, emit_type: 0x03, payload: vec![0, 1, 2] },
        ctx,
    );
    assert!(bad.is_empty(), "directive corrompue → 0 effet (TA-2)");

    // Kind valide mais non mappé en config (Delegate absent de la table) → 0 effet (TA-2).
    let dlg = RouteDirective { kind: DirectiveKind::Delegate, payload: b"x".to_vec() }.encode();
    let unmapped = router.on_event(
        FleetEvent::Emit { agent, cause, emit_type: 0x03, payload: dlg },
        ctx,
    );
    assert!(unmapped.is_empty(), "intention non mappée en config → 0 effet (TA-2)");

    // Le membre spawné (1er spawn → dernier octet = 0) produit son résultat → Done.
    let mut child = child_base;
    child[15] = 0;
    let done = router.on_event(
        FleetEvent::Result { agent: child, cause, text: "ok".into() },
        ctx,
    );
    assert!(matches!(done.as_slice(), [Route::Done]), "résultat du membre spawné → Done");
}

/// Fabrique de membre minimale pour les tests d'intégration (inventaire fermé = {0}).
struct TestFactory {
    engine: Engine,
    module: Module,
    store: Arc<ContentStore>,
    log: Arc<CausalLog>,
    cap: usize,
}

impl MemberFactory for TestFactory {
    fn materialize<'a>(
        &'a self,
        template_idx: usize,
        child: super::AgentId,
        tenant: TenantId,
    ) -> Option<MemberFuture<'a>> {
        if template_idx != 0 {
            return None; // hors inventaire → fail-closed (TA-2)
        }
        Some(Box::pin(async move {
            ActorInstanceBuilder::new(&self.engine, &self.module, child, Arc::clone(&self.store), Arc::clone(&self.log))
                .tenant(tenant)
                .build()
                .await
                .map_err(|e| format!("{e:?}"))
        }))
    }
    fn capacity(&self) -> usize {
        self.cap
    }
}

/// TA-1/TA-4/TA-7 + gardes — exécution réelle de `Route::Spawn` via une `MemberFactory` :
/// matérialisation + `register` (bon tenant) + arête causale vers l'emit-directive ; et les refus
/// fail-closed (cause forgée, pas de fabrique, index inconnu, borne atteinte).
#[tokio::test(flavor = "current_thread")]
async fn dispatch_spawn_via_factory_materializes_and_guards() {
    let (engine, store, log, _dir) = setup();
    let module = Module::new(&engine, AGENT_WAT).unwrap();
    let tenant = TenantId(1);

    // Action réelle → cause citable (provenance D3/TA-1).
    let producer = [0xA0u8; 16];
    let mut prod = ActorInstanceBuilder::new(&engine, &module, producer, Arc::clone(&store), Arc::clone(&log))
        .tenant(tenant)
        .build()
        .await
        .unwrap();
    prod.process_one(b"x").await.unwrap();
    let cause_action = prod.last_action().expect("le producteur a émis une action");

    let mut scheduler = Scheduler::new();
    let child = [0xC1u8; 16];
    let payload = b"do-the-work".to_vec();

    // (garde) Driver SANS fabrique → SpawnUnsupported.
    {
        let mut d0 = FleetDriver::new(tenant, Arc::clone(&log), vec![]);
        let cause = d0.cause_for(cause_action).unwrap();
        let e = d0
            .execute(Route::Spawn { template_idx: 0, child, payload: payload.clone(), cause }, &mut scheduler)
            .await;
        assert_eq!(e, Err(FleetError::SpawnUnsupported), "pas de fabrique → refus");
    }

    let factory = Box::new(TestFactory {
        engine: engine.clone(),
        module: module.clone(),
        store: Arc::clone(&store),
        log: Arc::clone(&log),
        cap: 1,
    });
    let mut driver = FleetDriver::new(tenant, Arc::clone(&log), vec![child]).with_factory(factory);
    let cause = driver.cause_for(cause_action).unwrap();

    // (TA-1) Cause forgée (jamais diffusée par le driver) → refus, avant tout effet.
    let forged = CauseRef([0xFFu8; 32]);
    let e = driver
        .execute(Route::Spawn { template_idx: 0, child, payload: payload.clone(), cause: forged }, &mut scheduler)
        .await;
    assert_eq!(e, Err(FleetError::UnknownCause), "cause non diffusée par le driver → refus (TA-1)");

    // (TA-2) Index hors inventaire fermé → fail-closed.
    let e = driver
        .execute(Route::Spawn { template_idx: 9, child, payload: payload.clone(), cause }, &mut scheduler)
        .await;
    assert_eq!(e, Err(FleetError::UnknownTemplate), "index hors inventaire → refus (TA-2)");

    // Spawn légitime → succès, membre enregistré dans le bon tenant (TA-4), arête causale (TA-1/TA-6).
    let ok = driver
        .execute(Route::Spawn { template_idx: 0, child, payload: payload.clone(), cause }, &mut scheduler)
        .await;
    assert_eq!(ok, Ok(()), "spawn légitime réussit");
    assert_eq!(scheduler.tenant_of(&child), Some(tenant), "(TA-4) membre dans le tenant du driver");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        agent_has_parent(&log, &child, &cause_action),
        "(TA-1/TA-6) le membre a une arête causale vers l'action-directive"
    );

    // (TA-7) Borne atteinte (cap=1, 1 membre vivant) → refus, pas d'OOM.
    let child2 = [0xC2u8; 16];
    let e = driver
        .execute(Route::Spawn { template_idx: 0, child: child2, payload, cause }, &mut scheduler)
        .await;
    assert_eq!(e, Err(FleetError::SpawnRefused), "borne d'instanciation atteinte → refus (TA-7)");
}

// ── Test P-dispatch (RFC-0002 §7) — expressivité : 1 corps, N configs ───────────

/// WAT minimal émettant un `Event` (`0x03`) qui recopie son message d'entrée. Sert d'**émetteur de
/// directive** : on lui envoie les octets d'une `RouteDirective`, il les émet en `Event` typé —
/// reproduisant le chemin guest→host d'un agent famille 4 qui émet une directive (pas du texte).
const EMIT_EVENT_WAT: &str = r#"(module
  (import "env" "commit_barrier" (func $cb))
  (import "env" "emit"           (func $emit (param i32 i32 i32)))
  (func (export "process") (param $ptr i32) (param $len i32)
    call $cb
    i32.const 3
    local.get $ptr
    local.get $len
    call $emit)
  (memory (export "memory") 1))"#;

/// Fabrique qui **enregistre** l'index de template matérialisé — permet d'observer la sélection
/// pilotée par la config. Inventaire fermé = `{0, 1}` (idx ≥ 2 → `None`, fail-closed TA-2).
struct RecordingFactory {
    engine: Engine,
    spec_module: Module,
    store: Arc<ContentStore>,
    log: Arc<CausalLog>,
    inventory: usize,
    cap: usize,
    seen: Arc<std::sync::Mutex<Vec<usize>>>,
}

impl MemberFactory for RecordingFactory {
    fn materialize<'a>(
        &'a self,
        template_idx: usize,
        child: super::AgentId,
        tenant: TenantId,
    ) -> Option<MemberFuture<'a>> {
        if template_idx >= self.inventory {
            return None;
        }
        self.seen.lock().unwrap().push(template_idx);
        Some(Box::pin(async move {
            ActorInstanceBuilder::new(&self.engine, &self.spec_module, child, Arc::clone(&self.store), Arc::clone(&self.log))
                .tenant(tenant)
                .build()
                .await
                .map_err(|e| format!("{e:?}"))
        }))
    }
    fn capacity(&self) -> usize {
        self.cap
    }
}

/// Exécute une flotte famille 4 : un émetteur émet `directive` (Event 0x03 typé), le **même** corps
/// `DispatchRouter` (paramétré par `table`) route, le driver exécute. Retourne les `template_idx`
/// matérialisés (vide = rien de spawné). C'est le montage bout-en-bout guest→host→spawn.
#[allow(clippy::too_many_arguments)]
async fn p_dispatch_run(
    engine: &Engine,
    emit_mod: &Module,
    spec_mod: &Module,
    store: &Arc<ContentStore>,
    log: &Arc<CausalLog>,
    tenant: TenantId,
    emitter: super::AgentId,
    table: std::collections::HashMap<DirectiveKind, usize>,
    directive: Vec<u8>,
) -> Vec<usize> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
    let factory = Box::new(RecordingFactory {
        engine: engine.clone(),
        spec_module: spec_mod.clone(),
        store: Arc::clone(store),
        log: Arc::clone(log),
        inventory: 2,
        cap: 8,
        seen: Arc::clone(&seen),
    });
    let mut scheduler = Scheduler::new();
    let inst = ActorInstanceBuilder::new(engine, emit_mod, emitter, Arc::clone(store), Arc::clone(log))
        .tenant(tenant)
        .build()
        .await
        .unwrap();
    let tx = scheduler.register(inst);
    let mut driver = FleetDriver::new(tenant, Arc::clone(log), vec![emitter]).with_factory(factory);
    // MÊME corps de Router pour toutes les flottes — seule `table` (config) change.
    let mut router = DispatchRouter::new(table, [0xCDu8; 16]);

    tx.send(Message::data(directive)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;

    let events = driver.poll_events();
    let mut routes = Vec::new();
    for ev in events {
        routes.extend(router.on_event(ev, driver.ctx()));
    }
    for route in routes {
        let _ = driver.execute(route, &mut scheduler).await;
    }
    let out = seen.lock().unwrap().clone();
    out
}

/// **Test P-dispatch (RFC-0002 §7)** — expressivité du dispatch : **un seul corps de `DispatchRouter`**,
/// **deux configs** (support : `Escalate→0` ; orchestrate : `Delegate→1`) → **deux sélections de
/// template distinctes**, chacune **fail-closed sur l'intention de l'autre**. Démontre que le *core*
/// de la famille 4 (router-vers-un-membre-sélectionné-par-directive) est paramétrable par config,
/// sans corps de Router propre à chaque flotte.
///
/// **Portée honnête (limite inscrite) :** ce test prouve l'expressivité du **spawn-sélection** (le
/// cœur famille 4), PAS le 2ᵉ saut d'`orchestrate` (ré-injection du résultat du spécialiste vers
/// l'orchestrateur). N=2 — voir RFC-0002 §7/§8 (verdict de promotion = arbitrage coût/bénéfice).
#[tokio::test(flavor = "current_thread")]
async fn p_dispatch_one_body_two_configs() {
    let (engine, store, log, _dir) = setup();
    let emit_mod = Module::new(&engine, EMIT_EVENT_WAT).unwrap();
    let spec_mod = Module::new(&engine, AGENT_WAT).unwrap();
    let tenant = TenantId(1);

    let escalate = RouteDirective { kind: DirectiveKind::Escalate, payload: b"q".to_vec() }.encode();
    let delegate = RouteDirective { kind: DirectiveKind::Delegate, payload: b"q".to_vec() }.encode();

    let cfg_support = || {
        let mut t = std::collections::HashMap::new();
        t.insert(DirectiveKind::Escalate, 0usize);
        t
    };
    let cfg_orchestrate = || {
        let mut t = std::collections::HashMap::new();
        t.insert(DirectiveKind::Delegate, 1usize);
        t
    };

    // Config support + Escalate → template 0.
    let s_esc = p_dispatch_run(&engine, &emit_mod, &spec_mod, &store, &log, tenant, [0xE0u8; 16], cfg_support(), escalate.clone()).await;
    assert_eq!(s_esc, vec![0], "config support : Escalate → template 0");

    // Config orchestrate + Delegate → template 1. MÊME corps de Router, autre config.
    let o_del = p_dispatch_run(&engine, &emit_mod, &spec_mod, &store, &log, tenant, [0xE1u8; 16], cfg_orchestrate(), delegate.clone()).await;
    assert_eq!(o_del, vec![1], "config orchestrate : Delegate → template 1");

    // Fail-closed croisé : config support ignore Delegate (non mappé) ; orchestrate ignore Escalate.
    let s_del = p_dispatch_run(&engine, &emit_mod, &spec_mod, &store, &log, tenant, [0xE2u8; 16], cfg_support(), delegate).await;
    assert!(s_del.is_empty(), "config support : Delegate non mappé → 0 spawn (fail-closed)");

    let o_esc = p_dispatch_run(&engine, &emit_mod, &spec_mod, &store, &log, tenant, [0xE3u8; 16], cfg_orchestrate(), escalate).await;
    assert!(o_esc.is_empty(), "config orchestrate : Escalate non mappé → 0 spawn (fail-closed)");
}
