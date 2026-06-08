// InferenceQueue — file d'inférence bornée avec priorité multi-niveau (ADR-0022).
//
// Remplace le `tokio::sync::Semaphore` plat de `InferencePool` par une structure
// mantenant 3 files FIFO indexées par classe (Supervisor > Foreground > Batch).
//
// Discipline de service (ADR-0022 D1) :
//   - Priorité stricte inter-classes : Supervisor avant Foreground avant Batch.
//   - FIFO strict intra-classe sur `admission_seq` (compteur monotone — pas Instant,
//     pour garantir l'ordre même à résolution sub-ms, ADR-0023 §D3).
//   - Garde-fou famine bornée : Batch attendant > max_starvation_ms → promu Foreground
//     (une promotion max par requête, ADR-0022 D1).
//
// Politique de rejet (ADR-0022 D2) :
//   - Quand file pleine + nouvelle requête Supervisor → évince la plus récente Batch en attente.
//   - Sinon : drop-newest, retourne Err(InferError::NoSlot).
//
// Observabilité (ADR-0022 D4, ADR-0023 D3) :
//   - Payload 0x0C enrichi avec priority_class, queue_depth_at_admission, promoted_from.
//   - Trace circulaire QueueTrace (256 entrées) pour assertions S5.
//   - Snapshot synchrone `queue_stats()` pour assertions déterministes.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use super::{InferError, InferResponse};

// ── Types publics ─────────────────────────────────────────────────────────────

/// Classe de priorité d'une requête d'inférence (ADR-0022 D1).
/// Codée sur u8 dans le payload 0x0C enrichi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PriorityClass {
    Supervisor  = 0x01,
    Foreground  = 0x02,
    Batch       = 0x03,
}

impl PriorityClass {
    /// Retourne la classe supérieure (promotion d'un cran, ADR-0022 D1 garde-fou).
    pub fn promoted(self) -> PriorityClass {
        match self {
            PriorityClass::Batch       => PriorityClass::Foreground,
            PriorityClass::Foreground  => PriorityClass::Supervisor,
            PriorityClass::Supervisor  => PriorityClass::Supervisor,
        }
    }
}

/// Statistiques de file exposées pour les assertions de test S5 (ADR-0022 D4, ADR-0023 D3).
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    /// Nombre de requêtes en attente par classe [Supervisor, Foreground, Batch].
    pub waiting: [usize; 3],
    /// Durée d'attente en ms de la requête la plus ancienne par classe.
    pub oldest_wait_ms: [Option<u64>; 3],
    /// Nombre total de requêtes admises depuis la création de la file.
    pub total_admitted: u64,
    /// Nombre total de requêtes refusées (NoSlot) depuis la création de la file.
    pub total_rejected: u64,
    /// Nombre total de promotions famine.
    pub total_promoted: u64,
}

/// Trace d'une requête pour assertions E1/E3 (ADR-0023 D3).
#[derive(Debug, Clone)]
pub struct QueueTrace {
    pub agent_id:                   [u8; 16],
    pub admission_seq:              u64,
    pub admission_instant:          Instant,
    pub priority_class_at_admission: PriorityClass,
    pub promoted_from:              Option<PriorityClass>,
    pub slot_acquired_instant:      Option<Instant>,
}

/// Informations sur l'admission transmises avec le résultat pour enrichir le payload 0x0C.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotAcquiredInfo {
    pub priority_class:            PriorityClass,
    pub queue_depth_at_admission:  u16,
    /// None si pas de promotion, Some(classe_originale) si promotion famine.
    pub promoted_from:             Option<PriorityClass>,
}

// ── Entrée interne ────────────────────────────────────────────────────────────

/// Entrée dans la file d'attente. Le canal `result_tx` transporte le résultat du backend.
pub(super) struct QueueEntry {
    pub(super) agent_id:           [u8; 16],
    pub(super) prompt:             Vec<u8>,
    pub(super) timeout_ms:         u32,
    pub(super) cancel:             CancellationToken,
    /// Compteur monotone d'admission (ADR-0023 §D3).
    pub(super) admission_seq:      u64,
    pub(super) admission_instant:  Instant,
    pub(super) effective_class:    PriorityClass,
    pub(super) original_class:     PriorityClass,
    pub(super) promoted:           bool,
    pub(super) depth_at_admission: u16,
    /// Canal de résultat : rempli par le dispatcher après exécution backend complète.
    pub(super) result_tx: tokio::sync::oneshot::Sender<Result<InferResponse, InferError>>,
}

// ── État interne de la file ───────────────────────────────────────────────────

pub(super) struct QueueState {
    pub(super) supervisor:  VecDeque<QueueEntry>,
    pub(super) foreground:  VecDeque<QueueEntry>,
    pub(super) batch:       VecDeque<QueueEntry>,
    pub(super) in_flight:   u32,
    queue_capacity:    usize,
    // max_concurrent est géré par le Semaphore dans InferenceQueue, pas ici.
    pub(super) max_starvation_ms: u64,
    pub(super) admission_counter: u64,
    // Statistiques
    pub(super) total_admitted: u64,
    pub(super) total_rejected: u64,
    pub(super) total_promoted: u64,
}

impl QueueState {
    pub fn new(max_concurrent: usize, queue_capacity: usize, max_starvation_ms: u64) -> Self {
        // max_concurrent est passé au Semaphore dans InferenceQueue::new() ; non stocké ici.
        let _ = max_concurrent;
        Self {
            supervisor: VecDeque::new(),
            foreground: VecDeque::new(),
            batch: VecDeque::new(),
            in_flight: 0,
            queue_capacity,
            max_starvation_ms,
            admission_counter: 0,
            total_admitted: 0,
            total_rejected: 0,
            total_promoted: 0,
        }
    }

    pub fn total_waiting(&self) -> usize {
        self.supervisor.len() + self.foreground.len() + self.batch.len()
    }

    /// Tente d'admettre une entrée dans la file.
    /// Retourne `Ok(depth_at_admission)` ou `Err(InferError::NoSlot)`.
    pub fn try_enqueue(
        &mut self,
        agent_id: [u8; 16],
        prompt: Vec<u8>,
        timeout_ms: u32,
        cancel: CancellationToken,
        class: PriorityClass,
        result_tx: tokio::sync::oneshot::Sender<Result<InferResponse, InferError>>,
    ) -> Result<u16, InferError> {
        let depth_before = self.total_waiting() as u16;
        let total_entries = self.total_waiting() + self.in_flight as usize;

        if total_entries >= self.queue_capacity {
            // File pleine : tentative d'éviction Batch si Supervisor arrive
            if class == PriorityClass::Supervisor && !self.batch.is_empty() {
                if let Some(evicted) = self.batch.pop_back() {
                    let _ = evicted.result_tx.send(Err(InferError::NoSlot));
                    self.total_rejected += 1;
                    // Admettre le Supervisor
                    self.admission_counter += 1;
                    let seq = self.admission_counter;
                    let entry = QueueEntry {
                        agent_id, prompt, timeout_ms, cancel,
                        admission_seq: seq,
                        admission_instant: Instant::now(),
                        effective_class: class,
                        original_class: class,
                        promoted: false,
                        depth_at_admission: depth_before,
                        result_tx,
                    };
                    self.supervisor.push_back(entry);
                    self.total_admitted += 1;
                    return Ok(depth_before);
                }
            }
            self.total_rejected += 1;
            return Err(InferError::NoSlot);
        }

        self.admission_counter += 1;
        let seq = self.admission_counter;
        let entry = QueueEntry {
            agent_id, prompt, timeout_ms, cancel,
            admission_seq: seq,
            admission_instant: Instant::now(),
            effective_class: class,
            original_class: class,
            promoted: false,
            depth_at_admission: depth_before,
            result_tx,
        };
        match class {
            PriorityClass::Supervisor => self.supervisor.push_back(entry),
            PriorityClass::Foreground => self.foreground.push_back(entry),
            PriorityClass::Batch      => self.batch.push_back(entry),
        }
        self.total_admitted += 1;
        Ok(depth_before)
    }

    /// Applique les promotions famine avant de dépiler.
    pub fn apply_starvation_promotions(&mut self) {
        let threshold_ms = self.max_starvation_ms;
        let now = Instant::now();

        // Batch → Foreground
        let mut promoted_to_fg = Vec::new();
        let mut remaining_batch = VecDeque::new();
        while let Some(mut entry) = self.batch.pop_front() {
            let wait_ms = now.duration_since(entry.admission_instant).as_millis() as u64;
            if !entry.promoted && wait_ms >= threshold_ms {
                entry.promoted = true;
                entry.effective_class = PriorityClass::Foreground;
                promoted_to_fg.push(entry);
                self.total_promoted += 1;
            } else {
                remaining_batch.push_back(entry);
            }
        }
        self.batch = remaining_batch;
        for entry in promoted_to_fg.into_iter().rev() {
            self.foreground.push_front(entry);
        }

        // Foreground → Supervisor (uniquement les Foreground originaux, pas les Batch promus)
        let mut promoted_to_sup = Vec::new();
        let mut remaining_fg = VecDeque::new();
        while let Some(mut entry) = self.foreground.pop_front() {
            let wait_ms = now.duration_since(entry.admission_instant).as_millis() as u64;
            // Un Batch promu en Foreground ne peut pas être promu à nouveau (promoted=true)
            if !entry.promoted && entry.original_class == PriorityClass::Foreground && wait_ms >= threshold_ms {
                entry.promoted = true;
                entry.effective_class = PriorityClass::Supervisor;
                promoted_to_sup.push(entry);
                self.total_promoted += 1;
            } else {
                remaining_fg.push_back(entry);
            }
        }
        self.foreground = remaining_fg;
        for entry in promoted_to_sup.into_iter().rev() {
            self.supervisor.push_front(entry);
        }
    }

    /// Dépile la prochaine entrée selon la priorité stricte.
    pub fn pop_next(&mut self) -> Option<QueueEntry> {
        self.apply_starvation_promotions();
        if let Some(e) = self.supervisor.pop_front() { return Some(e); }
        if let Some(e) = self.foreground.pop_front() { return Some(e); }
        if let Some(e) = self.batch.pop_front()      { return Some(e); }
        None
    }

    pub fn stats(&self) -> QueueStats {
        let now = Instant::now();
        let oldest = |q: &VecDeque<QueueEntry>| {
            q.front().map(|e| now.duration_since(e.admission_instant).as_millis() as u64)
        };
        QueueStats {
            waiting: [self.supervisor.len(), self.foreground.len(), self.batch.len()],
            oldest_wait_ms: [oldest(&self.supervisor), oldest(&self.foreground), oldest(&self.batch)],
            total_admitted: self.total_admitted,
            total_rejected: self.total_rejected,
            total_promoted: self.total_promoted,
        }
    }
}

// ── InferenceQueue — façade publique ─────────────────────────────────────────

/// File d'inférence bornée avec priorité multi-niveau (ADR-0022).
/// Utilisée par `InferencePool<B>` comme remplaçant du `Semaphore` plat.
pub struct InferenceQueue {
    pub(super) state:       Arc<Mutex<QueueState>>,
    pub(super) notify:      Arc<Notify>,
    /// Sémaphore Tokio limitant les inférences simultanées.
    /// Le dispatcher l'acquiert avant de spawner chaque inférence.
    pub(super) semaphore:   Arc<Semaphore>,
    /// Traces circulaires (256 entrées max) pour assertions E1/E3 dans S5.
    pub(super) traces:      Arc<Mutex<VecDeque<QueueTrace>>>,
    /// Notifié à chaque libération d'un slot d'inférence (ADR-0030 §FutureWork).
    /// Permet à IoAdmissionQueue de déclencher le préchargement C2 dès qu'un
    /// slot C1 se libère, sans attendre qu'un nouveau job soit soumis à l'inférence.
    pub(super) slot_freed:  Arc<Notify>,
}

impl InferenceQueue {
    /// Crée une nouvelle file (sans dispatcher — l'intégration avec le backend
    /// est faite par `InferencePool<B>::submit`).
    pub fn new(max_concurrent: usize, queue_capacity: usize, max_starvation_ms: u64) -> Self {
        let state = Arc::new(Mutex::new(QueueState::new(max_concurrent, queue_capacity, max_starvation_ms)));
        let notify = Arc::new(Notify::new());
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let traces = Arc::new(Mutex::new(VecDeque::with_capacity(256)));
        let slot_freed = Arc::new(Notify::new());
        Self { state, notify, semaphore, traces, slot_freed }
    }

    /// Retourne les statistiques courantes de la file.
    pub fn stats(&self) -> QueueStats {
        self.state.lock().unwrap().stats()
    }

    /// Retourne une copie des traces récentes (ADR-0023 D3).
    pub fn traces(&self) -> Vec<QueueTrace> {
        self.traces.lock().unwrap().iter().cloned().collect()
    }

    /// Annule les inférences en attente pour `agent_id`.
    pub fn cancel_agent(&self, agent_id: &[u8; 16]) {
        let st = self.state.lock().unwrap();
        for entry in st.supervisor.iter().chain(st.foreground.iter()).chain(st.batch.iter()) {
            if &entry.agent_id == agent_id {
                entry.cancel.cancel();
            }
        }
    }
}

// ── Tests unitaires TDD (Phase 6 Semaine 1) ─────────────────────────────────

#[cfg(test)]
pub(super) mod tests_queue {
    use super::*;
    use std::time::Duration;

    pub fn make_state(max_concurrent: usize, queue_capacity: usize) -> QueueState {
        QueueState::new(max_concurrent, queue_capacity, 10_000)
    }

    pub fn make_state_starvation(max_concurrent: usize, queue_capacity: usize, starvation_ms: u64) -> QueueState {
        QueueState::new(max_concurrent, queue_capacity, starvation_ms)
    }

    fn dummy_entry(class: PriorityClass, id: u8) -> (
        tokio::sync::oneshot::Sender<Result<InferResponse, InferError>>,
        tokio::sync::oneshot::Receiver<Result<InferResponse, InferError>>,
        [u8; 16],
        CancellationToken,
    ) {
        let agent_id = [id; 16];
        let cancel = CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        (tx, rx, agent_id, cancel)
    }

    fn enqueue(q: &mut QueueState, class: PriorityClass, id: u8) -> (
        [u8; 16],
        tokio::sync::oneshot::Receiver<Result<InferResponse, InferError>>,
    ) {
        let agent_id = [id; 16];
        let cancel = CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = q.try_enqueue(agent_id, b"p".to_vec(), 5000, cancel, class, tx);
        (agent_id, rx)
    }

    fn enqueue_result(q: &mut QueueState, class: PriorityClass, id: u8) -> Result<u16, InferError> {
        let agent_id = [id; 16];
        let cancel = CancellationToken::new();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        q.try_enqueue(agent_id, b"p".to_vec(), 5000, cancel, class, tx)
    }

    /// t_queue_bounded_emits_no_slot — file à capacité, submit → NoSlot.
    #[test]
    pub fn t_queue_bounded_emits_no_slot() {
        // max_concurrent=2, queue_capacity=4 → 2 in_flight + 2 en attente max
        let mut q = make_state(2, 4);
        q.in_flight = 2;

        assert!(enqueue_result(&mut q, PriorityClass::Foreground, 1).is_ok());
        assert!(enqueue_result(&mut q, PriorityClass::Foreground, 2).is_ok());

        let result = enqueue_result(&mut q, PriorityClass::Foreground, 3);
        assert_eq!(result, Err(InferError::NoSlot), "file pleine → NoSlot");
    }

    /// t_queue_priority_supervisor_passes_batch — Supervisor servi avant Batch.
    #[test]
    pub fn t_queue_priority_supervisor_passes_batch() {
        let mut q = make_state(4, 16);

        for i in 0..5u8 {
            enqueue(&mut q, PriorityClass::Batch, i);
        }
        enqueue(&mut q, PriorityClass::Supervisor, 10);

        let next = q.pop_next().expect("file non vide");
        assert_eq!(next.effective_class, PriorityClass::Supervisor,
            "Supervisor doit passer devant les Batch");
        assert_eq!(next.agent_id, [10u8; 16]);
    }

    /// t_queue_fifo_within_class — ordre FIFO strict intra-classe.
    #[test]
    pub fn t_queue_fifo_within_class() {
        let mut q = make_state(4, 16);

        enqueue(&mut q, PriorityClass::Foreground, 10);
        enqueue(&mut q, PriorityClass::Foreground, 11);
        enqueue(&mut q, PriorityClass::Foreground, 12);

        let e1 = q.pop_next().unwrap();
        let e2 = q.pop_next().unwrap();
        let e3 = q.pop_next().unwrap();

        assert_eq!(e1.agent_id, [10u8; 16]);
        assert_eq!(e2.agent_id, [11u8; 16]);
        assert_eq!(e3.agent_id, [12u8; 16]);
        assert!(e1.admission_seq < e2.admission_seq);
        assert!(e2.admission_seq < e3.admission_seq);
    }

    /// t_queue_starvation_promotion — Batch attendant > max_starvation_ms → promu Foreground.
    #[test]
    pub fn t_queue_starvation_promotion() {
        let mut q = make_state_starvation(4, 16, 1); // 1 ms

        enqueue(&mut q, PriorityClass::Batch, 1);
        enqueue(&mut q, PriorityClass::Batch, 2);

        std::thread::sleep(Duration::from_millis(10));

        q.apply_starvation_promotions();

        assert_eq!(q.batch.len(), 0, "Batch vidé après promotion");
        assert!(q.foreground.len() >= 2);
        assert_eq!(q.total_promoted, 2);

        for e in q.foreground.iter() {
            if e.original_class == PriorityClass::Batch {
                assert!(e.promoted);
                assert_eq!(e.effective_class, PriorityClass::Foreground);
            }
        }
    }

    /// t_queue_evicts_batch_for_supervisor — file pleine de Batch, Supervisor → éviction.
    #[test]
    pub fn t_queue_evicts_batch_for_supervisor() {
        // queue_capacity=3, max_concurrent=1 → 1 in_flight + 2 en attente
        let mut q = make_state(1, 3);
        q.in_flight = 1;

        let (_, mut rx_b1) = enqueue(&mut q, PriorityClass::Batch, 1);
        let (_, mut rx_b2) = enqueue(&mut q, PriorityClass::Batch, 2);

        // File pleine, Supervisor arrive → éviction du Batch le plus récent (2)
        let result = enqueue_result(&mut q, PriorityClass::Supervisor, 10);
        assert!(result.is_ok(), "Supervisor admis par éviction");

        assert_eq!(q.supervisor.len(), 1);
        assert_eq!(q.batch.len(), 1);
        assert_eq!(q.total_rejected, 1);
        assert_eq!(q.total_admitted, 3);

        // rx_b2 doit avoir reçu NoSlot
        match rx_b2.try_recv() {
            Ok(Err(InferError::NoSlot)) => {},
            other => panic!("Batch évincé doit recevoir NoSlot, got: {:?}", other),
        }
        // rx_b1 toujours en attente (pas évincé)
        assert!(rx_b1.try_recv().is_err(), "Batch 1 pas encore servi");
    }

    /// t_promotion_is_bounded_one_step — une requête promue ne peut pas l'être à nouveau.
    #[test]
    pub fn t_promotion_is_bounded_one_step() {
        let mut q = make_state_starvation(4, 16, 1);

        enqueue(&mut q, PriorityClass::Batch, 1);
        std::thread::sleep(Duration::from_millis(10));
        q.apply_starvation_promotions();
        assert_eq!(q.total_promoted, 1);
        assert_eq!(q.foreground.len(), 1);

        // 2ème promotion : ne doit pas upgrader vers Supervisor
        std::thread::sleep(Duration::from_millis(10));
        q.apply_starvation_promotions();
        assert_eq!(q.total_promoted, 1, "pas de 2ème promotion");
        assert_eq!(q.supervisor.len(), 0);
    }

    /// t_queue_admission_seq_monotone — l'admission_seq est toujours croissant.
    #[test]
    pub fn t_queue_admission_seq_monotone() {
        let mut q = make_state(4, 16);
        for i in 0..5u8 {
            enqueue(&mut q, PriorityClass::Foreground, i);
        }
        let mut seqs = Vec::new();
        while let Some(e) = q.pop_next() {
            seqs.push(e.admission_seq);
        }
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    }
}
