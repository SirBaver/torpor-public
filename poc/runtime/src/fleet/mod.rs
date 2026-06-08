//! ADR-0063 — Bibliothèque de Routers : `FleetDriver` + trait `Router`.
//!
//! Livrable **P-faible** sauvé de RFC-0001 §6 bis (la RFC est ABANDONNÉE ; cet ADR ne la
//! ressuscite pas). Tue le boilerplate des runners de flotte : aujourd'hui chacun fait
//! `tokio::spawn(run_loop(...))` à la main et réimplémente le poll-du-log
//! (`wait_action_result`). Le [`FleetDriver`] centralise le poll en un point et matérialise
//! les membres via `Scheduler::register` (qui lance la `run_loop`), au lieu de contourner le
//! `Scheduler`.
//!
//! # Modèle (ADR-0063 D2/D3/D4)
//!
//! - Le driver **référence** le `Scheduler` (`&mut`), ne le possède pas. Il n'appelle que la
//!   surface *mécanisme* (`register`/`send`/`send_caused_by`/`tenant_of`), jamais la surface
//!   *politique* (`spawn_child`/`rollback`/`*_as`) — réservée au `Supervisor` (ADR-0059).
//! - La causalité de flotte passe par le **canal TCB** `Message::caused` (le driver est du code
//!   trusted). **Aucun `CauseHandle` n'est minté** : il ne serait jamais consulté (seul site de
//!   consultation = `agent_add_cause`, chemin *guest* que le modèle Router ne sollicite pas).
//!   Voir ADR-0063 D3 et [[L133]].
//! - **Mono-tenant strict (D4)** : la garde [`tenant_of`](crate::scheduler::Scheduler::tenant_of)
//!   est la première instruction effective de l'exécution d'un envoi, fail-closed pré-effet. C'est
//!   la **seule** frontière inter-tenant de la flotte (analogue de `Supervisor::authorize`).
//!   Cross-tenant = DORMANT (trigger : flotte à ≥2 `TenantId`).
//!
//! # Invariant (ADR-0063 D3 bis)
//!
//! Le routage causal de flotte est décidé par le **Router/TCB**, jamais par l'agent guest. Aucune
//! arête de flotte n'emprunte le chemin guest `agent_add_cause`. Une future famille violant cet
//! invariant devra l'amender, pas le contourner.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use os_poc_causal_log::{ActionId, CausalLog, EmitEnvelope, EmitType};

use crate::actor::{ActorInstance, AgentId, Message, TenantId};
use crate::scheduler::Scheduler;

/// Référence de cause citable par un Router (ADR-0063 D3).
///
/// **Pure hygiène de typage anti-confusion d'arguments — PAS une frontière de sécurité.** Le
/// constructeur est `pub(crate)` : un Router (potentiellement défini hors de cette crate) reçoit
/// un `CauseRef` dans un [`FleetEvent::Result`] et le transite dans un [`Route::SendCaused`], mais
/// ne peut pas en fabriquer un. La non-forgeabilité réelle tient au content-addressing de
/// l'`action_id` (SHA-256) et à la garde de provenance du driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CauseRef(pub(crate) ActionId);

impl CauseRef {
    /// L'`action_id` enveloppé (lecture seule).
    pub fn action_id(&self) -> ActionId {
        self.0
    }
}

/// Événement remonté au Router par le driver (ADR-0063 D6).
#[derive(Clone, Debug)]
pub enum FleetEvent {
    /// Un agent attendu a émis un `ActionResult`. `cause` enveloppe son `action_id` (citable).
    Result {
        agent: AgentId,
        cause: CauseRef,
        text: String,
    },
    /// Le deadline de la flotte a expiré pour cet agent (aucun résultat reçu à temps).
    Deadline { agent: AgentId },
    /// Un agent a émis un événement typé **non-`ActionResult`** (lifecycle, crash `0x13`, refus
    /// `0x14`, ou un `Event` `0x03` portant une [`RouteDirective`]…), surveillé via
    /// [`FleetDriver::watch_emit`]. Sert `await_event` (§6 bis, supervision famille 6) **et** le
    /// dispatch famille 4 (RFC-0002). `cause` enveloppe l'`action_id` de cet emit (citable, non
    /// forgeable) — indispensable pour qu'un `DispatchRouter` câble la cause d'un [`Route::Spawn`]
    /// sans jamais fabriquer d'`ActionId` (garde-fou 1 ADR-0063 §6 bis). Non émis tant qu'aucun type
    /// n'est surveillé.
    Emit { agent: AgentId, cause: CauseRef, emit_type: u8, payload: Vec<u8> },
}

/// Action de routage décidée par un Router (ADR-0063 D6).
#[derive(Clone, Debug)]
pub enum Route {
    /// Envoi sans cause (racine du DAG).
    SendRoot { to: AgentId, payload: Vec<u8> },
    /// Envoi causalement lié à `cause` (canal TCB `Message::caused`).
    SendCaused {
        to: AgentId,
        payload: Vec<u8>,
        cause: CauseRef,
    },
    /// Terminaison coopérative d'un agent (mécanique no-op dans l'incrément 1 — la terminaison
    /// est gérée par le runner/`Scheduler` qui détient les `tx` ; voir ADR-0063 D2).
    Close(AgentId),
    /// **Famille 4 (RFC-0002)** — matérialise un nouveau membre puis lui route `payload`.
    ///
    /// Le driver (et lui seul) : (1) appelle sa [`MemberFactory`] sur `template_idx` (index dans
    /// l'inventaire **fermé** du runner — le « quoi spawner » reste politique TCB, jamais dans la
    /// directive de l'agent) ; (2) `register`-e l'instance (surface **mécanisme**, ADR-0063 D2) en
    /// gardant son `tx` ; (3) envoie `payload` **causalement lié à `cause`** (canal TCB).
    /// `cause` est un [`CauseRef`] non forgeable issu de l'emit-directive (garde-fou 1). C'est la
    /// seule `Route` à **introduire un acteur** en cours de run — d'où la borne d'instanciation
    /// (TA-7) appliquée à l'exécution.
    Spawn {
        template_idx: usize,
        child: AgentId,
        payload: Vec<u8>,
        cause: CauseRef,
    },
    /// La flotte a atteint son terme : le driver arrête sa boucle.
    Done,
}

/// Contexte présenté au Router à chaque `on_event` (ADR-0063 D6).
pub struct Ctx {
    tenant: TenantId,
    expected: Vec<AgentId>,
}

impl Ctx {
    /// Tenant du driver (tous les membres de la flotte y appartiennent, D4).
    pub fn tenant(&self) -> TenantId {
        self.tenant
    }
    /// Set d'agents attendus (paramètre le quorum N sans le coder en dur — garde-fou 3 §6 bis).
    pub fn expected(&self) -> &[AgentId] {
        &self.expected
    }
}

/// Politique de routage d'une flotte (ADR-0063 D6). Turing-complète DEDANS (corps Rust), finie
/// DEHORS (l'interface ne grandit pas d'une famille à l'autre — relevé §6 bis).
pub trait Router {
    fn on_event(&mut self, ev: FleetEvent, ctx: &Ctx) -> Vec<Route>;
}

/// Échec d'une opération du driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetError {
    /// D4 — envoi vers un agent hors du tenant du driver (ou inconnu). Refus sans effet.
    CrossTenantRefused,
    /// D3 — la cause ne provient pas d'un résultat diffusé par le driver (garde de provenance).
    UnknownCause,
    /// RFC-0002 — `Route::Spawn` sur un driver sans [`MemberFactory`] (flotte non-famille-4). Refus.
    SpawnUnsupported,
    /// RFC-0002 (TA-7) — borne d'instanciation atteinte : la fabrique refuse un membre de plus.
    SpawnRefused,
    /// RFC-0002 (TA-2) — `template_idx` hors de l'inventaire fermé de la fabrique. Refus fail-closed.
    UnknownTemplate,
    /// Échec de routage (agent inconnu/canal fermé).
    Routing(String),
}

// ── Famille 4 (RFC-0002) — directive typée + fabrique de membre ─────────────────

/// Future renvoyée par une [`MemberFactory`] (dep-free : pas d'`async-trait`).
pub type MemberFuture<'a> = Pin<Box<dyn Future<Output = Result<ActorInstance, String>> + 'a>>;

/// Fabrique de membre détenue par le driver pour la famille 4 (RFC-0002).
///
/// **Frontière mécanisme/politique (ADR-0063 D2) :** le runner *trusted* implémente ce trait et y
/// **ferme tout le « quoi spawner »** (engine, module WASM, store, log, caps atténuées éventuelles),
/// indexé par `template_idx` ∈ **inventaire statique**. Le driver ne fait qu'**exécuter un index** —
/// il ne choisit ni template ni caps, exactement comme `FanInRouter` n'exécute qu'un `finalize_cause`
/// figé au boot. La directive de l'agent **sélectionne** l'index (via [`RouteDirective::kind`] mappé
/// en config) ; elle ne **fournit** ni template ni caps (interdictions structurelles TA-3).
pub trait MemberFactory {
    /// Matérialise le membre `template_idx` avec l'identité `child` dans `tenant`. `None` si l'index
    /// est hors inventaire (fail-closed, TA-2).
    fn materialize<'a>(
        &'a self,
        template_idx: usize,
        child: AgentId,
        tenant: TenantId,
    ) -> Option<MemberFuture<'a>>;

    /// Borne d'instanciation (TA-7) : nombre maximum de membres vivants matérialisables. Au-delà,
    /// `Route::Spawn` est refusé (`SpawnRefused`), jamais d'OOM.
    fn capacity(&self) -> usize;
}

/// Vocabulaire **fermé** des intentions de routage famille 4 (RFC-0002). Un `u8` hors de cet ensemble
/// → décodage refusé (TA-2). L'agent exprime une *intention* (escalader, déléguer) ; le mapping
/// `kind → template_idx` vit **en config** du [`DispatchRouter`], jamais dans la directive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DirectiveKind {
    /// `support` — escalade vers un spécialiste.
    Escalate = 1,
    /// `orchestrate` — délégation d'une sous-question.
    Delegate = 2,
}

impl DirectiveKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Escalate),
            2 => Some(Self::Delegate),
            _ => None, // vocabulaire fermé : tout autre octet est refusé (TA-2).
        }
    }
}

/// Directive de routage typée émise par un agent guest (RFC-0002, piste (b)).
///
/// **Format de fil (ABI host/guest), zéro dépendance** — l'agent-sdk n'a pas de serde/msgpack (par
/// design : agents WASM minimaux). Layout binaire manuel, transporté dans un `Event` (`0x03`) :
///
/// ```text
///   octet 0..2 : MAGIC  = [0xFD, 0x04]   (sentinelle « directive de flotte, famille 4 »)
///   octet 2    : VERSION = 0x01
///   octet 3    : kind    (u8 ∈ vocabulaire fermé DirectiveKind)
///   octet 4..  : payload (données opaques transmises au membre — sous-question, contexte…)
/// ```
///
/// **Interdictions structurelles (le format n'a PAS de slot pour) :**
/// - pas de champ `cause: ActionId` → la cause est l'`action_id` de l'emit, lu par le driver (TA-1) ;
/// - pas de champ `caps` → les caps du membre viennent de la `MemberFactory` du runner (TA-3) ;
/// - pas de `template_idx` → l'agent choisit l'*intention* (`kind`), pas le template (renforce TA-3 ;
///   le mapping `kind → template_idx` est en config TCB).
///
/// **Fail-closed (TA-2) :** [`decode`](Self::decode) renvoie `None` sur tout octet de tête invalide
/// ou tout `kind` inconnu — jamais de défaut silencieux (contraste avec le `unwrap_or("specialist")`
/// fail-open des runners actuels, `support_runner.rs:173`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteDirective {
    pub kind: DirectiveKind,
    pub payload: Vec<u8>,
}

impl RouteDirective {
    const MAGIC: [u8; 2] = [0xFD, 0x04];
    const VERSION: u8 = 0x01;
    /// Taille de l'en-tête fixe (magic 2 + version 1 + kind 1).
    const HEADER_LEN: usize = 4;

    /// Encode la directive au format de fil (réf. pour l'agent-sdk + tests).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(Self::HEADER_LEN + self.payload.len());
        v.extend_from_slice(&Self::MAGIC);
        v.push(Self::VERSION);
        v.push(self.kind as u8);
        v.extend_from_slice(&self.payload);
        v
    }

    /// Décode fail-closed (TA-2) : `None` sur magic/version/kind invalide ou taille insuffisante.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::HEADER_LEN {
            return None;
        }
        if bytes[0..2] != Self::MAGIC || bytes[2] != Self::VERSION {
            return None;
        }
        let kind = DirectiveKind::from_u8(bytes[3])?;
        Some(Self { kind, payload: bytes[Self::HEADER_LEN..].to_vec() })
    }
}

/// Driver de flotte (ADR-0063). Possède la boucle de poll-du-log et l'exécution des `Route`.
/// Référence le `Scheduler` (passé en `&mut` à chaque envoi) — ne le possède pas (D2).
pub struct FleetDriver {
    tenant: TenantId,
    log: Arc<CausalLog>,
    /// Curseur de poll par agent (nombre d'entrées déjà converties en événements).
    cursors: HashMap<AgentId, usize>,
    /// Garde de provenance (D3) : `action_id` réellement diffusés par le driver — soit lus comme
    /// `ActionResult` dans `poll_events`, soit enregistrés via [`cause_for`](Self::cause_for).
    emitted: HashSet<ActionId>,
    /// Membres ayant produit ≥1 `ActionResult` (pour cibler les `FleetEvent::Deadline` au deadline
    /// de collecte sur les seuls membres muets).
    produced: HashSet<AgentId>,
    /// Types d'emit non-`ActionResult` à remonter comme `FleetEvent::Emit` (await_event, §6 bis).
    /// Vide par défaut → aucun `Emit` émis (FanIn/Quorum inchangés).
    watched_emits: HashSet<u8>,
    /// RFC-0002 — fabrique de membre (famille 4). `None` pour les flottes 1-3/5/6 (jamais de spawn).
    factory: Option<Box<dyn MemberFactory>>,
    /// RFC-0002 — `tx` des membres spawnés via `Route::Spawn`, **gardés** pour ne pas fermer leur
    /// canal (sinon l'acteur meurt). `len()` = borne d'instanciation courante (TA-7).
    members: HashMap<AgentId, mpsc::Sender<Message>>,
    ctx: Ctx,
}

impl FleetDriver {
    /// Crée un driver pour `tenant`, observant `expected`. `log` doit être le **même** `Arc` que
    /// celui des agents enregistrés (source des événements).
    pub fn new(tenant: TenantId, log: Arc<CausalLog>, expected: Vec<AgentId>) -> Self {
        Self {
            tenant,
            log,
            cursors: HashMap::new(),
            emitted: HashSet::new(),
            produced: HashSet::new(),
            watched_emits: HashSet::new(),
            factory: None,
            members: HashMap::new(),
            ctx: Ctx { tenant, expected },
        }
    }

    /// Dote le driver d'une [`MemberFactory`] (RFC-0002, famille 4) : active l'exécution de
    /// [`Route::Spawn`]. Sans elle, tout `Route::Spawn` est refusé (`SpawnUnsupported`). À appeler
    /// avant la boucle. Surveille aussi `Event` (`0x03`) — canal de transport des directives.
    pub fn with_factory(mut self, factory: Box<dyn MemberFactory>) -> Self {
        self.factory = Some(factory);
        self.watched_emits.insert(EmitType::Event as u8);
        self
    }

    /// Contexte courant (pour appeler un Router hors de la boucle, ex. en test).
    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Surveille un type d'emit non-`ActionResult` : `poll_events` le remontera en
    /// `FleetEvent::Emit` (await_event, §6 bis ; supervision famille 6). À appeler avant la boucle.
    pub fn watch_emit(&mut self, emit_type: u8) {
        self.watched_emits.insert(emit_type);
    }

    /// Positionne le curseur d'un agent à la fin du log courant : les actions déjà présentes ne
    /// seront PAS converties en événements. À appeler après `register`, avant la boucle.
    pub fn prime_cursor(&mut self, agent: &AgentId) {
        let n = self
            .log
            .query_by_agent_range(agent, None, None)
            .map(|v| v.len())
            .unwrap_or(0);
        self.cursors.insert(*agent, n);
    }

    /// Enregistre un `action_id` (créé par le runner, ex. la racine du DAG) comme cause citable.
    /// Vérifie son existence dans le log (provenance) et l'ajoute à `emitted`. Retourne un
    /// `CauseRef` que le runner peut passer à un Router. `None` si l'action n'existe pas.
    pub fn cause_for(&mut self, action_id: ActionId) -> Option<CauseRef> {
        match self.log.get(&action_id) {
            Ok(Some(_)) => {
                self.emitted.insert(action_id);
                Some(CauseRef(action_id))
            }
            _ => None,
        }
    }

    /// Attend le **premier** `ActionResult` de `agent` (poll jusqu'à `deadline`). Remplace le
    /// `wait_action_result` bespoke des runners. Avance le curseur de l'agent et enregistre la
    /// cause (provenance D3). `None` si le deadline expire sans résultat. Utile pour la racine du
    /// DAG (préambule hors fan-in) ou tout point de synchronisation ponctuel.
    pub async fn wait_result(
        &mut self,
        agent: &AgentId,
        tick: Duration,
        deadline: Duration,
    ) -> Option<(CauseRef, String)> {
        let stop = Instant::now() + deadline;
        loop {
            let ids = self.log.query_by_agent_range(agent, None, None).unwrap_or_default();
            let cursor = self.cursors.entry(*agent).or_insert(0);
            while *cursor < ids.len() {
                let aid = ids[*cursor];
                *cursor += 1;
                let Ok(Some(e)) = self.log.get(&aid) else { continue };
                let Some(pb) = e.emit_payload else { continue };
                let Ok(env) = EmitEnvelope::from_msgpack(&pb) else { continue };
                if env.emit_type == EmitType::ActionResult as u8 {
                    self.emitted.insert(aid);
                    self.produced.insert(*agent);
                    return Some((
                        CauseRef(aid),
                        String::from_utf8_lossy(&env.payload).trim().to_string(),
                    ));
                }
            }
            if Instant::now() >= stop {
                return None;
            }
            tokio::time::sleep(tick).await;
        }
    }

    /// Poll le log : convertit les nouveaux `ActionResult` des agents attendus en `FleetEvent::Result`
    /// et avance les curseurs. Chaque `action_id` vu est enregistré dans `emitted` (provenance D3).
    /// Coût O(N agents × M entrées) par appel — pas de curseur incrémental sur le log (ADR-0063 D7).
    pub fn poll_events(&mut self) -> Vec<FleetEvent> {
        let mut events = Vec::new();
        let expected = self.ctx.expected.clone();
        for agent in &expected {
            let ids = self.log.query_by_agent_range(agent, None, None).unwrap_or_default();
            let cursor = self.cursors.entry(*agent).or_insert(0);
            while *cursor < ids.len() {
                let aid = ids[*cursor];
                *cursor += 1;
                let Ok(Some(e)) = self.log.get(&aid) else { continue };
                let Some(pb) = e.emit_payload else { continue };
                let Ok(env) = EmitEnvelope::from_msgpack(&pb) else { continue };
                if env.emit_type == EmitType::ActionResult as u8 {
                    self.emitted.insert(aid);
                    self.produced.insert(*agent);
                    events.push(FleetEvent::Result {
                        agent: *agent,
                        cause: CauseRef(aid),
                        text: String::from_utf8_lossy(&env.payload).trim().to_string(),
                    });
                } else if self.watched_emits.contains(&env.emit_type) {
                    // RFC-0002 — enregistrer l'aid comme cause citable : un DispatchRouter pourra
                    // câbler un Route::Spawn causalement lié à CET emit-directive (TA-1), sans jamais
                    // fabriquer d'ActionId. Sans cet enregistrement, la garde de provenance (D3)
                    // bloquerait le spawn (UnknownCause).
                    self.emitted.insert(aid);
                    events.push(FleetEvent::Emit {
                        agent: *agent,
                        cause: CauseRef(aid),
                        emit_type: env.emit_type,
                        payload: env.payload,
                    });
                }
            }
        }
        events
    }

    /// D4 — garde mono-tenant : `Some(self.tenant)` requis. `None` (inconnu/terminé) = refus.
    fn tenant_ok(&self, scheduler: &Scheduler, to: &AgentId) -> bool {
        scheduler.tenant_of(to) == Some(self.tenant)
    }

    /// Exécute une `Route` (mécanisme). Pour tout envoi, la garde mono-tenant (D4) est la
    /// **première instruction effective**, fail-closed **pré-effet** (le canal TCB ne checke rien :
    /// une fois le message envoyé, l'arête est injectée — pas de fenêtre d'annulation).
    pub async fn execute(&mut self, route: Route, scheduler: &mut Scheduler) -> Result<(), FleetError> {
        match route {
            Route::SendRoot { to, payload } => {
                if !self.tenant_ok(scheduler, &to) {
                    return Err(FleetError::CrossTenantRefused);
                }
                scheduler
                    .send(&to, Message::data(payload))
                    .await
                    .map_err(FleetError::Routing)
            }
            Route::SendCaused { to, payload, cause } => {
                // D4 — garde mono-tenant AVANT tout effet.
                if !self.tenant_ok(scheduler, &to) {
                    return Err(FleetError::CrossTenantRefused);
                }
                // D3 — garde de provenance : la cause doit avoir été diffusée par le driver.
                if !self.emitted.contains(&cause.0) {
                    return Err(FleetError::UnknownCause);
                }
                // Canal TCB : Message::caused, aucun handle (D3, ADR-0058 R1 §A).
                scheduler
                    .send_caused_by(&to, payload, cause.0)
                    .await
                    .map_err(FleetError::Routing)
            }
            Route::Spawn { template_idx, child, payload, cause } => {
                // TA-1 — provenance : la cause doit être un emit réellement diffusé par le driver
                // (impossible à forger : CauseRef est pub(crate), et `emitted` n'est peuplé que par
                // la lecture du log dans poll_events).
                if !self.emitted.contains(&cause.0) {
                    return Err(FleetError::UnknownCause);
                }
                // RFC-0002 — fabrique requise (flotte famille 4 uniquement).
                let factory = self.factory.as_ref().ok_or(FleetError::SpawnUnsupported)?;
                // TA-7 — borne d'instanciation : refus AVANT tout effet, jamais d'OOM.
                if self.members.len() >= factory.capacity() {
                    return Err(FleetError::SpawnRefused);
                }
                // TA-2 — index hors inventaire fermé → refus fail-closed.
                let fut = factory
                    .materialize(template_idx, child, self.tenant)
                    .ok_or(FleetError::UnknownTemplate)?;
                let instance = fut.await.map_err(FleetError::Routing)?;
                // D2 — `register` est la surface MÉCANISME (jamais `spawn_child`). Le `tx` est
                // GARDÉ (sinon le canal se ferme et l'acteur meurt).
                let tx = scheduler.register(instance);
                self.members.insert(child, tx);
                // Canal TCB — payload causalement lié à l'emit-directive (TA-1/TA-6).
                scheduler
                    .send_caused_by(&child, payload, cause.0)
                    .await
                    .map_err(FleetError::Routing)
            }
            Route::Close(_agent) => Ok(()),
            Route::Done => Ok(()),
        }
    }

    /// Boucle dirigée par événements jusqu'à `Route::Done` (retourne `true`).
    ///
    /// Au **deadline de collecte** (`deadline`), le driver émet un `FleetEvent::Deadline` pour chaque
    /// membre attendu **muet** (n'ayant produit aucun `ActionResult`) — le Router peut alors finaliser
    /// avec les membres présents (agrégation/quorum PARTIEL), comportement faithful des runners
    /// pré-fleet. La boucle continue ensuite jusqu'à `deadline` de grâce supplémentaire pour collecter
    /// le résultat du sink (agrégateur/secrétaire). Retourne `false` si aucun `Done` au terme.
    /// `tick` = période de poll (ADR-0063 D7 : poll naïf, pas de curseur incrémental).
    pub async fn run(
        &mut self,
        router: &mut dyn Router,
        scheduler: &mut Scheduler,
        tick: Duration,
        deadline: Duration,
    ) -> bool {
        let collect_stop = Instant::now() + deadline;
        // Grâce après le deadline de collecte : laisse le sink produire son résultat.
        let hard_stop = collect_stop + deadline;
        let mut deadline_fired = false;
        loop {
            for ev in self.poll_events() {
                for route in router.on_event(ev, &self.ctx) {
                    if matches!(route, Route::Done) {
                        return true;
                    }
                    let _ = self.execute(route, scheduler).await;
                }
            }
            if !deadline_fired && Instant::now() >= collect_stop {
                deadline_fired = true;
                // Finalisation partielle : Deadline pour chaque membre attendu muet.
                let expected = self.ctx.expected.clone();
                for agent in expected {
                    if self.produced.contains(&agent) {
                        continue;
                    }
                    for route in router.on_event(FleetEvent::Deadline { agent }, &self.ctx) {
                        if matches!(route, Route::Done) {
                            return true;
                        }
                        let _ = self.execute(route, scheduler).await;
                    }
                }
            }
            if Instant::now() >= hard_stop {
                return false;
            }
            tokio::time::sleep(tick).await;
        }
    }
}

// ── Routers génériques (incrément 1 — familles 2 et 3, §6 bis) ──────────────────

/// Famille 2 (§6 bis) — **fan-in** : agrège les résultats des membres puis route un `FINALIZE`
/// causalement lié à la racine du DAG. Chaque résultat d'un membre (≠ agrégateur) est transmis à
/// l'agrégateur (`REPORT:<text>`, causalement lié au résultat). Quand tous les membres attendus
/// ont répondu, un `FINALIZE` est routé (lié à `finalize_cause`). Le résultat de l'agrégateur
/// termine la flotte (`Done`).
pub struct FanInRouter {
    aggregator: AgentId,
    finalize_cause: CauseRef,
    /// Étiquette par membre, insérée dans le payload `REPORT:<label>:<text>` (protocole fan-in
    /// attendu par l'agrégateur, ex. `infra`/`db`/`security`). Un membre sans étiquette émet
    /// `REPORT:<text>`. Le format reste **du mécanisme révisable** (ADR-0063 §D6).
    labels: HashMap<AgentId, String>,
    seen: HashSet<AgentId>,
    finalized: bool,
}

impl FanInRouter {
    pub fn new(
        aggregator: AgentId,
        finalize_cause: CauseRef,
        labels: HashMap<AgentId, String>,
    ) -> Self {
        Self {
            aggregator,
            finalize_cause,
            labels,
            seen: HashSet::new(),
            finalized: false,
        }
    }
}

impl FanInRouter {
    /// Émet le `FINALIZE` (lié à `finalize_cause`) une seule fois, si au moins un rapport est arrivé.
    fn finalize(&mut self) -> Vec<Route> {
        if !self.finalized && !self.seen.is_empty() {
            self.finalized = true;
            return vec![Route::SendCaused {
                to: self.aggregator,
                payload: b"FINALIZE".to_vec(),
                cause: self.finalize_cause,
            }];
        }
        Vec::new()
    }
}

impl Router for FanInRouter {
    fn on_event(&mut self, ev: FleetEvent, ctx: &Ctx) -> Vec<Route> {
        let (agent, cause, text) = match ev {
            FleetEvent::Result { agent, cause, text } => (agent, cause, text),
            // Deadline de collecte : finalisation PARTIELLE avec les rapports déjà reçus.
            FleetEvent::Deadline { .. } => return self.finalize(),
            FleetEvent::Emit { .. } => return Vec::new(), // fan-in n'écoute pas les emits typés
        };
        if agent == self.aggregator {
            return vec![Route::Done];
        }
        let mut routes = Vec::new();
        if self.seen.insert(agent) {
            let payload = match self.labels.get(&agent) {
                Some(label) => format!("REPORT:{label}:{text}"),
                None => format!("REPORT:{text}"),
            };
            routes.push(Route::SendCaused {
                to: self.aggregator,
                payload: payload.into_bytes(),
                cause,
            });
        }
        // Tous les membres attendus (hors agrégateur) ont-ils répondu ? → FINALIZE.
        let all_in = ctx
            .expected()
            .iter()
            .filter(|a| **a != self.aggregator)
            .all(|a| self.seen.contains(a));
        if all_in {
            routes.extend(self.finalize());
        }
        routes
    }
}

/// Famille 3 (§6 bis) — **quorum/vote** : collecte les votes (résultats des votants) et route un
/// `TALLY:<n>` vers le secrétaire (lié à la proposition) dès que `threshold` votes sont reçus.
/// Chaque vote est transmis au secrétaire (`VOTE:<text>`, lié au résultat du votant). Le résultat
/// du secrétaire termine la flotte (`Done`).
pub struct QuorumRouter {
    secretary: AgentId,
    proposal_cause: CauseRef,
    threshold: usize,
    votes: HashSet<AgentId>,
    tallied: bool,
}

impl QuorumRouter {
    pub fn new(secretary: AgentId, proposal_cause: CauseRef, threshold: usize) -> Self {
        Self {
            secretary,
            proposal_cause,
            threshold,
            votes: HashSet::new(),
            tallied: false,
        }
    }
}

impl QuorumRouter {
    /// Émet le `TALLY:<threshold>` (lié à la proposition) une seule fois, si au moins un vote est
    /// arrivé. Le secrétaire calcule la majorité contre `threshold` (= N attendu) ; les votants
    /// muets comptent comme abstentions — d'où `threshold` même en tally PARTIEL.
    fn tally(&mut self) -> Vec<Route> {
        if !self.tallied && !self.votes.is_empty() {
            self.tallied = true;
            return vec![Route::SendCaused {
                to: self.secretary,
                payload: format!("TALLY:{}", self.threshold).into_bytes(),
                cause: self.proposal_cause,
            }];
        }
        Vec::new()
    }
}

impl Router for QuorumRouter {
    fn on_event(&mut self, ev: FleetEvent, _ctx: &Ctx) -> Vec<Route> {
        let (agent, cause, text) = match ev {
            FleetEvent::Result { agent, cause, text } => (agent, cause, text),
            // Deadline de collecte : tally PARTIEL avec les votes déjà reçus.
            FleetEvent::Deadline { .. } => return self.tally(),
            FleetEvent::Emit { .. } => return Vec::new(), // quorum n'écoute pas les emits typés
        };
        if agent == self.secretary {
            return vec![Route::Done];
        }
        let mut routes = Vec::new();
        if self.votes.insert(agent) {
            routes.push(Route::SendCaused {
                to: self.secretary,
                payload: format!("VOTE:{text}").into_bytes(),
                cause,
            });
        }
        if self.votes.len() >= self.threshold {
            routes.extend(self.tally());
        }
        routes
    }
}

/// Famille 1 (§6 bis) — **pipeline** : chaîne ordonnée d'étapes. Le résultat de l'étape `i` est
/// transmis à l'étape `i+1` (causalement lié). Le résultat de la dernière étape → `Done`.
///
/// `transform(index_étape_suivante, texte_précédent) -> payload` construit le message du saut
/// suivant. C'est la **frontière P-faible** : le *routage* est générique, mais le *contenu* d'un
/// saut (prompt « Résume : … », « Améliore : … ») reste applicatif — d'où la `fn` de transform.
/// Défaut [`PipelineRouter::new`] : transmet le texte brut.
pub struct PipelineRouter {
    stages: Vec<AgentId>,
    transform: fn(usize, &str) -> Vec<u8>,
}

fn pipeline_forward_raw(_next: usize, text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

impl PipelineRouter {
    /// Chaîne transmettant le texte brut d'une étape à la suivante.
    pub fn new(stages: Vec<AgentId>) -> Self {
        Self { stages, transform: pipeline_forward_raw }
    }
    /// Chaîne avec transform de payload par-saut (prompt applicatif).
    pub fn with_transform(stages: Vec<AgentId>, transform: fn(usize, &str) -> Vec<u8>) -> Self {
        Self { stages, transform }
    }
}

impl Router for PipelineRouter {
    fn on_event(&mut self, ev: FleetEvent, _ctx: &Ctx) -> Vec<Route> {
        let FleetEvent::Result { agent, cause, text } = ev else {
            return Vec::new();
        };
        let Some(i) = self.stages.iter().position(|a| *a == agent) else {
            return Vec::new();
        };
        if i + 1 < self.stages.len() {
            let payload = (self.transform)(i + 1, &text);
            vec![Route::SendCaused { to: self.stages[i + 1], payload, cause }]
        } else {
            vec![Route::Done] // dernière étape atteinte
        }
    }
}

/// Famille 5 (§6 bis) — **raffinement itératif** : un agent re-traite sa propre sortie jusqu'à
/// convergence (`accepted`) ou `max_iter` tours. La sortie acceptée (ou la dernière) → `Done`.
///
/// Le prédicat `accepted` (convergence) est **application-spécifique** (frontière P-faible : le
/// contrôle de boucle borné est générique, le critère « assez bon » ne l'est pas). Variante
/// mono-agent (self-refine) ; la boucle draft↔critique à 2 agents avec reconstruction de prompt
/// reste un runner bespoke (cf. `iterative_runner` — la logique de feedback est trop applicative).
pub struct RefineRouter {
    refiner: AgentId,
    max_iter: usize,
    iter: usize,
    accepted: fn(&str) -> bool,
}

impl RefineRouter {
    pub fn new(refiner: AgentId, max_iter: usize, accepted: fn(&str) -> bool) -> Self {
        Self { refiner, max_iter, iter: 0, accepted }
    }
}

impl Router for RefineRouter {
    fn on_event(&mut self, ev: FleetEvent, _ctx: &Ctx) -> Vec<Route> {
        let FleetEvent::Result { agent, cause, text } = ev else {
            return Vec::new();
        };
        if agent != self.refiner {
            return Vec::new();
        }
        self.iter += 1;
        if (self.accepted)(&text) || self.iter >= self.max_iter {
            return vec![Route::Done];
        }
        // Re-soumet la sortie pour un nouveau tour (causalement lié au tour précédent).
        vec![Route::SendCaused { to: self.refiner, payload: text.into_bytes(), cause }]
    }
}

/// Famille 6 (§6 bis) — **supervision attente-typée** : attend qu'un agent surveillé émette un
/// `emit_type` précis (ex. `0x13` AgentCrash, un type d'approbation applicatif). Le driver doit
/// surveiller ce type via [`FleetDriver::watch_emit`]. À réception de l'emit attendu du bon agent →
/// `Done`. Réalise la primitive `await_event` (§6 bis) sur les emits typés (≠ `ActionResult`).
pub struct SuperviseRouter {
    target: AgentId,
    awaited: u8,
}

impl SuperviseRouter {
    pub fn new(target: AgentId, awaited_emit_type: u8) -> Self {
        Self { target, awaited: awaited_emit_type }
    }
}

impl Router for SuperviseRouter {
    fn on_event(&mut self, ev: FleetEvent, _ctx: &Ctx) -> Vec<Route> {
        match ev {
            FleetEvent::Emit { agent, emit_type, .. }
                if agent == self.target && emit_type == self.awaited =>
            {
                vec![Route::Done]
            }
            _ => Vec::new(),
        }
    }
}

/// Famille 4 (RFC-0002, piste (b)) — **dispatch piloté par directive typée**. Sur réception d'un
/// `Event` (`0x03`) portant une [`RouteDirective`] décodable, matérialise un membre dont le template
/// est résolu **par la config** (`kind → template_idx`), puis lui transmet le `payload` de la
/// directive (causalement lié à l'emit). Le résultat du membre spawné termine la flotte (`Done`).
///
/// **Le corps de ce Router est générique** : il ne contient aucune logique propre à `support` ou
/// `orchestrate` — seules la `table` de config et la [`MemberFactory`] (côté driver) varient. C'est
/// l'objet du **test P-dispatch** (RFC-0002 §7) : ré-exprimer les deux flottes famille 4 sans toucher
/// ce corps. *Incrément mécanisme (8a) : « spawn puis Done sur résultat du membre » — la ré-injection
/// du résultat vers l'orchestrateur (2ᵉ saut d'`orchestrate`) relève du test d'expressivité (8b).*
///
/// **Fail-closed (TA-2)** : directive non décodable **ou** `kind` absent de la `table` → `Vec::new()`
/// (0 effet). Aucun défaut silencieux.
pub struct DispatchRouter {
    /// Config TCB : intention fermée → index de template dans l'inventaire de la `MemberFactory`.
    table: HashMap<DirectiveKind, usize>,
    /// Préfixe d'identité des membres matérialisés (le dernier octet est un compteur).
    child_base: AgentId,
    next: u8,
    /// Membres demandés (pour reconnaître leur `ActionResult` → `Done`).
    spawned: HashSet<AgentId>,
}

impl DispatchRouter {
    pub fn new(table: HashMap<DirectiveKind, usize>, child_base: AgentId) -> Self {
        Self { table, child_base, next: 0, spawned: HashSet::new() }
    }
}

impl Router for DispatchRouter {
    fn on_event(&mut self, ev: FleetEvent, _ctx: &Ctx) -> Vec<Route> {
        match ev {
            FleetEvent::Emit { cause, emit_type, payload, .. }
                if emit_type == EmitType::Event as u8 =>
            {
                // TA-2 — décodage fail-closed du vocabulaire fermé.
                let Some(d) = RouteDirective::decode(&payload) else { return Vec::new() };
                // TA-2 — intention non mappée en config → refus (pas de défaut silencieux).
                let Some(&template_idx) = self.table.get(&d.kind) else { return Vec::new() };
                // Identité du membre (le « combien/qui » reste au Router/config, pas à l'agent).
                let mut child = self.child_base;
                child[15] = self.next;
                self.next = self.next.wrapping_add(1);
                self.spawned.insert(child);
                vec![Route::Spawn { template_idx, child, payload: d.payload, cause }]
            }
            // Le membre spawné a produit son résultat → terme de la flotte.
            FleetEvent::Result { agent, .. } if self.spawned.contains(&agent) => {
                vec![Route::Done]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
