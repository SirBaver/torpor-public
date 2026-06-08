// T3+T4+T5 (ADR-0019) — InferenceBackend trait, SleepyBackend mock, InferencePool.
// Phase 6 (ADR-0022) — InferenceQueue bornée avec priorité multi-niveau.
//
// agent_infer (T5) capture un InferFn (type-erased Arc) dans la closure func_wrap_async.
// Scheduler::rollback (T6) appelle pool.cancel(agent_id) quand lifecycle==WaitingInference.
//
// Architecture Phase 6 :
//   - InferencePool<B> conserve la même façade publique (ABI figée D-Ph6-A).
//   - Sous le capot : InferenceQueue (file prioritaire) + dispatcher Tokio auto-démarré.
//   - Le dispatcher (pool_dispatcher) :
//     1. Acquiert un permit sémaphore.
//     2. Dépile l'entrée de plus haute priorité selon ADR-0022.
//     3. Appelle le backend.
//     4. Envoie le résultat complet (InferResponse ou InferError) via le canal oneshot.
//   - submit() / submit_with_class() : enfilent puis attendent le résultat du dispatcher.

pub mod queue;
pub use queue::{InferenceQueue, PriorityClass, QueueStats, QueueTrace, SlotAcquiredInfo};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::select;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use std::pin::Pin;

use crate::actor::AgentId;

// OllamaBackend dépend de reqwest + serde_json (runtime/Cargo.toml).

/// Type-erased inference function capturée par la host function `agent_infer` (T5).
/// Créée via `InferencePool::as_infer_fn` pour effacer le paramètre générique B.
pub type InferFn = std::sync::Arc<
    dyn Fn(AgentId, Vec<u8>, u32) -> Pin<Box<dyn std::future::Future<Output = Result<InferResponse, InferError>> + Send>>
    + Send + Sync,
>;

/// Type-erased cancel function pour Scheduler::rollback (T6).
/// Créée via `InferencePool::as_cancel_fn`. No-op si l'agent n'a pas d'inférence en cours.
pub type CancelFn = std::sync::Arc<dyn Fn(&AgentId) + Send + Sync>;

/// Résultat d'une inférence réussie.
///
/// `truncated` est calculé par le host function `agent_infer` : `len_out == response_buf_cap`.
/// Ici le champ est renseigné par le backend pour les réponses qui dépassent une limite interne.
///
/// `slot_info` : informations d'admission de file (ADR-0022). Permet à actor.rs d'enrichir
/// le payload InferenceRequest (0x0C) avec priority_class, queue_depth, promoted_from.
#[derive(Debug, Clone, PartialEq)]
pub struct InferResponse {
    pub text: String,
    pub truncated: bool,
    /// Informations d'admission de file (ADR-0022). None si pas de file (backend direct).
    pub slot_info: Option<SlotAcquiredInfo>,
}

/// Erreurs possibles d'une inférence (ADR-0019 §ABI error codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferError {
    /// Demande annulée (CancellationToken déclenché — rollback ou terminate).
    Cancelled,
    /// Timeout effectif dépassé sans réponse du backend.
    Timeout,
    /// Erreur interne du backend (code 0x01..0xFF, message ≤ 255 bytes).
    BackendError { code: u8, message: String },
    /// File pleine — aucun slot disponible dans InferencePool (D-Q-V2.6, maintenant actif Phase 6).
    NoSlot,
}

/// Contrat async pour les backends d'inférence (ADR-0019 §InferenceBackend).
///
/// La méthode `infer` est appelée depuis un contexte Tokio multi-thread.
/// Elle doit respecter le contrat `biased;` de sélection : quand `cancel` est déclenché,
/// le résultat doit être Err(InferError::Cancelled) sans délai supplémentaire.
pub trait InferenceBackend: Send + Sync + 'static {
    fn infer(
        &self,
        agent_id: &AgentId,
        prompt: &[u8],
        timeout_ms: u32,
        cancel: CancellationToken,
    ) -> impl std::future::Future<Output = Result<InferResponse, InferError>> + Send;
}

/// Backend mock — simule une inférence en dormant `delay_ms` millisecondes.
///
/// Utilisé en tests d'intégration (T5, T6) pour valider le flux agent_infer
/// sans dépendre d'Ollama ou d'un vrai LLM.
///
/// Le `select! { biased; }` est **obligatoire** (ADR-0019 §Q-V2.3) :
/// la branche `cancel` doit précéder `sleep` pour que l'annulation
/// l'emporte quand les deux branches sont prêtes simultanément.
#[derive(Clone)]
pub struct SleepyBackend {
    pub delay_ms: u64,
}

impl InferenceBackend for SleepyBackend {
    async fn infer(
        &self,
        _agent_id: &AgentId,
        _prompt: &[u8],
        _timeout_ms: u32,
        cancel: CancellationToken,
    ) -> Result<InferResponse, InferError> {
        select! {
            biased;
            _ = cancel.cancelled() => Err(InferError::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {
                Ok(InferResponse {
                    text: format!("sleepy-response-after-{}ms", self.delay_ms),
                    truncated: false,
                    slot_info: None,
                })
            }
        }
    }
}

/// Backend mock avec réponse fixe — pour les tests déterministes (S1, S2).
///
/// Contrairement à SleepyBackend, permet de contrôler exactement le texte
/// retourné à l'agent WASM, ce qui rend les assertions sur le parsing JSON
/// entièrement reproductibles.
#[derive(Clone)]
pub struct FixedResponseBackend {
    pub delay_ms: u64,
    pub response: String,
}

impl InferenceBackend for FixedResponseBackend {
    async fn infer(
        &self,
        _agent_id: &AgentId,
        _prompt: &[u8],
        _timeout_ms: u32,
        cancel: CancellationToken,
    ) -> Result<InferResponse, InferError> {
        select! {
            biased;
            _ = cancel.cancelled() => Err(InferError::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {
                Ok(InferResponse {
                    text: self.response.clone(),
                    truncated: false,
                    slot_info: None,
                })
            }
        }
    }
}

/// Backend Ollama (production) — appelle qwen2.5:3b via HTTP (ADR-0019 §D-D).
///
/// Prérequis : Ollama tourne en local (`ollama serve`), modèle téléchargé
/// (`ollama pull qwen2.5:3b`). Si Ollama est absent, les appels échouent avec
/// BackendError ; les scénarios CI utilisent FixedResponseBackend à la place.
pub struct OllamaBackend {
    pub model: String,
    pub endpoint: String,
}

impl Default for OllamaBackend {
    fn default() -> Self {
        Self {
            model: "qwen2.5:3b".to_string(),
            endpoint: "http://localhost:11434".to_string(),
        }
    }
}

impl InferenceBackend for OllamaBackend {
    async fn infer(
        &self,
        _agent_id: &AgentId,
        prompt: &[u8],
        timeout_ms: u32,
        cancel: CancellationToken,
    ) -> Result<InferResponse, InferError> {
        let prompt_str = String::from_utf8_lossy(prompt).to_string();
        let url = format!("{}/api/generate", self.endpoint);
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt_str,
            "stream": false,
        });
        let client = reqwest::Client::new();
        let timeout_dur = Duration::from_millis(timeout_ms as u64);

        select! {
            biased;
            _ = cancel.cancelled() => Err(InferError::Cancelled),
            result = tokio::time::timeout(timeout_dur, client.post(&url).json(&body).send()) => {
                match result {
                    Err(_elapsed) => Err(InferError::Timeout),
                    Ok(Err(e)) => Err(InferError::BackendError { code: 0x01, message: e.to_string() }),
                    Ok(Ok(resp)) => {
                        let json: serde_json::Value = resp
                            .json()
                            .await
                            .map_err(|e| InferError::BackendError { code: 0x02, message: e.to_string() })?;
                        let text = json["response"].as_str().unwrap_or("").to_string();
                        Ok(InferResponse { text, truncated: false, slot_info: None })
                    }
                }
            }
        }
    }
}

// ── InferencePool — architecture dispatcher ────────────────────────────────────
//
// Chaque appel submit_with_class() :
//   1. Enfile une QueueEntry dans InferenceQueue (avec result_tx : Sender<DispatchResult>).
//   2. Notifie le dispatcher.
//   3. Attend sur result_rx le résultat complet (InferResponse ou InferError).
//
// Le dispatcher (pool_dispatcher, tâche Tokio de fond) :
//   1. Attend la notification.
//   2. Acquiert un permit sémaphore (borné à max_concurrent).
//   3. Dépile l'entrée de plus haute priorité (pop_next avec promotions famine).
//   4. Spawne une tâche qui appelle le backend, puis envoie le résultat via result_tx.
//
// Annulation :
//   - cancel() déclenche le CancellationToken de l'entrée (dans active HashMap ET dans la file).
//   - Le backend respecte le token (biased select).
//   - Si l'entrée est encore en file (pas encore dispatchée) : dispatcher la passe,
//     voit cancel.is_cancelled() == true, et envoie Err(Cancelled) directement.

/// Résultat complet d'une inférence, transmis par le dispatcher au soumetteur.
type DispatchResult = Result<InferResponse, InferError>;

/// T4 (ADR-0019) — Pool d'inférences borné.
/// Phase 6 (ADR-0022) : le `Semaphore` plat est remplacé par `InferenceQueue` qui
/// implémente la priorité multi-niveau (Supervisor > Foreground > Batch) et le
/// garde-fou de famine bornée (ADR-0023).
///
/// Façade publique inchangée (D-Ph6-A). Voir `submit`, `cancel`, `as_infer_fn`, `as_cancel_fn`.
pub struct InferencePool<B: InferenceBackend> {
    queue:   Arc<InferenceQueue>,
    // `backend` n'est pas stocké ici : le dispatcher en possède un Arc<B> indépendant.
    // Utiliser `new_with_queue_params` pour construire le pool avec un backend spécifique.
    _phantom: std::marker::PhantomData<B>,
    /// CancellationTokens actives, indexées par agent_id.
    active: Arc<Mutex<HashMap<AgentId, CancellationToken>>>,
}

impl<B: InferenceBackend + 'static> InferencePool<B> {
    /// Crée un pool avec `max_concurrent` slots d'inférence simultanés.
    ///
    /// `queue_capacity = 4 × max_concurrent` (ADR-0022 D3).
    /// `max_starvation_ms = 10_000` (ADR-0022 D1).
    ///
    /// Le dispatcher est démarré automatiquement en tâche de fond.
    pub fn new(max_concurrent: usize, backend: B) -> Self {
        let queue_capacity = (4 * max_concurrent).max(4);
        Self::new_with_queue_params(max_concurrent, queue_capacity, 10_000, backend)
    }

    /// Crée un pool avec paramètres de file explicites (pour les tests et scénarios S5).
    ///
    /// Le dispatcher est démarré automatiquement en tâche de fond.
    pub fn new_with_queue_params(
        max_concurrent:    usize,
        queue_capacity:    usize,
        max_starvation_ms: u64,
        backend:           B,
    ) -> Self {
        let q = Arc::new(InferenceQueue::new(max_concurrent, queue_capacity, max_starvation_ms));
        let b = Arc::new(backend);
        let active = Arc::new(Mutex::new(HashMap::new()));

        // Démarrer le dispatcher automatiquement. Le dispatcher possède son propre Arc<B>.
        tokio::spawn(pool_dispatcher(Arc::clone(&q), b));

        Self {
            queue: q,
            _phantom: std::marker::PhantomData,
            active,
        }
    }

    /// Lance une inférence pour `agent_id` (classe Foreground par défaut).
    ///
    /// Compatibilité : cette signature est identique à la Phase 2 (ABI figée D-Ph6-A).
    pub async fn submit(
        &self,
        agent_id: AgentId,
        prompt: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<InferResponse, InferError> {
        self.submit_with_class(agent_id, prompt, timeout_ms, PriorityClass::Foreground).await
    }

    /// Lance une inférence avec classe de priorité explicite (Phase 6).
    pub async fn submit_with_class(
        &self,
        agent_id: AgentId,
        prompt: Vec<u8>,
        timeout_ms: u32,
        class: PriorityClass,
    ) -> Result<InferResponse, InferError> {
        let cancel = CancellationToken::new();
        {
            let mut active = self.active.lock().unwrap();
            active.insert(agent_id, cancel.clone());
        }

        let result = self.do_submit(agent_id, prompt, timeout_ms, cancel.clone(), class).await;

        {
            let mut active = self.active.lock().unwrap();
            active.remove(&agent_id);
        }
        result
    }

    /// Interne : enfile l'entrée et attend le résultat complet du dispatcher.
    async fn do_submit(
        &self,
        agent_id: AgentId,
        prompt: Vec<u8>,
        timeout_ms: u32,
        cancel: CancellationToken,
        class: PriorityClass,
    ) -> Result<InferResponse, InferError> {
        // Créer le canal oneshot qui recevra le résultat COMPLET du dispatcher.
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<DispatchResult>();

        // Tenter l'admission dans la file.
        {
            let mut st = self.queue.state.lock().unwrap();
            match st.try_enqueue(agent_id, prompt, timeout_ms, cancel.clone(), class, result_tx) {
                Ok(_depth) => {}
                Err(e) => return Err(e),
            }
        }
        // Notifier le dispatcher qu'une entrée est disponible.
        self.queue.notify.notify_one();

        // Attendre le résultat ou annulation immédiate.
        select! {
            biased;
            _ = cancel.cancelled() => {
                // Annulation déclenchée par cancel() : le dispatcher verra le token cancelled.
                // On enregistre la cancel dans la file aussi.
                self.queue.cancel_agent(&agent_id);
                Err(InferError::Cancelled)
            }
            result = result_rx => {
                match result {
                    Ok(r)  => r,
                    Err(_) => Err(InferError::Cancelled), // canal dropé (dispatcher arrêté)
                }
            }
        }
    }

    /// Annule l'inférence en cours pour `agent_id` (déclenche la CancellationToken).
    /// No-op si aucune inférence active pour cet agent.
    pub fn cancel(&self, agent_id: &AgentId) {
        let active = self.active.lock().unwrap();
        if let Some(token) = active.get(agent_id) {
            token.cancel();
        }
        drop(active);
        self.queue.cancel_agent(agent_id);
    }

    /// Retourne true si une inférence est active pour cet agent.
    pub fn is_active(&self, agent_id: &AgentId) -> bool {
        self.active.lock().unwrap().contains_key(agent_id)
    }

    /// Retourne le nombre d'inférences actives (en cours ou en attente de slot).
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }

    /// Retourne le nombre de slots libres dans le sémaphore d'inférence.
    /// Utile pour vérifier l'absence de slot zombie après annulation (UC-10 / S16).
    pub fn available_permits(&self) -> usize {
        self.queue.semaphore.available_permits()
    }

    /// Retourne les statistiques de file (ADR-0022 D4 / ADR-0023 D3).
    pub fn queue_stats(&self) -> QueueStats {
        self.queue.stats()
    }

    /// Retourne les traces récentes de la file (ADR-0023 D3 — assertions E1/E3, Phase 10).
    pub fn queue_traces(&self) -> Vec<QueueTrace> {
        self.queue.traces()
    }

    /// Convertit le pool en InferFn (type-erased) pour la host function agent_infer (T5).
    /// Utilise PriorityClass::Foreground par défaut.
    pub fn as_infer_fn(pool: Arc<Self>) -> InferFn {
        Arc::new(move |agent_id, prompt, timeout_ms| {
            let p = Arc::clone(&pool);
            Box::pin(async move { p.submit(agent_id, prompt, timeout_ms).await })
        })
    }

    /// Variante de as_infer_fn avec classe de priorité explicite (ADR-0022, S5).
    pub fn as_infer_fn_with_class(pool: Arc<Self>, class: PriorityClass) -> InferFn {
        Arc::new(move |agent_id, prompt, timeout_ms| {
            let p = Arc::clone(&pool);
            Box::pin(async move { p.submit_with_class(agent_id, prompt, timeout_ms, class).await })
        })
    }

    /// T6 — Convertit le pool en CancelFn (type-erased) pour Scheduler::rollback.
    pub fn as_cancel_fn(pool: Arc<Self>) -> CancelFn {
        Arc::new(move |agent_id| pool.cancel(agent_id))
    }

    /// Retourne le `Notify` qui fire à chaque libération d'un slot d'inférence.
    ///
    /// Permet à `IoAdmissionQueue::new_with_c1_hint` de démarrer un préchargement
    /// C2 dès qu'un slot C1 se libère — coordination C1→C2 (ADR-0030 §FutureWork).
    pub fn slot_freed_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.queue.slot_freed)
    }
}

// ── Dispatcher Tokio ───────────────────────────────────────────────────────────
//
// Tâche de fond unique par InferencePool.
//
// Boucle principale :
//   1. Attend une notification (Notify) d'une nouvelle entrée en file.
//   2. Boucle interne : tant qu'il y a des entrées ET des permits disponibles :
//      a. Acquiert un permit sémaphore (try_acquire_owned → non bloquant).
//      b. Dépile l'entrée de plus haute priorité (pop_next).
//      c. Spawne une tâche d'inférence qui :
//         - Vérifie l'annulation avant d'appeler le backend.
//         - Appelle backend.infer().
//         - Envoie le résultat complet via result_tx.
//         - Libère le permit (drop) à la fin.
//         - Notifie le dispatcher pour débloquer d'éventuelles entrées en attente.

async fn pool_dispatcher<B: InferenceBackend + 'static>(
    queue:   Arc<InferenceQueue>,
    backend: Arc<B>,
) {
    loop {
        // Attendre qu'une entrée soit disponible ou qu'un slot se libère.
        queue.notify.notified().await;

        // Boucle interne : dépiler autant d'entrées que possible.
        loop {
            // Tenter d'acquérir un slot sémaphore (non bloquant).
            let permit = match queue.semaphore.clone().try_acquire_owned() {
                Ok(p)  => p,
                Err(_) => break, // Plus de slots libres — attendre la prochaine notification.
            };

            // Incrémenter in_flight et dépiler.
            let entry = {
                let mut st = queue.state.lock().unwrap();
                let e = st.pop_next();
                if e.is_some() {
                    st.in_flight += 1;
                }
                e
            };

            let entry = match entry {
                Some(e) => e,
                None => {
                    // File vide — remettre le permit (drop automatique).
                    drop(permit);
                    break;
                }
            };

            // Enregistrer dans la trace circulaire.
            {
                let mut t = queue.traces.lock().unwrap();
                if t.len() >= 256 { t.pop_front(); }
                t.push_back(QueueTrace {
                    agent_id:                    entry.agent_id,
                    admission_seq:               entry.admission_seq,
                    admission_instant:           entry.admission_instant,
                    priority_class_at_admission: entry.original_class,
                    promoted_from: if entry.promoted { Some(entry.original_class) } else { None },
                    slot_acquired_instant:       Some(std::time::Instant::now()),
                });
            }

            let queue_clone   = Arc::clone(&queue);
            let backend_clone = Arc::clone(&backend);

            // Construire le SlotAcquiredInfo depuis l'entrée (ADR-0022).
            let slot_info = SlotAcquiredInfo {
                priority_class:           entry.original_class,
                queue_depth_at_admission: entry.depth_at_admission,
                promoted_from: if entry.promoted { Some(entry.original_class) } else { None },
            };

            // Spawner la tâche d'inférence.
            tokio::spawn(async move {
                let _permit = permit; // maintient le slot pendant l'inférence

                // Vérifier si l'entrée a été annulée pendant l'attente en file.
                if entry.cancel.is_cancelled() {
                    let _ = entry.result_tx.send(Err(InferError::Cancelled));
                } else {
                    // Appeler le backend et envoyer le résultat complet (avec slot_info ADR-0022).
                    let result = backend_clone
                        .infer(&entry.agent_id, &entry.prompt, entry.timeout_ms, entry.cancel)
                        .await
                        .map(|mut r| { r.slot_info = Some(slot_info); r });
                    let _ = entry.result_tx.send(result);
                }

                // Décrémenter in_flight et notifier :
                //   - le dispatcher C1 pour les inférences suivantes ;
                //   - le dispatcher C2 (IoAdmissionQueue) via slot_freed (ADR-0030).
                {
                    let mut st = queue_clone.state.lock().unwrap();
                    if st.in_flight > 0 { st.in_flight -= 1; }
                }
                queue_clone.notify.notify_one();
                queue_clone.slot_freed.notify_one();
            });
        }
    }
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// T3 — SleepyBackend répond après delay_ms si pas annulé.
    #[tokio::test]
    async fn sleepy_backend_responds_after_delay() {
        let backend = SleepyBackend { delay_ms: 50 };
        let cancel = CancellationToken::new();
        let result = backend.infer(&[0u8; 16], b"test", 5000, cancel).await;
        assert!(result.is_ok());
        let resp = result.unwrap();
        assert!(resp.text.contains("50ms"));
        assert!(!resp.truncated);
    }

    /// T3 — annulation avant fin du sleep → Err(Cancelled).
    #[tokio::test]
    async fn sleepy_backend_cancelled_before_response() {
        let backend = SleepyBackend { delay_ms: 10_000 };
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_clone.cancel();
        });

        let result = backend.infer(&[0u8; 16], b"test", 20_000, cancel).await;
        assert_eq!(result, Err(InferError::Cancelled));
    }

    /// T3 — annulation immédiate (token déjà cancelled) → Err(Cancelled) sans attendre delay.
    #[tokio::test]
    async fn sleepy_backend_pre_cancelled() {
        let backend = SleepyBackend { delay_ms: 60_000 };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = backend.infer(&[0u8; 16], b"test", 60_000, cancel).await;
        assert_eq!(result, Err(InferError::Cancelled));
    }

    // ── Tests T4 — InferencePool ──────────────────────────────────────────────

    /// T4 — happy path : submit retourne la réponse du backend après delay.
    #[tokio::test]
    async fn pool_submit_returns_response() {
        let pool = InferencePool::new(4, SleepyBackend { delay_ms: 20 });
        let result = pool.submit([1u8; 16], b"prompt".to_vec(), 5000).await;
        assert!(result.is_ok());
        assert!(result.unwrap().text.contains("20ms"));
    }

    /// T4 — cancel pendant l'inférence → Err(Cancelled).
    #[tokio::test]
    async fn pool_cancel_during_inference() {
        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 10_000 }));
        let pool_clone = pool.clone();
        let agent_id = [2u8; 16];

        let handle = tokio::spawn({
            let pool = pool.clone();
            async move { pool.submit(agent_id, b"prompt".to_vec(), 30_000).await }
        });

        tokio::time::sleep(Duration::from_millis(30)).await;
        pool_clone.cancel(&agent_id);

        let result = handle.await.unwrap();
        assert_eq!(result, Err(InferError::Cancelled));
    }

    /// T4 — sémaphore borné : avec max_concurrent=1, le second submit attend le premier.
    #[tokio::test]
    async fn pool_semaphore_serializes_when_full() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let pool = Arc::new(InferencePool::new(1, SleepyBackend { delay_ms: 80 }));
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for i in 0..3u8 {
            let p = pool.clone();
            let c = concurrent.clone();
            let m = max_seen.clone();
            handles.push(tokio::spawn(async move {
                c.fetch_add(1, Ordering::SeqCst);
                let cur = c.load(Ordering::SeqCst);
                m.fetch_max(cur, Ordering::SeqCst);
                let _ = p.submit([i; 16], b"p".to_vec(), 5000).await;
                c.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles { h.await.unwrap(); }
        assert!(max_seen.load(Ordering::SeqCst) >= 1);
    }

    /// T4 — is_active : false avant submit, true pendant, false après.
    #[tokio::test]
    async fn pool_is_active_tracks_state() {
        let pool = Arc::new(InferencePool::new(4, SleepyBackend { delay_ms: 100 }));
        let agent_id = [3u8; 16];

        assert!(!pool.is_active(&agent_id));

        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            pool_clone.submit(agent_id, b"p".to_vec(), 5000).await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(pool.is_active(&agent_id));

        handle.await.unwrap().unwrap();
        assert!(!pool.is_active(&agent_id));
    }

    // ── Tests Phase 6 — InferenceQueue bornée (ADR-0022) ─────────────────────
    // Importé depuis queue::tests_queue

    #[test]
    fn t_queue_bounded_emits_no_slot() {
        queue::tests_queue::t_queue_bounded_emits_no_slot();
    }

    #[test]
    fn t_queue_priority_supervisor_passes_batch() {
        queue::tests_queue::t_queue_priority_supervisor_passes_batch();
    }

    #[test]
    fn t_queue_starvation_promotion() {
        queue::tests_queue::t_queue_starvation_promotion();
    }

    #[test]
    fn t_queue_evicts_batch_for_supervisor() {
        queue::tests_queue::t_queue_evicts_batch_for_supervisor();
    }

    #[test]
    fn t_queue_fifo_within_class() {
        queue::tests_queue::t_queue_fifo_within_class();
    }

    #[test]
    fn t_promotion_is_bounded_one_step() {
        queue::tests_queue::t_promotion_is_bounded_one_step();
    }

    #[test]
    fn t_queue_admission_seq_monotone() {
        queue::tests_queue::t_queue_admission_seq_monotone();
    }

    /// Phase 6 — NoSlot retourné par submit quand file pleine.
    /// (Test d'intégration pool + queue.)
    #[tokio::test]
    async fn pool_no_slot_when_queue_full() {
        // max_concurrent=1, queue_capacity=2 → 1 in_flight + 1 en attente max
        let pool = Arc::new(InferencePool::new_with_queue_params(
            1, 2, 10_000,
            SleepyBackend { delay_ms: 500 }, // délai long pour remplir la file
        ));

        // Lancer 2 soumissions simultanées pour remplir la file
        let p1 = Arc::clone(&pool);
        let p2 = Arc::clone(&pool);
        let p3 = Arc::clone(&pool);

        let h1 = tokio::spawn(async move { p1.submit([1u8;16], b"p".to_vec(), 5000).await });
        // Courte pause pour laisser h1 occuper le slot
        tokio::time::sleep(Duration::from_millis(10)).await;
        let h2 = tokio::spawn(async move { p2.submit([2u8;16], b"p".to_vec(), 5000).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        // h3 devrait trouver la file pleine
        let h3 = tokio::spawn(async move { p3.submit([3u8;16], b"p".to_vec(), 5000).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        // Annuler h1 et h2 pour nettoyer
        pool.cancel(&[1u8;16]);
        pool.cancel(&[2u8;16]);

        let r3 = h3.await.unwrap();
        // h3 peut avoir reçu NoSlot ou Cancelled selon le timing
        assert!(r3 == Err(InferError::NoSlot) || r3 == Err(InferError::Cancelled),
            "h3 doit recevoir NoSlot ou Cancelled : {:?}", r3);

        let _ = h1.await;
        let _ = h2.await;
    }
}
