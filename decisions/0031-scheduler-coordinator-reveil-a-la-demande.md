# ADR-0031 — SchedulerCoordinator : réveil à la demande (baseline) avant admission prédictive

**Date :** 2026-05-23
**Statut :** Acceptée
**Contexte :** ADR-0030 §FutureWork (admission prédictive), TODO Axe 2 (SchedulerCoordinator), spec/07 §3.4 (coordination C1×C2)

---

## Contexte et problème

ADR-0030 a livré le pipeline séquentiel C2→C1 (`IoAdmissionQueue::acquire` → lecture ContentStore → `InferencePool::submit`) et le câblage préparatoire C1→C2 (`slot_freed_notify` + `new_with_c1_hint`). Le cycle d'éviction/réveil a été livré séparément (ADR-0030 §FutureWork débloqué, scénario S11 : `Scheduler::evict_agent`, `wake_agent`, table `dormant`).

Reste ouvert dans TODO Axe 2 : un **SchedulerCoordinator** qui orchestre le réveil d'agents dormants en fonction de la disponibilité conjointe C1+C2. La question bloquante était formulée ainsi : *window slot C1 (admission prédictive) vs demande à la volée (réveil sur message entrant) ?*

Sans tranchage, le pool d'agents dormants ne sert à rien : `evict_agent` est appelable, mais aucun chemin opérationnel ne déclenche `wake_agent`. Le cycle est inerte.

---

## Décision

### D1 — Réveil à la demande (Option B) comme baseline opérationnel

Le `SchedulerCoordinator` adopte la discipline **lazy wakeup** :

- Le scheduler conserve une table `dormant: HashMap<AgentId, EvictedState>` (déjà livrée).
- Tout point d'entrée externe (`send`, `send_caused_by`, `checkpoint`, `respond_validation`, etc.) vérifie d'abord `is_dormant(target)`.
- Si l'agent cible est dormant, le coordinateur exécute le pipeline complet **avant** la livraison du message :

```text
1. io_queue.acquire(agent_id, priority, last_active) → IoPermit       [C2]
2. wake_agent(target, engine, module, store, log)                     [reconstruction]
3. drop IoPermit
4. send(target, msg)                                                  [livraison]
```

Si `acquire` retourne `NoSlot`, le caller reçoit `Err(WakeError::IoCongested)` — pas de file d'attente côté coordinateur, la backpressure est exposée explicitement.

### D2 — Pas d'admission prédictive en Phase 7

L'**admission prédictive** (Option A — précharger `k = available_inference_slots()` agents dormants en avance) est explicitement reportée :

- Aucune API `InferencePool::available_slots() -> usize` n'est exposée actuellement (seul `slot_freed_notify: Arc<Notify>` existe, signal binaire pas jauge).
- Sans baseline (D1), le bénéfice de A n'est pas mesurable. On ne peut pas réfuter l'argument *« le préchargement anticipé masque la latence »* sans nombre de référence.
- A nécessite une politique de sélection des dormants (FIFO ? LRU ? priorité ? deadline ?) qui demande à elle seule un ADR séparé.

L'option A est conservée comme `FutureWork` ; le critère de déclenchement est : *latence de réveil mesurée sous charge réelle > budget cible documenté*. Sans mesure, pas de complexité ajoutée.

### D3 — Interface publique du coordinateur

Le coordinateur **n'est pas** un nouveau composant Tokio long-running. C'est une **fonction de pré-livraison** intégrée à `Scheduler` :

```rust
impl Scheduler {
    /// Livre `msg` à `target`, réveillant l'agent depuis ContentStore si dormant.
    ///
    /// Pipeline en cas d'agent dormant (ADR-0031 §D1) :
    ///   1. io_queue.acquire (gate C2)
    ///   2. wake_agent (reconstruction depuis EvictedState)
    ///   3. drop IoPermit (libère le slot C2)
    ///   4. send (livraison normale)
    ///
    /// Sur agent actif : équivalent à `send` (pas d'overhead).
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
    ) -> Result<(), DeliverError>;
}
```

`deliver` est le seul chemin de livraison qui garantit la cohérence C2-gate avant wakeup. Les méthodes existantes (`send`, `send_caused_by`, etc.) restent valides pour les **agents actifs** (callers qui savent que `target` n'est pas dormant — cas des scénarios sans cycle evict).

`DeliverError` distingue trois cas :
- `Unknown(target)` : ni actif ni dormant — agent inconnu.
- `IoCongested` : `io_queue.acquire` retourne `NoSlot`. Le caller décide (retry, drop, escalade).
- `WakeFailed(reason)` : `restore_from_evicted` a échoué (corruption ContentStore, snapshot absent). Erreur dure.

### D4 — Politique de cache_score au réveil

`io_queue.acquire(agent_id, priority, last_active)` attend un `Option<Instant>` pour calculer l'affinité cache. Pour un agent dormant :

- `last_active` = `EvictedState.evicted_at: Instant` (timestamp de l'éviction, stocké au moment du `evict_agent`).
- Plus l'éviction est récente, plus le `cache_score` est élevé (les blocs ContentStore associés ont plus de chances d'être encore dans le block cache RocksDB).
- Si `EvictedState.evicted_at` n'est pas trackée actuellement, le coordinateur passe `None` (agent froid, score=0). **Action :** ajouter le champ à `EvictedState` lors de l'implémentation.

---

## Propriétés à vérifier (scénario S12, à produire)

Le scénario S12 (`poc/scenarios/S12-wake-on-demand/`) est la dette d'implémentation associée à cet ADR. Trois propriétés :

| Propriété | Description | Mode de vérification |
|-----------|-------------|---------------------|
| **P-α** | `deliver` sur agent dormant passe par `io_queue.acquire` avant `wake_agent` | invariant testable via compteur dans le coordinateur (incrémenté entre acquire et wake) |
| **P-β** | Sous saturation C2 (cap_io=2, N=5 wakeups concurrents), au plus `cap_io` `restore_from_evicted` simultanés | atomic counter + max observé ≤ cap_io |
| **P-γ** | Le message livré post-wake est bien causé par l'état pré-éviction (continuité causale C2→reconstruction→livraison) | hash du commit post-wake parent == `EvictedState.last_snapshot` |

P-γ recoupe P-γ de S11 ; la nouveauté de S12 est la combinaison **acquire(C2) + wake + send** sous concurrence.

---

## Ce qui ne change pas

- ADR-0030 reste valide intégralement : le pipeline C2→C1 et le câblage préparatoire C1→C2 sont la fondation sur laquelle D1 s'appuie.
- L'`IoAdmissionQueue` n'est pas modifiée. Le coordinateur l'utilise telle quelle.
- L'`InferencePool` n'est pas modifié. La coordination C1×C2 reste séquentielle (`acquire(C2) → drop → submit(C1)`) — la coordination prédictive (Option A) est différée.
- `Scheduler::send`, `send_caused_by`, etc. : interface inchangée. Un nouveau chemin `deliver` les complète, ne les remplace pas.

---

## Conséquences

### Sur le code

- `Scheduler::deliver` : nouvelle méthode (~50 lignes).
- `EvictedState.evicted_at: Instant` : nouveau champ (rétro-compatible — initialisé par `evict_agent` au moment où il insère dans `dormant`).
- `DeliverError` : nouveau type d'erreur.
- Scénario S12 : runner + run.sh + report.json (sur le modèle S10/S11).

### Sur le modèle de coût

Pour un agent dormant, `deliver` ajoute :
- 1 round-trip d'acquisition `IoPermit` (latence dominée par contention C2 — bornée par `cap_actif`)
- 1 lecture ContentStore + reconstruction WASM (latence ≈ T5-bis p50 ≈ 1 ms + cold start WASM)
- 1 register dans le scheduler

Borne supérieure attendue sous `cap_io = 14` (classe 1) et `N = 100` wakeups burst : 100/14 ≈ 7 s sériel. Compatible avec la borne C2 publiée (spec/07 §3.3).

### Sur la complexité

D1 ajoute **une méthode** au scheduler, **un champ** à `EvictedState`, **un type d'erreur**. Pas de tâche de fond, pas de file additionnelle, pas de politique de sélection — par construction, le réveil est déclenché par la livraison externe, pas par une heuristique interne.

---

## Critère de déclenchement de l'Option A (admission prédictive)

L'ADR-0031 sera amendé pour activer l'admission prédictive si **et seulement si** :

1. S12 (D1 baseline) est livré et vert.
2. Une charge réaliste (≥ 100 agents, ratio dormants/actifs ≥ 10:1) montre une latence p99 de `deliver` qui dépasse un budget documenté (à fixer dans `spec/04-hypotheses.md` lors de l'introduction de `H-wake-latence`).
3. Une instrumentation `InferencePool::available_slots() -> usize` est conçue (ADR séparé).

Tant que ces trois conditions ne sont pas réunies, l'admission prédictive reste hypothétique et ne justifie pas l'ajout de complexité.

---

## Références

- ADR-0030 — Scheduler unifié C1+C2 (pipeline séquentiel C2→C1, FutureWork)
- `spec/07-plafonds-architecturaux.md §3.4` — Coordination C1×C2
- `poc/runtime/src/io_queue.rs` — `IoAdmissionQueue::acquire`
- `poc/runtime/src/scheduler.rs` — `evict_agent`, `wake_agent`, table `dormant`
- `poc/scenarios/S11-evict-wake/` — cycle eviction/wakeup (précondition de S12)
- TODO.md Axe 2 §SchedulerCoordinator — dette soldée par cet ADR + S12 (à produire)
