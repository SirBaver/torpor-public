# S10 — Scheduler unifié C1+C2

**ADR :** ADR-0030
**Propriétés :** C2 (I/O Admission Control) + coordination C1×C2
**Date :** 2026-05-22

---

## Objectif

Vérifier que `IoAdmissionQueue` (C2) respecte la borne `cap_actif` sur les lectures ContentStore simultanées, que la priorité sémantique est observable, et que le pipeline C2→C1 complète tous les agents sans erreur.

## Paramètres

| Paramètre | Valeur | Description |
|-----------|--------|-------------|
| `n_agents` | 8 | 2 Supervisor, 4 Foreground, 2 Batch |
| `cap_io` | 3 | `IoAdmissionQueue::cap_actif` (C2) |
| `k_infer` | 2 | `InferencePool::max_concurrent` (C1) |
| `infer_delay_ms` | 50 | Délai mock LLM par inférence |
| K_RUNS | 3 | Répétitions pour conformité |

## Pipeline C2→C1 (ADR-0030 §D2)

```
Pour chaque agent (tous concurrents) :
  1. io_queue.acquire(agent_id, priority, last_active) → IoPermit
  2. ContentStore::get_header(snapshot_hash)              ← I/O réelle
  3. drop IoPermit                                        ← slot C2 libéré
  4. InferencePool::submit(agent_id, prompt, timeout)     ← mock LLM (C1)
```

## Propriétés vérifiées

| ID | Description | Seuil |
|----|-------------|-------|
| **P-α** | max lectures ContentStore simultanées ≤ cap_io | max_io ≤ 3 |
| **P-β** | max inférences simultanées ≤ k_infer | garanti par InferencePool semaphore |
| **P-γ** | tous les agents complètent le pipeline | n_completed = 8 |
| **P-δ** | invariant d'ordre d'admission : à chaque `pop_best()` avec waiters Supervisor présents, le résultat est Supervisor | `sup_chosen_when_present == pop_with_sup_present` |

## Résultats (2026-05-22, post P-δ-invariant)

| Propriété | Valeur observée (K=3) | Statut |
|-----------|----------------------|--------|
| P-α : max_io | 2–3 (≤ cap_io=3) | pass |
| P-γ : n_completed | 8/8 | pass |
| P-δ : pop_with_sup_present / sup_chosen | 2/2 (run1) | pass |
| Durée totale | 203–206 ms (≈ théorique 200 ms) | pass |

## Invariant P-δ — mécanisme

L'ancienne P-δ utilisait un proxy timing (`sup_median < batch_median`) — sensible au jitter Tokio et non falsifiable avec N=2 agents par classe. La P-δ actuelle est un **invariant dur déterministe** :

- `IoQueueState.pop_best()` incrémente `pop_with_sup_present` à chaque appel lorsque la file Supervisor est non vide.
- Si la Supervisor est bien choisie (garanti structurellement par priorité stricte), `sup_chosen_when_present` est également incrémenté.
- L'assertion `sup_chosen_when_present == pop_with_sup_present` est vérifiable sans timing ni N≥5.

Compteurs exposés dans `IoQueueStats` et reportés dans le JSON de chaque run.

## Portée et limites

- **Preload simulé :** les agents WASM restent en mémoire — la lecture ContentStore (`get_header`) est réelle mais légère (une entrée, pas 50 MB). La borne C2 est structurellement correcte ; le calibrage de `cap_actif` sur 50 MB/agent nécessite le cycle evict/wake (FutureWork ADR-0030).
- **Coordination C1→C2 explicite :** non implémentée. Le pipeline C2→C1 séquentiel suffit pour valider les bornes ; la notif C1→C2 est une optimisation future (ADR-0030 §FutureWork).

## Reproductibilité

```bash
cd poc/scenarios/S10-unified-scheduler
./run.sh
cat report.json
```
