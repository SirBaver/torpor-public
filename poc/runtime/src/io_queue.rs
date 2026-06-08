// IoAdmissionQueue — file d'admission I/O bornée par cap_actif (C2, ADR-0030).
//
// Limite les lectures ContentStore simultanées (chargement d'état d'agent depuis NVMe)
// à `cap_actif = floor(BW_NVMe / état_par_agent)` opérations concurrentes.
//
// Discipline de service (ADR-0030 §D2) :
//   - Priorité stricte inter-classes : Supervisor > Foreground > Batch.
//   - Affinité de cache intra-classe : agents dont l'état est le plus récemment
//     accédé (cache_score élevé) servis en premier — leur état est probablement
//     encore dans le block cache RocksDB.
//   - Ties : admission_seq croissant (FIFO).
//
// Coordination C1×C2 (ADR-0030 §D3) :
//   Le caller du scheduler unifié doit acquérir un permit C2 AVANT de soumettre
//   à l'InferenceQueue (C1). Cela évite de précharger l'état d'un agent qui n'aura
//   pas de slot d'inférence. La coordination explicite C1→C2 (notif quand slot C1
//   imminent) est une optimisation future (ADR-0030 §FutureWork).
//
// Permit RAII (IoPermit) :
//   Le caller tient le permit pendant la lecture ContentStore puis le drop.
//   Drop → décrémente in_flight + notifie le dispatcher pour servir l'entrée suivante.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use crate::actor::AgentId;
use crate::inference::PriorityClass;

// ── Types publics ──────────────────────────────────────────────────────────────

/// Erreur d'admission I/O (ADR-0030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    /// File pleine : in_flight + waiters >= queue_capacity.
    NoSlot,
    /// Dispatcher arrêté (runtime Tokio terminé).
    Cancelled,
}

/// Permit I/O RAII — maintient un slot pendant la lecture ContentStore.
///
/// Drop → décrémente `in_flight` dans `IoQueueState` et notifie le dispatcher
/// pour qu'il serve l'entrée suivante en attente.
pub struct IoPermit {
    _permit: OwnedSemaphorePermit,
    state:   Arc<Mutex<IoQueueState>>,
    notify:  Arc<Notify>,
}

impl Drop for IoPermit {
    fn drop(&mut self) {
        {
            let mut st = self.state.lock().unwrap();
            if st.in_flight > 0 {
                st.in_flight -= 1;
            }
        }
        self.notify.notify_one();
    }
}

// ── Statistiques ───────────────────────────────────────────────────────────────

/// Statistiques exposées pour les assertions de scénario S10.
#[derive(Debug, Clone, Default)]
pub struct IoQueueStats {
    pub waiting:                  [usize; 3],
    pub in_flight:                u32,
    pub total_admitted:           u64,
    pub total_rejected:           u64,
    /// Nombre de fois où `pop_best()` a été appelé alors qu'au moins un waiter
    /// Supervisor était présent dans la file.
    pub pop_with_sup_present:     u64,
    /// Parmi les appels ci-dessus, nombre de fois où le résultat était un Supervisor.
    /// Invariant P-δ dur : `sup_chosen_when_present == pop_with_sup_present`.
    pub sup_chosen_when_present:  u64,
}

// ── Entrée interne ─────────────────────────────────────────────────────────────

#[allow(dead_code)] // agent_id/priority réservés pour cancel_agent et stats futures
struct IoWaiter {
    agent_id:      AgentId,
    priority:      PriorityClass,
    /// Score d'affinité de cache : élevé = état chaud dans le block cache RocksDB.
    /// Calculé à partir de `Instant::elapsed()` depuis le dernier accès ContentStore.
    cache_score:   u64,
    admission_seq: u64,
    result_tx:     tokio::sync::oneshot::Sender<Result<OwnedSemaphorePermit, IoError>>,
}

// ── État interne ───────────────────────────────────────────────────────────────

pub(super) struct IoQueueState {
    supervisor:                   VecDeque<IoWaiter>,
    foreground:                   VecDeque<IoWaiter>,
    batch:                        VecDeque<IoWaiter>,
    pub in_flight:                u32,
    queue_capacity:               usize,
    admission_counter:            u64,
    pub total_admitted:           u64,
    pub total_rejected:           u64,
    pub pop_with_sup_present:     u64,
    pub sup_chosen_when_present:  u64,
}

impl IoQueueState {
    fn new(cap_actif: usize, queue_capacity: usize) -> Self {
        let _ = cap_actif;
        Self {
            supervisor:              VecDeque::new(),
            foreground:              VecDeque::new(),
            batch:                   VecDeque::new(),
            in_flight:               0,
            queue_capacity,
            admission_counter:       0,
            total_admitted:          0,
            total_rejected:          0,
            pop_with_sup_present:    0,
            sup_chosen_when_present: 0,
        }
    }

    fn total_waiting(&self) -> usize {
        self.supervisor.len() + self.foreground.len() + self.batch.len()
    }

    /// Tente d'enregistrer un waiter dans la file.
    /// Retourne `Err(IoError::NoSlot)` si `in_flight + waiters >= queue_capacity`.
    fn try_enqueue(
        &mut self,
        agent_id:    AgentId,
        priority:    PriorityClass,
        cache_score: u64,
        result_tx:   tokio::sync::oneshot::Sender<Result<OwnedSemaphorePermit, IoError>>,
    ) -> Result<(), IoError> {
        let total = self.total_waiting() + self.in_flight as usize;
        if total >= self.queue_capacity {
            self.total_rejected += 1;
            return Err(IoError::NoSlot);
        }
        self.admission_counter += 1;
        let waiter = IoWaiter {
            agent_id,
            priority,
            cache_score,
            admission_seq: self.admission_counter,
            result_tx,
        };
        match priority {
            PriorityClass::Supervisor => self.supervisor.push_back(waiter),
            PriorityClass::Foreground => self.foreground.push_back(waiter),
            PriorityClass::Batch      => self.batch.push_back(waiter),
        }
        self.total_admitted += 1;
        Ok(())
    }

    /// Dépile le meilleur waiter :
    ///   1. Priorité stricte inter-classes (Supervisor > Foreground > Batch).
    ///   2. Au sein d'une classe : cache_score desc (chaud d'abord).
    ///   3. Tie-break : admission_seq asc (FIFO).
    ///
    /// Invariant P-δ : si supervisor non vide, retourne toujours un Supervisor.
    /// Les compteurs `pop_with_sup_present` / `sup_chosen_when_present` permettent
    /// de vérifier cet invariant de manière déterministe (sans timing) en S10.
    fn pop_best(&mut self) -> Option<IoWaiter> {
        let sup_present = !self.supervisor.is_empty();
        if sup_present {
            self.pop_with_sup_present += 1;
            let idx = Self::best_in(&self.supervisor);
            let w = self.supervisor.remove(idx);
            if w.is_some() {
                self.sup_chosen_when_present += 1;
            }
            return w;
        }
        if !self.foreground.is_empty() {
            let idx = Self::best_in(&self.foreground);
            return self.foreground.remove(idx);
        }
        if !self.batch.is_empty() {
            let idx = Self::best_in(&self.batch);
            return self.batch.remove(idx);
        }
        None
    }

    /// Retourne l'indice du meilleur waiter dans `bucket` (cache_score desc, seq asc).
    fn best_in(bucket: &VecDeque<IoWaiter>) -> usize {
        bucket.iter().enumerate()
            .max_by(|(_, a), (_, b)| {
                a.cache_score.cmp(&b.cache_score)
                    // tie : plus petit seq d'abord (FIFO)
                    .then_with(|| b.admission_seq.cmp(&a.admission_seq))
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    pub fn stats(&self) -> IoQueueStats {
        IoQueueStats {
            waiting:                 [self.supervisor.len(), self.foreground.len(), self.batch.len()],
            in_flight:               self.in_flight,
            total_admitted:          self.total_admitted,
            total_rejected:          self.total_rejected,
            pop_with_sup_present:    self.pop_with_sup_present,
            sup_chosen_when_present: self.sup_chosen_when_present,
        }
    }
}

// ── IoAdmissionQueue — façade publique ────────────────────────────────────────

/// File d'admission I/O bornée (C2, ADR-0030).
///
/// Utiliser `acquire()` pour obtenir un `IoPermit` avant toute lecture ContentStore.
/// Tenir le permit pendant la lecture ; le drop libère le slot.
///
/// Coordination C1→C2 (ADR-0030 §FutureWork) : utiliser `new_with_c1_hint` pour
/// brancher la notification de libération de slot C1 (`InferencePool::slot_freed_notify`).
/// Le dispatcher C2 se réveille alors aussi lors de chaque fin d'inférence, ce qui
/// réduit la latence de démarrage du préchargement suivant.
pub struct IoAdmissionQueue {
    state:         Arc<Mutex<IoQueueState>>,
    notify:        Arc<Notify>,
    semaphore:     Arc<Semaphore>,
    /// Cap configuré (nb opérations I/O simultanées max), paramètre de config ADR-0030.
    pub cap_actif: usize,
}

impl IoAdmissionQueue {
    /// Crée une file avec `cap_actif` opérations I/O simultanées.
    ///
    /// `queue_capacity` = cap total (in_flight + waiters). Recommandé : ≥ 4 × cap_actif.
    /// Lance automatiquement le dispatcher en tâche de fond Tokio.
    pub fn new(cap_actif: usize, queue_capacity: usize) -> Self {
        let c1_hint = Arc::new(Notify::new()); // hint non connecté = no-op
        Self::new_with_c1_hint(cap_actif, queue_capacity, c1_hint)
    }

    /// Comme `new`, mais branche le `c1_hint` sur la notification de libération de
    /// slot d'inférence (ADR-0030 §FutureWork).
    ///
    /// Passer `InferencePool::slot_freed_notify()` comme `c1_hint` : le dispatcher C2
    /// se réveillera aussi à chaque fin d'inférence, pas uniquement lors des nouvelles
    /// demandes I/O ou des libérations de permit C2.
    pub fn new_with_c1_hint(cap_actif: usize, queue_capacity: usize, c1_hint: Arc<Notify>) -> Self {
        let state     = Arc::new(Mutex::new(IoQueueState::new(cap_actif, queue_capacity)));
        let notify    = Arc::new(Notify::new());
        let semaphore = Arc::new(Semaphore::new(cap_actif));
        tokio::spawn(io_dispatcher(
            Arc::clone(&state),
            Arc::clone(&notify),
            Arc::clone(&semaphore),
            c1_hint,
        ));
        Self { state, notify, semaphore, cap_actif }
    }

    /// Demande un permit I/O pour le préchargement de l'état de `agent_id`.
    ///
    /// `last_active` : instant du dernier accès ContentStore réussi pour cet agent.
    /// None = agent froid (score 0). Récent = cache chaud (score élevé → priorité cache).
    ///
    /// Bloque (async) jusqu'à l'attribution d'un slot ou `Err(IoError::NoSlot)`
    /// si la file est pleine.
    pub async fn acquire(
        &self,
        agent_id:    AgentId,
        priority:    PriorityClass,
        last_active: Option<Instant>,
    ) -> Result<IoPermit, IoError> {
        let cache_score = last_active
            .map(|t| 3600u64.saturating_sub(t.elapsed().as_secs()))
            .unwrap_or(0);

        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut st = self.state.lock().unwrap();
            st.try_enqueue(agent_id, priority, cache_score, tx)?;
        }
        self.notify.notify_one();

        let permit = match rx.await {
            Ok(Ok(p))  => p,
            Ok(Err(e)) => return Err(e),
            Err(_)     => return Err(IoError::Cancelled),
        };
        Ok(IoPermit {
            _permit: permit,
            state:   Arc::clone(&self.state),
            notify:  Arc::clone(&self.notify),
        })
    }

    /// Statistiques courantes de la file.
    pub fn stats(&self) -> IoQueueStats {
        self.state.lock().unwrap().stats()
    }

    /// Nombre d'opérations I/O actuellement en cours (≤ cap_actif garanti par semaphore).
    pub fn in_flight(&self) -> u32 {
        self.state.lock().unwrap().in_flight
    }

    /// Nombre de slots I/O disponibles immédiatement (0..cap_actif).
    ///
    /// Utilisé par le scheduler unifié pour la coordination C1→C2 (ADR-0030 §D3) :
    /// avant de précharger un agent, vérifier qu'un slot d'inférence (C1) sera bientôt
    /// disponible — `io_queue.available_permits() > 0` est une condition nécessaire mais
    /// pas suffisante. La coordination explicite C1→C2 est une optimisation future.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

// ── Dispatcher ─────────────────────────────────────────────────────────────────
//
// Boucle : attend une notification → essaie d'acquérir un permit sémaphore →
// dépile le meilleur waiter (priorité × affinité cache) → envoie le permit au caller.
// Si l'envoi échoue (caller disparu), décrémente in_flight et re-notifie.
//
// Sources de réveil (select!) :
//   - `notify`   : nouvelle demande I/O enfilée, ou permit C2 libéré (IoPermit::drop)
//   - `c1_hint`  : slot C1 libéré (InferencePool::slot_freed_notify → ADR-0030 §FutureWork)

async fn io_dispatcher(
    state:     Arc<Mutex<IoQueueState>>,
    notify:    Arc<Notify>,
    semaphore: Arc<Semaphore>,
    c1_hint:   Arc<Notify>,
) {
    loop {
        tokio::select! {
            _ = notify.notified()   => {}
            _ = c1_hint.notified()  => {}
        }
        loop {
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p)  => p,
                Err(_) => break,
            };
            let waiter = {
                let mut st = state.lock().unwrap();
                let w = st.pop_best();
                if w.is_some() {
                    st.in_flight += 1;
                }
                w
            };
            match waiter {
                None => {
                    drop(permit);
                    break;
                }
                Some(w) => {
                    match w.result_tx.send(Ok(permit)) {
                        Ok(()) => {}
                        Err(_returned) => {
                            // Caller disparu — libérer le slot immédiatement.
                            let mut st = state.lock().unwrap();
                            if st.in_flight > 0 {
                                st.in_flight -= 1;
                            }
                            notify.notify_one();
                            // _returned (permit) droppé ici → sémaphore libéré.
                        }
                    }
                }
            }
        }
    }
}

// ── Tests unitaires ────────────────────────────────────────────────────────────

#[cfg(test)]
pub(super) mod tests_io_queue {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    // ── Tests sur IoQueueState (sync, pas de Tokio requis) ──────────────────

    fn make_state() -> IoQueueState {
        IoQueueState::new(4, 16)
    }

    fn enqueue_state(
        st:       &mut IoQueueState,
        priority: PriorityClass,
        id:       u8,
        score:    u64,
    ) -> tokio::sync::oneshot::Receiver<Result<OwnedSemaphorePermit, IoError>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = st.try_enqueue([id; 16], priority, score, tx);
        rx
    }

    /// Borne dure : NoSlot quand in_flight + waiters >= queue_capacity.
    #[test]
    fn t_io_queue_rejects_when_full() {
        let mut st = IoQueueState::new(2, 4);
        st.in_flight = 2; // simule 2 in-flight

        let (tx1, _) = tokio::sync::oneshot::channel();
        let (tx2, _) = tokio::sync::oneshot::channel();
        assert!(st.try_enqueue([1; 16], PriorityClass::Foreground, 0, tx1).is_ok());
        assert!(st.try_enqueue([2; 16], PriorityClass::Foreground, 0, tx2).is_ok());

        let (tx3, _) = tokio::sync::oneshot::channel();
        assert_eq!(
            st.try_enqueue([3; 16], PriorityClass::Foreground, 0, tx3),
            Err(IoError::NoSlot),
            "file pleine → NoSlot"
        );
    }

    /// Priorité stricte inter-classes : Supervisor servi avant Foreground avant Batch.
    #[test]
    fn t_io_priority_supervisor_first() {
        let mut st = make_state();
        enqueue_state(&mut st, PriorityClass::Batch,       1, 0);
        enqueue_state(&mut st, PriorityClass::Foreground,  2, 0);
        enqueue_state(&mut st, PriorityClass::Supervisor,  3, 0);

        let best = st.pop_best().unwrap();
        assert_eq!(best.priority, PriorityClass::Supervisor);
        assert_eq!(best.agent_id, [3; 16]);

        let second = st.pop_best().unwrap();
        assert_eq!(second.priority, PriorityClass::Foreground);

        let third = st.pop_best().unwrap();
        assert_eq!(third.priority, PriorityClass::Batch);
    }

    /// Affinité de cache : score élevé = servi en premier au sein d'une classe.
    #[test]
    fn t_io_cache_affinity_observable() {
        let mut st = make_state();
        enqueue_state(&mut st, PriorityClass::Foreground, 1, 0);     // froid
        enqueue_state(&mut st, PriorityClass::Foreground, 2, 3599);  // chaud
        enqueue_state(&mut st, PriorityClass::Foreground, 3, 100);   // tiède

        let best = st.pop_best().unwrap();
        assert_eq!(best.agent_id, [2; 16], "agent chaud (score=3599) servi en premier");

        let second = st.pop_best().unwrap();
        assert_eq!(second.agent_id, [3; 16], "agent tiède (score=100) en deuxième");

        let third = st.pop_best().unwrap();
        assert_eq!(third.agent_id, [1; 16], "agent froid (score=0) en dernier");
    }

    /// FIFO intra-classe à score identique : admission_seq croissant.
    #[test]
    fn t_io_fifo_same_score() {
        let mut st = make_state();
        enqueue_state(&mut st, PriorityClass::Foreground, 10, 500);
        enqueue_state(&mut st, PriorityClass::Foreground, 11, 500);
        enqueue_state(&mut st, PriorityClass::Foreground, 12, 500);

        let e1 = st.pop_best().unwrap();
        let e2 = st.pop_best().unwrap();
        let e3 = st.pop_best().unwrap();
        assert_eq!(e1.agent_id, [10; 16]);
        assert_eq!(e2.agent_id, [11; 16]);
        assert_eq!(e3.agent_id, [12; 16]);
        assert!(e1.admission_seq < e2.admission_seq && e2.admission_seq < e3.admission_seq);
    }

    // ── Test intégration : borne dure observable (async) ────────────────────

    /// Coordination C1→C2 — c1_hint répété n'introduit pas de deadlock.
    ///
    /// Vérifie que le câblage `new_with_c1_hint` est correct : des notifications C1
    /// simultanées à des acquisitions C2 n'entraînent ni panique, ni blocage, ni
    /// corruption du compteur in_flight.
    #[tokio::test]
    async fn t_c1_hint_wires_without_deadlock() {
        let c1_hint = Arc::new(Notify::new());
        let queue   = Arc::new(IoAdmissionQueue::new_with_c1_hint(2, 8, Arc::clone(&c1_hint)));

        // Envoyer des notifications C1 en fond pendant les acquisitions C2
        let hint_bg = Arc::clone(&c1_hint);
        tokio::spawn(async move {
            for _ in 0..6u32 {
                tokio::time::sleep(Duration::from_millis(15)).await;
                hint_bg.notify_one();
            }
        });

        // 4 agents font chacun une acquisition C2 avec un délai (> 2 en parallèle)
        let mut handles = vec![];
        for i in 0..4u8 {
            let q = Arc::clone(&queue);
            handles.push(tokio::spawn(async move {
                let p = q.acquire([i; 16], PriorityClass::Foreground, None).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                drop(p);
            }));
        }
        for h in handles { h.await.unwrap(); }

        // Aucun permit ne doit rester bloqué
        assert_eq!(queue.in_flight(), 0, "in_flight doit être 0 après toutes les acquisitions");
    }

    /// cap_actif respecté : jamais plus de `cap_actif` permits simultanés.
    #[tokio::test]
    async fn t_io_bound_respected() {
        let queue    = Arc::new(IoAdmissionQueue::new(2, 12));
        let inflight = Arc::new(AtomicU32::new(0));
        let max_seen = Arc::new(AtomicU32::new(0));

        let mut handles = vec![];
        for i in 0..5u8 {
            let q = Arc::clone(&queue);
            let c = Arc::clone(&inflight);
            let m = Arc::clone(&max_seen);
            handles.push(tokio::spawn(async move {
                let _permit = q.acquire([i; 16], PriorityClass::Foreground, None).await.unwrap();
                let cur = c.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                // permit dropped ici → slot libéré
            }));
        }
        for h in handles { h.await.unwrap(); }

        let max = max_seen.load(Ordering::SeqCst);
        assert!(max <= 2, "cap_actif=2 respecté, max observé = {max}");
        assert!(max >= 1, "au moins 1 opération en parallèle");
    }
}
