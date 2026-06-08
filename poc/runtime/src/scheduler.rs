// Scheduler Tokio : un acteur = une task Tokio, séquentialité garantie par actor model.
// S7 : overhead Tokio pur ≈ 64 bytes/tâche ; coût réel dominé par la mémoire WASM linéaire
//      (≥ 64 KiB pour `(memory 1)`). 500 agents actifs = ≥ 32 MB minimum.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use wasmtime::{Engine, Module};
use os_poc_causal_log::{ActionId, CausalLog, EmitEnvelope, EmitType, LogEntry};
use os_poc_store::{BlockHash, ContentStore, StoreError};
use os_poc_capabilities::{CapabilityId, CapabilityStore, Permissions};
use crate::actor::{ActorInstance, AgentId, EvictedState, Message, TenantId, ValidationVerdict, run_loop};
use crate::error::RuntimeError;
use crate::inference::{CancelFn, PriorityClass};
use crate::io_queue::{IoAdmissionQueue, IoError};
use std::time::{SystemTime, UNIX_EPOCH};

/// ADR-0059 — `Registry` : le **mécanisme** d'annuaire et de routage, sans politique.
///
/// Issu de la décomposition du `Scheduler` (ADR-0013 §D2, trigger armé par le test SD-0
/// `inv_sd_auth_cross_tenant_supervision_unguarded`). Le `Registry` possède l'état
/// d'annuaire (`senders`/`handles`/`dormant`/`tenants`) et n'expose que des opérations de
/// mécanisme : enregistrer, router, réveiller, recenser le tenant. **Aucune décision de
/// supervision** (suspend/rollback/checkpoint) ne vit ici — c'est le rôle du `Supervisor`
/// (séparation mécanisme/politique, seL4 [Klein et al. 2009] / Genode). La table `tenants`
/// est ici une donnée d'annuaire ; sa *consultation* à des fins de politique appartient au
/// `Supervisor` via `tenant_of`.
pub struct Registry {
    senders: HashMap<AgentId, mpsc::Sender<Message>>,
    /// Handles des tasks run_loop pour détecter les agents terminés (reap).
    handles: HashMap<AgentId, tokio::task::JoinHandle<()>>,
    /// Agents évincés de la mémoire (ADR-0030 §FutureWork).
    /// Clé : agent_id. Valeur : état minimal suffisant pour reconstituion via ContentStore.
    dormant: HashMap<AgentId, EvictedState>,
    /// ADR-0057 (MT-1) — tenant de chaque agent enregistré, indexé à `register`.
    /// Donnée d'annuaire : le `Registry` la maintient, le `Supervisor` la consulte pour
    /// la politique cross-tenant (ADR-0059, fin de la dette ADR-0057 §D5).
    tenants: HashMap<AgentId, TenantId>,
    /// M1 (revue sécurité) — garde d'isolation de câblage : `CapabilityStore` (par pointeur d'Arc)
    /// → tenant qui le détient. Permet de refuser à `register` qu'un même `cap_store` soit partagé
    /// par DEUX tenants distincts (fuite d'autorité cross-tenant). ADR-0057 §D2 reposait sur cette
    /// précondition de câblage sans la vérifier ; le garde la rend un invariant runtime.
    cap_store_tenant: HashMap<usize, TenantId>,
    /// M1 — pointeur de cap_store par agent, pour nettoyer `cap_store_tenant` au `reap` (évite un
    /// faux-positif si une adresse d'Arc libérée est réutilisée pour un autre tenant).
    agent_cap_ptr: HashMap<AgentId, usize>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            senders: HashMap::new(),
            handles: HashMap::new(),
            dormant: HashMap::new(),
            tenants: HashMap::new(),
            cap_store_tenant: HashMap::new(),
            agent_cap_ptr: HashMap::new(),
        }
    }

    /// Enregistre un acteur et lance sa run_loop dans une task Tokio.
    /// Retourne le sender pour envoyer des messages à cet acteur.
    pub fn register(&mut self, instance: ActorInstance) -> mpsc::Sender<Message> {
        // ADR-0015 dette résolue : reap() sur chaque register pour nettoyer les agents
        // terminés avant d'en enregistrer de nouveaux — évite la croissance indéfinie de
        // `senders`/`handles` dans les schedulers long-courriers.
        self.reap();
        let id = instance.agent_id();
        let tenant = instance.tenant();
        // M1 (revue sécurité) — garde d'isolation de câblage : un `cap_store` ne peut pas être
        // partagé par deux tenants distincts (sinon une capability de T1 serait résoluble depuis
        // T2 — fuite d'autorité totale, silencieuse). C'est une erreur de câblage (runner /
        // future flotte déclarative RFC-0001), pas un vecteur adverse runtime → fail-fast (panic).
        let cap_ptr = instance.cap_store_ptr();
        match self.cap_store_tenant.get(&cap_ptr) {
            Some(&owner) if owner != tenant => panic!(
                "ADR-0057 §D2 / M1 : cap_store partagé entre tenants distincts ({owner:?} et \
                 {tenant:?}) — fuite d'autorité cross-tenant. Câbler un CapabilityStore par tenant."
            ),
            _ => { self.cap_store_tenant.insert(cap_ptr, tenant); }
        }
        self.agent_cap_ptr.insert(id, cap_ptr);
        // ADR-0057 (MT-1) : indexe le tenant (D2). Le Registry n'en fait aucune politique.
        self.tenants.insert(id, tenant);
        let (tx, rx) = mpsc::channel(32);
        self.senders.insert(id, tx.clone());
        let handle = tokio::spawn(run_loop(instance, rx));
        self.handles.insert(id, handle);
        tx
    }

    /// ADR-0057 (MT-1) — tenant d'un agent enregistré, ou `None` s'il est inconnu/terminé.
    pub fn tenant_of(&self, agent: &AgentId) -> Option<TenantId> {
        self.tenants.get(agent).copied()
    }

    /// Vrai si l'agent est actif (présent dans `senders`).
    pub fn is_active(&self, agent: &AgentId) -> bool {
        self.senders.contains_key(agent)
    }

    /// Nettoie les senders et handles des agents dont la run_loop s'est terminée.
    /// À appeler périodiquement pour éviter la croissance indéfinie de `senders`.
    pub fn reap(&mut self) {
        let finished: Vec<AgentId> = self.handles
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in &finished {
            self.handles.remove(id);
            self.senders.remove(id);
            self.tenants.remove(id);
            // M1 : retirer l'entrée cap_store_tenant si plus aucun agent vivant n'utilise ce ptr.
            if let Some(ptr) = self.agent_cap_ptr.remove(id) {
                if !self.agent_cap_ptr.values().any(|&p| p == ptr) {
                    self.cap_store_tenant.remove(&ptr);
                }
            }
        }
    }

    /// Envoie un message à un acteur enregistré.
    pub async fn send(&self, target: &AgentId, msg: Message) -> Result<(), String> {
        let tx = self.senders.get(target).ok_or("agent inconnu")?;
        tx.send(msg).await.map_err(|e| e.to_string())
    }

    /// ADR-0003 — Livre un message avec cause cross-agent explicite.
    pub async fn send_caused_by(&self, target: &AgentId, payload: Vec<u8>, cause: ActionId) -> Result<(), String> {
        self.send(target, Message::caused(payload, cause)).await
    }

    // ── Cycle éviction/réveil (ADR-0030 §FutureWork) — pur mécanisme ──────────

    /// Évince un agent actif de la mémoire (envoie `Message::Evict`, attend l'`EvictedState`,
    /// nettoie l'annuaire, stocke en `dormant`). ADR-0031 §D4 : `evicted_at` capturé ici.
    pub async fn evict_agent(&mut self, target: &AgentId) -> Result<EvictedState, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(target, Message::Evict { reply: tx }).await?;
        let mut evicted = rx.await.map_err(|_| "evict reply channel closed".to_string())?;
        evicted.evicted_at = std::time::Instant::now();
        // Attendre que la run_loop se termine avant de nettoyer.
        if let Some(handle) = self.handles.remove(target) {
            let _ = handle.await;
        }
        self.senders.remove(target);
        self.dormant.insert(evicted.id, evicted.clone());
        Ok(evicted)
    }

    /// Réveille un agent évincé depuis la table dormant (ADR-0031 §D3). F6 : distingue
    /// référence pendante (`DanglingSnapshot`) de toute autre erreur I/O (`WakeFailed`).
    pub async fn wake_agent(
        &mut self,
        target:    &AgentId,
        engine:    &Engine,
        module:    &Module,
        store_ref: Arc<ContentStore>,
        log_ref:   Arc<CausalLog>,
    ) -> Result<AgentId, DeliverError> {
        let evicted = self.dormant.remove(target)
            .ok_or_else(|| DeliverError::WakeFailed(
                format!("agent {:?} non trouvé dans la table dormant", target)
            ))?;
        let instance = ActorInstance::restore_from_evicted(
            engine, module, &evicted, store_ref, log_ref,
        ).await.map_err(|e| match e {
            RuntimeError::Store(StoreError::MissingBlock(hash)) => DeliverError::DanglingSnapshot(hash),
            other => DeliverError::WakeFailed(format!("restore_from_evicted: {other:?}")),
        })?;
        self.register(instance);
        Ok(*target)
    }

    /// Retourne vrai si l'agent est dans la table dormant (évincé).
    pub fn is_dormant(&self, target: &AgentId) -> bool {
        self.dormant.contains_key(target)
    }

    /// Retourne l'état évincé d'un agent dormant (lecture seule).
    pub fn dormant_state(&self, target: &AgentId) -> Option<&EvictedState> {
        self.dormant.get(target)
    }

    /// Nombre d'agents dormants.
    pub fn dormant_count(&self) -> usize {
        self.dormant.len()
    }

    /// ADR-0031 — Livre `msg` à `target`, réveillant l'agent depuis ContentStore si dormant.
    /// Agent actif → `send` direct (pas de C2). Agent dormant → acquire C2 + wake + send.
    pub async fn deliver(
        &mut self,
        target:    &AgentId,
        msg:       Message,
        io_queue:  &IoAdmissionQueue,
        priority:  PriorityClass,
        engine:    &Engine,
        module:    &Module,
        store:     Arc<ContentStore>,
        log:       Arc<CausalLog>,
    ) -> Result<(), DeliverError> {
        // Cas 1 : agent actif — livraison directe, pas de C2 nécessaire.
        if self.senders.contains_key(target) {
            return self.send(target, msg).await.map_err(|_| DeliverError::Unknown);
        }
        // Cas 2 : agent dormant — pipeline C2 + wake + send.
        if let Some(ev) = self.dormant.get(target) {
            let last_active = Some(ev.evicted_at);
            let io_permit = io_queue.acquire(*target, priority, last_active).await
                .map_err(|e| match e {
                    IoError::NoSlot    => DeliverError::IoCongested,
                    IoError::Cancelled => DeliverError::WakeFailed("io_queue cancelled".to_string()),
                })?;
            self.wake_agent(target, engine, module, store, log).await?;
            drop(io_permit);
            return self.send(target, msg).await.map_err(|_| DeliverError::WakeFailed(
                "send après wake_agent échoué".to_string()
            ));
        }
        // Cas 3 : agent inconnu.
        Err(DeliverError::Unknown)
    }
}

/// ADR-0059 §C — Autorité de supervision présentée à une opération de politique.
///
/// **Modèle capability-style** [Dennis & Van Horn 1966] : l'autorité est un *témoin passé*,
/// jamais inférée par le `Supervisor`. Évite le confused deputy — la politique ne devine pas
/// qui a le droit, on le lui dit. Deux principaux distincts (ADR-0059 §C) :
/// - `Orchestrator` : le runner/`main` trusted qui possède le `Scheduler`. Autorité ambiante
///   cross-tenant *par construction* (il détient le registre). Toute supervision passe.
/// - `Tenant(t)` : autorité intra-tenant. Une opération sur une cible d'un autre tenant que
///   `t` est refusée (`CrossTenantDenied`) — ferme la dette ADR-0057 §D5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionAuthority {
    /// Runner/orchestrateur trusted — autorité ambiante cross-tenant.
    Orchestrator,
    /// Autorité bornée au tenant `t` — refus si la cible n'appartient pas à `t`.
    Tenant(TenantId),
}

/// ADR-0059 §C — Erreur d'une opération de supervision (politique).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisionError {
    /// L'autorité `Tenant(t)` a visé un agent d'un autre tenant. Refus fail-closed.
    /// **Audit (ADR-0059, décision O1) :** ce refus n'émet AUCUN événement log en l'état —
    /// le type de retour + le test (absence d'effet) prouvent la fermeture. On introduira un
    /// `EmitType::SupervisionDenied` dès qu'un consommateur du log DISTINCT de l'orchestrateur
    /// émetteur devra constater le refus (cf. condition de bascule O1→O2 dans ADR-0059).
    CrossTenantDenied,
    /// Échec de routage du message de supervision (agent inconnu/canal fermé).
    Routing(String),
}

impl std::fmt::Display for SupervisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SupervisionError::CrossTenantDenied =>
                write!(f, "supervision cross-tenant refusée (autorité de tenant insuffisante)"),
            SupervisionError::Routing(e) => write!(f, "routage supervision: {e}"),
        }
    }
}

/// ADR-0059 — `Supervisor` : la **politique** de supervision, séparée du mécanisme
/// (`Registry`). Issu de la décomposition du `Scheduler` (ADR-0013 §D2, trigger armé par SD-0,
/// dette ADR-0057 §D5 fermée). Détient l'état de politique (`cap_store` partagé avec la façade,
/// `cancel_fn`, `log_ref`) et opère SUR un `Registry` passé par référence — le `Supervisor`
/// n'accède jamais aux tables internes de l'annuaire, seulement à son API publique
/// (`tenant_of`/`send`/`register`). C'est ici, et seulement ici, qu'entre le check de tenant.
pub struct Supervisor {
    /// Partagé (même `Arc`) avec `Scheduler.cap_store` — révocation globale préservée.
    cap_store: Arc<Mutex<CapabilityStore>>,
    /// T6 — annule les inférences en cours avant rollback. None si aucun pool configuré.
    cancel_fn: Option<CancelFn>,
    /// ADR-0024 — référence au log pour émettre 0x11/0x12 (journal de compensation).
    log_ref: Option<Arc<CausalLog>>,
}

impl Supervisor {
    fn new(cap_store: Arc<Mutex<CapabilityStore>>) -> Self {
        Self { cap_store, cancel_fn: None, log_ref: None }
    }

    fn set_cancel_fn(&mut self, cancel_fn: CancelFn) {
        self.cancel_fn = Some(cancel_fn);
    }

    fn set_log_ref(&mut self, log_ref: Arc<CausalLog>) {
        self.log_ref = Some(log_ref);
    }

    /// ADR-0059 §C — Cœur de la politique cross-tenant. `Orchestrator` passe toujours ;
    /// `Tenant(t)` passe ssi la cible appartient à `t` (consulte `registry.tenant_of`).
    /// Un agent inconnu sous autorité de tenant est refusé (fail-closed).
    fn authorize(
        &self,
        registry: &Registry,
        target: &AgentId,
        authority: SupervisionAuthority,
    ) -> Result<(), SupervisionError> {
        match authority {
            SupervisionAuthority::Orchestrator => Ok(()),
            SupervisionAuthority::Tenant(t) => match registry.tenant_of(target) {
                Some(tt) if tt == t => Ok(()),
                _ => Err(SupervisionError::CrossTenantDenied),
            },
        }
    }

    /// Déclenche un checkpoint superviseur sur un acteur (A4), sous autorité.
    async fn checkpoint(&self, registry: &Registry, target: &AgentId, authority: SupervisionAuthority) -> Result<(), SupervisionError> {
        self.authorize(registry, target, authority)?;
        registry.send(target, Message::Checkpoint).await.map_err(SupervisionError::Routing)
    }

    /// Suspend un acteur (A4 : transition → Suspended), sous autorité.
    async fn suspend(&self, registry: &Registry, target: &AgentId, authority: SupervisionAuthority) -> Result<(), SupervisionError> {
        self.authorize(registry, target, authority)?;
        registry.send(target, Message::Suspend).await.map_err(SupervisionError::Routing)
    }

    /// A3 — Répond à une demande de validation en cours (réponse, pas supervision d'un tiers :
    /// pas de check de tenant — c'est l'orchestrateur qui répond à une requête de l'agent).
    async fn respond_validation(&self, registry: &Registry, target: &AgentId, verdict: ValidationVerdict) -> Result<(), String> {
        registry.send(target, Message::ValidationResponse { verdict }).await
    }

    /// ADR-0012 — Injecte un résumé causal au démarrage d'une nouvelle session.
    async fn resume_session(&self, registry: &Registry, target: &AgentId, summary: Vec<u8>) -> Result<(), String> {
        registry.send(target, Message::SessionResume { summary }).await
    }

    /// ADR-0003 — Crée un agent fils causalement lié au parent, avec délégation optionnelle
    /// de capabilities depuis le `cap_store` partagé. Politique d'autorité (atténuation).
    #[allow(clippy::too_many_arguments)]
    async fn spawn_child(
        &self,
        registry: &mut Registry,
        engine: &Engine,
        module: &Module,
        child_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        parent_cause: ActionId,
        initial_payload: Vec<u8>,
        parent_agent_id: &AgentId,
        cap_grants: &[(CapabilityId, Permissions, String)],
    ) -> Result<(AgentId, Vec<CapabilityId>), RuntimeError> {
        // Politique stricte : toute délégation échouée (cap inconnue, atténuation violée)
        // annule le spawn entier pour éviter un enfant avec un sous-ensemble silencieux de caps
        // (confused deputy — Hardy 1988 ; principe d'atténuation Genode/seL4).
        let child_caps: Vec<CapabilityId> = {
            // C2 (revue sécurité) : cap_store partagé intra-tenant — tolérant au poison
            // (un panic d'un porteur ne doit pas DoS tout le tenant). Mutation atomique.
            let mut store = self.cap_store.lock().unwrap_or_else(|e| e.into_inner());
            let mut caps = Vec::with_capacity(cap_grants.len());
            for (parent_cap_id, perms, resource) in cap_grants {
                match store.delegate(*parent_cap_id, parent_agent_id, child_id, perms.clone(), resource.clone()) {
                    Ok(cap_id) => caps.push(cap_id),
                    Err(e) => {
                        tracing::warn!(
                            parent = ?parent_agent_id,
                            child = ?child_id,
                            cap_id = ?parent_cap_id,
                            error = %e,
                            "spawn_child: délégation de cap échouée — spawn annulé"
                        );
                        return Err(RuntimeError::SpawnFailed);
                    }
                }
            }
            caps
        };
        let mut instance = ActorInstance::new_precompiled_with_caps(
            engine, module, child_id, store_ref, log_ref,
            self.cap_store.clone(), child_caps.clone(),
        ).await?;
        // ADR-0015 D15.2-b : capture le parent direct pour l'inclure dans le payload
        // AgentCrash (0x13) émis en cas de terminaison anormale.
        instance.set_parent_agent_id(*parent_agent_id);
        let tx = registry.register(instance);
        tx.send(Message::caused(initial_payload, parent_cause))
            .await
            .map_err(|_| RuntimeError::SpawnFailed)?;
        Ok((child_id, child_caps))
    }

    /// D5 — Rollback d'un acteur vers un snapshot antérieur, sous autorité.
    ///
    /// **Refus cross-tenant : aucun effet.** Si `authorize` échoue, on retourne immédiatement
    /// `CrossTenantDenied` SANS émettre le journal de compensation (0x11/0x12) ni envoyer
    /// `Message::Rollback` — donc aucun 0x0B dans le log. C'est ce qui rend INV-SD-AUTH
    /// observable par l'absence d'effet (décision audit O1, ADR-0059).
    ///
    /// Journal de compensation (chemin autorisé, ADR-0024) :
    ///   1. CompensationOpen (0x11) ; 2. cancel() → 0x0E ; 3. CrashPoint::AfterCancel ;
    ///   4. Message::Rollback (l'agent émet 0x0B) ; 5. CompensationClose (0x12).
    async fn rollback(&self, registry: &Registry, target: &AgentId, target_seq: u64, authority: SupervisionAuthority) -> Result<(), SupervisionError> {
        // ── Étape 0 : autorité (avant tout effet observable) ─────────────────
        self.authorize(registry, target, authority)?;

        // ── Étape 1 : CompensationOpen (0x11) ────────────────────────────────
        if let Some(log) = &self.log_ref {
            emit_compensation_open(log, target);
        }
        // ── Étape 2 : annulation de l'inférence en cours ─────────────────────
        if let Some(cancel) = &self.cancel_fn {
            cancel(target);
        }
        // ── Étape 3 : CrashPoint::AfterCancel ────────────────────────────────
        #[cfg(feature = "crash-injection")]
        crate::crash_point::fire(crate::crash_point::CrashPoint::AfterCancel);
        #[cfg(not(feature = "crash-injection"))]
        crate::crash_point::fire(());
        // ── Étape 4 : envoi du message de rollback ────────────────────────────
        let send_result = registry.send(target, Message::Rollback { target_seq }).await;
        // ── Étape 5 : CompensationClose (0x12) ───────────────────────────────
        // Émis systématiquement même si send() échoue (agent mort) pour éviter les 0x11
        // orphelins. Un 0x11 sans 0x12 reste la signature d'un crash process (ADR-0024 D3).
        if let Some(log) = &self.log_ref {
            let outcome = if send_result.is_ok() { 0x00u8 } else { 0x02u8 };
            emit_compensation_close(log, target, target_seq, outcome);
        }
        send_result.map_err(SupervisionError::Routing)
    }
}

/// ADR-0024 / ADR-0013 / ADR-0059 — `Scheduler` : façade composant `Registry` (mécanisme)
/// et `Supervisor` (politique). Le type public reste stable : ses méthodes historiques
/// délèguent au sous-composant adéquat, avec autorité `Orchestrator` implicite (le détenteur
/// du `Scheduler` EST le runner trusted). Les variantes `*_as` exposent l'autorité explicite
/// (ADR-0059 §C). Bins/tests existants inchangés (INV-SD-NOREG).
pub struct Scheduler {
    /// ADR-0059 — mécanisme d'annuaire/routage.
    registry: Registry,
    /// ADR-0059 — politique de supervision.
    supervisor: Supervisor,
    /// Partagé (même `Arc`) avec `supervisor.cap_store`. Exposé pour compat (callers/tests).
    pub cap_store: Arc<Mutex<CapabilityStore>>,
}

impl Scheduler {
    pub fn new() -> Self {
        let cap_store = Arc::new(Mutex::new(CapabilityStore::new()));
        Self {
            registry: Registry::new(),
            supervisor: Supervisor::new(cap_store.clone()),
            cap_store,
        }
    }

    /// T6 — Installe la CancelFn du pool d'inférence (sur le `Supervisor`).
    pub fn set_cancel_fn(&mut self, cancel_fn: CancelFn) {
        self.supervisor.set_cancel_fn(cancel_fn);
    }

    /// ADR-0024 — Installe la référence au log causal (journal de compensation, sur le `Supervisor`).
    pub fn set_log_ref(&mut self, log_ref: Arc<CausalLog>) {
        self.supervisor.set_log_ref(log_ref);
    }

    // ── Délégation au Registry (mécanisme — ADR-0059) ────────────────────────────

    /// Enregistre un acteur et lance sa run_loop. Délègue au `Registry`.
    pub fn register(&mut self, instance: ActorInstance) -> mpsc::Sender<Message> {
        self.registry.register(instance)
    }

    /// ADR-0057 (MT-1) — tenant d'un agent enregistré. Délègue au `Registry`.
    pub fn tenant_of(&self, agent: &AgentId) -> Option<TenantId> {
        self.registry.tenant_of(agent)
    }

    /// Nettoie les agents terminés. Délègue au `Registry`.
    pub fn reap(&mut self) {
        self.registry.reap()
    }

    /// Envoie un message à un acteur enregistré. Délègue au `Registry`.
    pub async fn send(&self, target: &AgentId, msg: Message) -> Result<(), String> {
        self.registry.send(target, msg).await
    }

    /// ADR-0003 — Livre un message avec cause cross-agent explicite. Délègue au `Registry`.
    pub async fn send_caused_by(&self, target: &AgentId, payload: Vec<u8>, cause: ActionId) -> Result<(), String> {
        self.registry.send_caused_by(target, payload, cause).await
    }

    // ── Délégation au Supervisor (politique — ADR-0059) ──────────────────────────
    // Méthodes historiques : autorité `Orchestrator` implicite (le détenteur du Scheduler
    // est le runner trusted). Variantes `*_as` : autorité explicite (ADR-0059 §C).

    /// Déclenche un checkpoint superviseur (A4). Autorité `Orchestrator`.
    pub async fn checkpoint(&self, target: &AgentId) -> Result<(), String> {
        self.supervisor.checkpoint(&self.registry, target, SupervisionAuthority::Orchestrator)
            .await.map_err(|e| e.to_string())
    }

    /// Checkpoint sous autorité explicite (ADR-0059 §C).
    pub async fn checkpoint_as(&self, target: &AgentId, authority: SupervisionAuthority) -> Result<(), SupervisionError> {
        self.supervisor.checkpoint(&self.registry, target, authority).await
    }

    /// Suspend un acteur (A4). Autorité `Orchestrator`.
    pub async fn suspend(&self, target: &AgentId) -> Result<(), String> {
        self.supervisor.suspend(&self.registry, target, SupervisionAuthority::Orchestrator)
            .await.map_err(|e| e.to_string())
    }

    /// Suspend sous autorité explicite (ADR-0059 §C).
    pub async fn suspend_as(&self, target: &AgentId, authority: SupervisionAuthority) -> Result<(), SupervisionError> {
        self.supervisor.suspend(&self.registry, target, authority).await
    }

    /// A3 — Répond à une demande de validation en cours. Délègue au `Supervisor`.
    pub async fn respond_validation(&self, target: &AgentId, verdict: ValidationVerdict) -> Result<(), String> {
        self.supervisor.respond_validation(&self.registry, target, verdict).await
    }

    /// ADR-0003 — Crée un agent fils causalement lié au parent. Délègue au `Supervisor`.
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_child(
        &mut self,
        engine: &Engine,
        module: &Module,
        child_id: AgentId,
        store_ref: Arc<ContentStore>,
        log_ref: Arc<CausalLog>,
        parent_cause: ActionId,
        initial_payload: Vec<u8>,
        parent_agent_id: &AgentId,
        cap_grants: &[(CapabilityId, Permissions, String)],
    ) -> Result<(AgentId, Vec<CapabilityId>), RuntimeError> {
        self.supervisor.spawn_child(
            &mut self.registry, engine, module, child_id, store_ref, log_ref,
            parent_cause, initial_payload, parent_agent_id, cap_grants,
        ).await
    }

    /// D5 — Rollback d'un acteur vers un snapshot antérieur. Autorité `Orchestrator`.
    pub async fn rollback(&self, target: &AgentId, target_seq: u64) -> Result<(), String> {
        self.supervisor.rollback(&self.registry, target, target_seq, SupervisionAuthority::Orchestrator)
            .await.map_err(|e| e.to_string())
    }

    /// Rollback sous autorité explicite (ADR-0059 §C). Refus cross-tenant = aucun effet.
    pub async fn rollback_as(&self, target: &AgentId, target_seq: u64, authority: SupervisionAuthority) -> Result<(), SupervisionError> {
        self.supervisor.rollback(&self.registry, target, target_seq, authority).await
    }

    /// ADR-0012 — Injecte un résumé causal au démarrage d'une nouvelle session. Délègue au `Supervisor`.
    pub async fn resume_session(&self, target: &AgentId, summary: Vec<u8>) -> Result<(), String> {
        self.supervisor.resume_session(&self.registry, target, summary).await
    }

    // ── Cycle éviction/réveil (ADR-0030 §FutureWork) — délégation au Registry ──

    /// Évince un agent actif. Délègue au `Registry`.
    pub async fn evict_agent(&mut self, target: &AgentId) -> Result<EvictedState, String> {
        self.registry.evict_agent(target).await
    }

    /// Réveille un agent évincé depuis la table dormant. Délègue au `Registry`.
    pub async fn wake_agent(
        &mut self,
        target:    &AgentId,
        engine:    &Engine,
        module:    &Module,
        store_ref: Arc<ContentStore>,
        log_ref:   Arc<CausalLog>,
    ) -> Result<AgentId, DeliverError> {
        self.registry.wake_agent(target, engine, module, store_ref, log_ref).await
    }

    /// Retourne vrai si l'agent est dans la table dormant. Délègue au `Registry`.
    pub fn is_dormant(&self, target: &AgentId) -> bool {
        self.registry.is_dormant(target)
    }

    /// Retourne l'état évincé d'un agent dormant. Délègue au `Registry`.
    pub fn dormant_state(&self, target: &AgentId) -> Option<&EvictedState> {
        self.registry.dormant_state(target)
    }

    /// Nombre d'agents dormants. Délègue au `Registry`.
    pub fn dormant_count(&self) -> usize {
        self.registry.dormant_count()
    }

    /// ADR-0031 — Livre `msg` à `target`, réveillant l'agent si dormant. Délègue au `Registry`.
    pub async fn deliver(
        &mut self,
        target:    &AgentId,
        msg:       Message,
        io_queue:  &IoAdmissionQueue,
        priority:  PriorityClass,
        engine:    &Engine,
        module:    &Module,
        store:     Arc<ContentStore>,
        log:       Arc<CausalLog>,
    ) -> Result<(), DeliverError> {
        self.registry.deliver(target, msg, io_queue, priority, engine, module, store, log).await
    }
}

/// Erreur de livraison via `Scheduler::deliver` (ADR-0031 §D3).
#[derive(Debug)]
pub enum DeliverError {
    /// `agent_id` inconnu : ni actif ni dormant.
    Unknown,
    /// `io_queue.acquire` a retourné `NoSlot` — file C2 saturée.
    /// Le caller décide : retry, drop, escalade.
    IoCongested,
    /// `wake_agent` ou `restore_from_evicted` a échoué (ContentStore corrompu, snapshot absent).
    /// Erreur dure : la reconstruction a échoué, l'agent ne peut pas être réveillé.
    WakeFailed(String),
    /// F6 : `restore_from_evicted` a détecté une référence pendante — le snapshot dont
    /// dépend l'agent est absent du ContentStore. Distinct de `WakeFailed` pour permettre
    /// une gestion différenciée (quarantaine vs retry) sans parser une chaîne.
    DanglingSnapshot(BlockHash),
}

// ── Helpers journal de compensation ───────────────────────────────────────────

/// Identifiant réservé pour les événements émis par le scheduler lui-même.
const SCHEDULER_AGENT_ID: AgentId = [0xFFu8; 16];

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Émet CompensationOpen (0x11) dans le log causal du scheduler.
///
/// Payload : [agent_id: 16B | expected_inference_event_id: 32B (zeros = inconnu)].
/// Le `expected_inference_event_id` est laissé à zéro car le scheduler ne connaît pas
/// l'action_id de l'événement InferenceCancelled avant que cancel() ne le produise.
///
/// **Durabilité (ADR-0027) :** appel à `log.append()` non-durable (pas de fsync forcé).
/// Sous SIGKILL/panic le WAL OS-buffered survit et 0x11 est rejoué par RocksDB au redémarrage.
/// Sous power-loss (non couvert Phase 6), 0x11 peut être perdu — mais alors `cancel()` n'a pas
/// non plus persisté son effet (0x0E perdu), le `Message::Rollback` (Tokio in-memory) est perdu
/// et le ContentStore reste inchangé : équivalent à « transaction jamais commencée ». P6 tient.
/// Ne pas basculer vers `append_durable()` sans coupler fsync ContentStore (cf. ADR-0027 §D4).
fn emit_compensation_open(log: &CausalLog, target_agent_id: &AgentId) {
    let ts = now_ms();
    let mut payload = [0u8; 48];
    payload[..16].copy_from_slice(target_agent_id);
    // payload[16..48] = zeros (expected_inference_event_id inconnu à ce stade)
    let envelope = EmitEnvelope::new(
        EmitType::CompensationOpen,
        SCHEDULER_AGENT_ID,
        0,
        now_us(),
        payload.to_vec(),
    );
    let entry = LogEntry {
        agent_id: SCHEDULER_AGENT_ID,
        ts_ms: ts,
        parent_ids: vec![],
        hash_before: [0u8; 32],
        hash_after: [0u8; 32],
        emit_payload: Some(envelope.to_msgpack()),
    };
    if let Err(e) = log.append(&entry) {
        tracing::warn!("CompensationOpen emit failed: {}", e);
    }
}

/// Émet CompensationClose (0x12) dans le log causal du scheduler.
///
/// Payload : [agent_id: 16B | target_seq: 8B LE | outcome: 1B] = 25B.
/// outcome : 0x00 = Applied, 0x01 = AgentTerminated, 0x02 = AgentDisappeared.
///
/// **Durabilité (ADR-0027) :** `append()` non-durable, symétrique à `emit_compensation_open`.
/// L'invariant tenu est l'uniformité du régime : 0x11/0x0E/0x0B/0x12 partagent tous le
/// régime no-force. Sous power-loss, soit toute la séquence survit (page cache OS), soit
/// aucun élément ne survit. Cela évite un mode mixte où la paire (0x11,0x12) serait durable
/// mais 0x0B non — qui ferait croire à reconstruct que la compensation a abouti alors que
/// l'application côté agent n'a laissé aucune trace. Cf. ADR-0027 §Justification cas D.
fn emit_compensation_close(log: &CausalLog, target_agent_id: &AgentId, target_seq: u64, outcome: u8) {
    let ts = now_ms();
    let mut payload = [0u8; 25];
    payload[..16].copy_from_slice(target_agent_id);
    payload[16..24].copy_from_slice(&target_seq.to_le_bytes());
    payload[24] = outcome;
    let envelope = EmitEnvelope::new(
        EmitType::CompensationClose,
        SCHEDULER_AGENT_ID,
        0,
        now_us(),
        payload.to_vec(),
    );
    let entry = LogEntry {
        agent_id: SCHEDULER_AGENT_ID,
        ts_ms: ts,
        parent_ids: vec![],
        hash_before: [0u8; 32],
        hash_after: [0u8; 32],
        emit_payload: Some(envelope.to_msgpack()),
    };
    if let Err(e) = log.append(&entry) {
        tracing::warn!("CompensationClose emit failed: {}", e);
    }
}

/// Retourne le timestamp courant en millisecondes (utilitaire pour les tests).
#[doc(hidden)]
pub fn scheduler_now_ms() -> u64 {
    now_ms()
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests unitaires Scheduler::deliver (ADR-0031) ─────────────────────────────

#[cfg(test)]
mod tests_deliver {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use wasmtime::Module;
    use os_poc_causal_log::CausalLog;
    use os_poc_store::{ContentStore, Cache};
    use crate::actor::{ActorInstance, Message, AGENT_WAT};
    use crate::io_queue::IoAdmissionQueue;
    use crate::inference::PriorityClass;
    use crate::make_engine;

    fn setup() -> (wasmtime::Engine, Arc<ContentStore>, Arc<CausalLog>, TempDir) {
        let dir = TempDir::new().unwrap();
        let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);
        let store = Arc::new(ContentStore::open(&dir.path().join("store"), Some(shared_cache.clone())).unwrap());
        let log   = Arc::new(CausalLog::open(&dir.path().join("log"), Some(shared_cache)).unwrap());
        let engine = make_engine();
        (engine, store, log, dir)
    }

    /// P-γ : un agent actif reçoit son message directement (sans passer par IoAdmissionQueue).
    /// Vérifie que `deliver` sur un agent actif n'acquiert pas de slot C2.
    #[tokio::test(flavor = "current_thread")]
    async fn t_deliver_active_agent_bypasses_io() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();

        let agent_id: AgentId = [0xA1u8; 16];
        let instance = ActorInstance::new_precompiled(
            &engine, &module, agent_id, Arc::clone(&store), Arc::clone(&log),
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.register(instance);

        // io_queue avec cap=1 mais un permit déjà consommé — si deliver passe par C2,
        // il devrait bloquer ou retourner IoCongested.
        // cap=1, queue_capacity=1 → une seule entrée possible.
        let io_queue = IoAdmissionQueue::new(1, 1);
        // Consommer le seul slot disponible pour simuler une file saturée.
        let _permit = io_queue.acquire([0xFFu8; 16], PriorityClass::Foreground, None).await.unwrap();

        // deliver sur agent actif doit réussir même si la file C2 est pleine.
        let result = scheduler.deliver(
            &agent_id,
            Message::data(vec![0x00]),
            &io_queue,
            PriorityClass::Foreground,
            &engine,
            &module,
            Arc::clone(&store),
            Arc::clone(&log),
        ).await;

        assert!(result.is_ok(), "deliver sur agent actif doit réussir même si C2 saturée");
    }

    /// P-α + P-β : un agent dormant est réveillé via C2 et reçoit son message.
    #[tokio::test(flavor = "current_thread")]
    async fn t_deliver_dormant_agent_wakes_and_receives() {
        let (engine, store, log, _dir) = setup();
        let module = Module::new(&engine, AGENT_WAT).unwrap();

        let agent_id: AgentId = [0xB2u8; 16];
        let instance = ActorInstance::new_precompiled(
            &engine, &module, agent_id, Arc::clone(&store), Arc::clone(&log),
        ).await.unwrap();

        let mut scheduler = Scheduler::new();
        scheduler.register(instance);

        // Quelques actions pour créer un historique.
        scheduler.send(&agent_id, Message::data(vec![0x00])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Évincer l'agent.
        scheduler.evict_agent(&agent_id).await.unwrap();
        assert_eq!(scheduler.dormant_count(), 1, "agent doit être dormant après éviction");
        assert!(!scheduler.registry.is_active(&agent_id), "agent ne doit plus être actif");

        // io_queue avec capacité suffisante.
        let io_queue = IoAdmissionQueue::new(2, 8);

        // deliver doit réveiller l'agent et lui livrer le message.
        let result = scheduler.deliver(
            &agent_id,
            Message::data(vec![0x00]),
            &io_queue,
            PriorityClass::Foreground,
            &engine,
            &module,
            Arc::clone(&store),
            Arc::clone(&log),
        ).await;

        assert!(result.is_ok(), "deliver sur agent dormant doit réussir : {:?}", result);
        assert_eq!(scheduler.dormant_count(), 0, "agent doit être actif après deliver");
        assert!(scheduler.registry.is_active(&agent_id), "agent doit être dans senders après réveil");

        // Laisser le temps au message d'être traité.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
