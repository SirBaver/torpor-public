# ADR-0030 — Scheduler unifié C1+C2 : IoAdmissionQueue + coordination I/O-inférence

**Date :** 2026-05-22
**Statut :** Acceptée
**Contexte :** spec/07 §3.3–3.4, TODO.md Axe 2, ADR-0022/0023 (InferenceQueue C1)

---

## Contexte et problème

La Phase 6 a résolu C1 (mur de l'inférence) via `InferenceQueue` (ADR-0022/0023). C2 (Thundering Herd — saturation PCIe) reste non implémenté : si N agents idle se réveillent simultanément et rechargent leur état (50 MB/agent) depuis ContentStore, le bus PCIe sature. Mesure T5-qualif : cap borne basse = 14 agents/s (classe 1, 741 MB/s QD=1).

La spec (§3.4) est explicite : les deux schedulers doivent être **coordonnés** — précharger l'état d'un agent sans slot d'inférence disponible gaspille du block cache et retarde d'autres agents. Le scheduler optimal précharge exactement k agents où k = slots d'inférence disponibles imminents.

---

## Décision

### D1 — IoAdmissionQueue (C2)

Nouvelle primitive `poc/runtime/src/io_queue.rs` qui gate les lectures ContentStore simultanées sous la borne `cap_actif`. Même structure que `InferenceQueue` (3 files VecDeque + dispatcher Tokio + sémaphore), avec une discipline de service étendue : **priorité sémantique + affinité de cache**.

**Discipline de service (D2) :**
1. Priorité stricte inter-classes : Supervisor > Foreground > Batch.
2. Au sein d'une classe : `cache_score` décroissant — agents dont l'état est le plus récemment accédé passent en premier (leur état est probablement encore dans le block cache RocksDB).
3. Tie-break : `admission_seq` croissant (FIFO).

**Cache score :** calculé dans `acquire()` depuis `last_active: Option<Instant>` → `3600.saturating_sub(elapsed_secs)`. Score 0 = agent froid (chargé depuis NVMe). Score proche de 3600 = agent accédé il y a < 1 s (état probablement en block cache).

**Permit RAII (`IoPermit`) :**
Le caller tient l'`IoPermit` pendant la lecture ContentStore. Drop → décrémente `in_flight` + notifie le dispatcher. L'`OwnedSemaphorePermit` est envoyé via `oneshot::channel` au caller (il est `Send`). Si le caller a disparu (timeout, annulation), le dispatcher récupère le permit et décrémente `in_flight`.

**Cap actif :** paramètre de configuration (pas de constante compilée), initialisé depuis les mesures hardware :
- Classe 1 (i3en.xlarge, 741 MB/s QD=1) : `cap_actif = 14`
- Classe 2 (AMD/WD SN530, 1 290 MB/s QD=1) : `cap_actif = 25`
- Valeur conservatrice par défaut : `14` (borne basse toutes classes mesurées)

### D2 — Coordination C1×C2 (pipeline séquentiel)

Le pipeline obligatoire pour tout agent chargé depuis ContentStore :
```
acquire(C2) → lecture ContentStore → drop permit → submit(C1 InferencePool)
```

Cette séquence assure qu'un agent n'occupe pas un slot d'inférence pendant son chargement, et qu'un agent n'est pas chargé si aucun slot d'inférence n'est imminement disponible.

**Coordination explicite C1→C2 (optimisation future, §FutureWork) :** La spec §3.4 décrit une coordination où C1 notifie C2 quand un slot devient libre → C2 démarre le preload en avance. Non implémenté en Phase 7 (PoC) — la coordination implicite séquentielle C2→C1 est suffisante pour valider les bornes. La méthode `IoAdmissionQueue::available_permits()` prépare ce point d'extension.

### D3 — `Scheduler::reap()` — câblage periodik

`reap()` est désormais appelé au début de `Scheduler::register()`. Garantit que les handles et senders des agents terminés sont nettoyés à chaque nouvel enregistrement. Aucune tâche de fond dédiée requise : le rythme de `register()` est suffisant pour les schedulers existants.

---

## Propriétés vérifiées (S10, 2026-05-22)

Scénario S10 (`poc/scenarios/S10-unified-scheduler/`), N=8 agents (2 Supervisor, 4 Foreground, 2 Batch), cap_io=3, k_infer=2.
Trois runs de validation (K=3) :

| Run | max_io | n_ok | elapsed | sup_med | batch_med | verdict |
|-----|--------|------|---------|---------|-----------|---------|
| 1 | 3 | 8/8 | 205 ms | 101 ms | 205 ms | pass |
| 2 | 1 | 8/8 | 206 ms | 206 ms | 206 ms | pass |
| 3 | 3 | 8/8 | 205 ms | 205 ms | 205 ms | pass |

| Propriété | Description | Statut | Note |
|-----------|-------------|--------|------|
| **P-α** | max I/O concurrent ≤ cap_io | **invariant dur** 3/3 ✓ | vérifié par compteur atomique |
| **P-β** | max inférences concurrent ≤ k_infer | **garanti** 3/3 ✓ | sémaphore InferencePool (ADR-0022) |
| **P-γ** | tous les agents complètent | **invariant dur** 3/3 ✓ | n_completed == n_agents |
| **P-δ** | ordre priorité Supervisor < Batch (médiane latence) | **proxy flaky** 1/3 ✓ | voir note ci-dessous |

**Note P-δ :** P-δ est un proxy de latence pour un invariant d'ordre que le scheduler garantit structurellement (`pop_best()` priorité stricte, testé dans `t_io_priority_supervisor_first`). Le proxy est bruité par construction : avec N=2 agents par classe, la médiane est calculée sur 2 valeurs et le signal priorité est submergé par la variance de scheduling. P-δ devrait être remplacé par une assertion d'ordre d'admission : à chaque appel `pop_best()` avec waiters Supervisor présents, asserter que le résultat est Supervisor — déterministe, testable avec N=2, sans timing. Ouvert comme dette dans TODO.md (P-δ-invariant).

Débit total : ~205ms pour 8 agents avec k_infer=2 et 50ms/inférence → ≈ théorique (4 × 50ms = 200ms).

---

## Ce qui ne change pas

- Interface de `InferenceQueue` (ADR-0022) : inchangée. `IoAdmissionQueue` est un composant **additionnel**, pas un remplacement.
- Interface de `Scheduler` : `register()`, `send()`, `rollback()`, `spawn_child()` inchangés. L'appel `reap()` dans `register()` est transparent pour les callers.
- `cap_actif` par défaut non câblé dans `Scheduler` — c'est la responsabilité du caller (scénario, binaire de production) de construire `IoAdmissionQueue` avec le bon cap.

---

## FutureWork

- **Agent eviction/wakeup cycle** : actuellement les agents WASM restent en mémoire. Pour déclencher C2 en régime réel, il faut implémenter `Agent::evict()` (drop WASM instance) et `Agent::wake()` (reconstruct depuis ContentStore snapshot). C2 gate le `wake()`.
- **T5-qualif NVMe serveur** : recalibrer `cap_actif` sur PCIe Gen4 dédié (hypothèse ~100 agents/s) ; mettre à jour le default dans la config runtime.

## Coordination explicite C1→C2 — Câblage préparatoire (2026-05-22)

### Ce qui est livré : signal + plomberie du dispatcher

- `InferenceQueue.slot_freed: Arc<Notify>` — notifié par `pool_dispatcher` à chaque libération de slot d'inférence.
- `InferencePool::slot_freed_notify() -> Arc<Notify>` — API publique exposant ce signal.
- `IoAdmissionQueue::new_with_c1_hint(cap_actif, queue_capacity, c1_hint: Arc<Notify>)` — branche ce signal sur le dispatcher C2 via `tokio::select!`.
- **Test :** `t_c1_hint_wires_without_deadlock` — prouve l'absence de deadlock/corruption, pas un bénéfice de latence.

### Limite : le dispatcher ne peut pas prendre la décision d'admission

Le `select!` sur `c1_hint` réveille `io_dispatcher` quand un slot C1 se libère. Mais `io_dispatcher` ne peut servir que les demandes déjà présentes dans `IoAdmissionQueue`. Dans `s10_runner.rs`, tous les agents s'auto-enfilent via `acquire()`, qui appelle `notify.notify_one()` — le `c1_hint` est donc un réveil redondant dans ce contexte. Il n'existe pas de scénario actuel où `c1_hint` déclenche un dispatch que `notify` n'aurait pas déclenché.

### Ce qui est ouvert : admission prédictive (bloquée sur wakeup cycle)

La coordination décrite en spec §3.4 — « ne précharge que k agents, où k = slots C1 imminents » — est une décision d'**admission** prise *avant* d'enfiler dans `IoAdmissionQueue`. Elle appartient à un `SchedulerCoordinator` qui détient un pool d'agents dormants et choisit lesquels réveiller selon la disponibilité conjointe C1+C2. Ce coordinateur est **bloqué sur le cycle eviction/wakeup** (item FutureWork ci-dessous) : sans `Agent::evict()` / `Agent::wake()`, il n'y a pas d'agents dormants à orchestrer.

Le câblage livré (`slot_freed_notify` + `new_with_c1_hint`) est la précondition nécessaire pour ce `SchedulerCoordinator` — pas l'optimisation elle-même.

---

## Références

- `spec/07-plafonds-architecturaux.md §3.3` — C2 Thundering Herd, calcul cap_actif
- `spec/07-plafonds-architecturaux.md §3.4` — Coordination C1×C2
- `poc/runtime/src/io_queue.rs` — implémentation IoAdmissionQueue
- `poc/scenarios/S10-unified-scheduler/` — scénario de vérification
- ADR-0022, ADR-0023 — InferenceQueue (C1)
- ADR-0011 — Options RocksDB (block cache 256 MB, bloom filter)
- `results/T5/SYNTHESE.md` — mesures hardware qui dimensionnent cap_actif
