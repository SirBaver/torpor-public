# S12 — SchedulerCoordinator : réveil à la demande

**ADR :** ADR-0031  
**Propriétés :** livraison via `Scheduler::deliver` — réveil lazy d'agents dormants  
**Date :** 2026-05-23

---

## Objectif

Valider que `Scheduler::deliver` (ADR-0031 §D1) orchestre correctement le réveil d'agents dormants derrière la gate C2 (`IoAdmissionQueue`), et que les agents actifs reçoivent leurs messages directement sans passer par C2.

## Mécanisme

### `Scheduler::deliver`

Méthode de pré-livraison intégrée au scheduler (ADR-0031 §D3) :

- **Agent actif** : livraison directe via le canal mpsc existant. Pas de C2.
- **Agent dormant** : pipeline complet —
  1. `io_queue.acquire(agent_id, priority, last_active)` — gate C2, `last_active = evicted_at`
  2. `wake_agent(target, engine, module, store, log)` — reconstruction WASM depuis ContentStore
  3. drop `IoPermit` — libère le slot C2
  4. `send(target, msg)` — livraison normale
- **Agent inconnu** : `Err(DeliverError::Unknown)`.

### `EvictedState.evicted_at`

Champ ajouté (ADR-0031 §D4) : capturé dans `Scheduler::evict_agent` au moment où l'état est enregistré dans la table `dormant`. Sert à calculer le `cache_score` pour l'affinité cache de `IoAdmissionQueue`.

### `DeliverError`

- `Unknown` : agent inconnu.
- `IoCongested` : `io_queue.acquire` a retourné `NoSlot` — file C2 saturée.
- `WakeFailed(reason)` : reconstruction WASM échouée.

## Propriétés vérifiées

| ID | Description | Seuil |
|----|-------------|-------|
| **P-α** | Tous les agents dormants ont été réveillés et ont reçu le message | `n_woken == n_dormant && dormant_after_deliver == 0` |
| **P-β** | À aucun moment plus de `cap_io` réveils simultanés | garanti structurellement + `total_rejected == 0` |
| **P-γ** | Les agents actifs reçoivent leurs messages sans passer par C2 | `direct_deliveries == n_active` même quand la file C2 est saturée |

## Paramètres

| Paramètre | Défaut | Description |
|-----------|--------|-------------|
| `n_agents` | 6 | Nombre total d'agents |
| `n_dormant` | 3 | Agents à évincer avant deliver |
| `cap_io` | 2 | Capacité `IoAdmissionQueue` (borne C2) |
| `n_actions` | 5 | Actions par agent avant éviction |
| K_RUNS | 3 | Répétitions |

## Résultats (2026-05-23)

| Propriété | Valeur observée | Statut |
|-----------|-----------------|--------|
| P-α : n_woken / n_dormant | 3/3 | pass |
| P-α : dormant_after_deliver | 0 | pass |
| P-β : io_rejected | 0 | pass |
| P-γ : direct_deliveries / n_active | 3/3 | pass |

Verdict : **3/3 pass**

## Option A (admission prédictive) — FutureWork

ADR-0031 §D2 : l'admission prédictive (préchargement de k agents dormants en avance sur les slots d'inférence disponibles) est différée. Le critère de déclenchement est :

1. S12 (baseline D1) livré et vert. **Satisfait.**
2. Latence p99 de `deliver` sous charge réelle > budget documenté (H-wake-latence, à définir).
3. `InferencePool::available_slots() -> usize` conçu (ADR séparé).

## Reproductibilité

```bash
cd poc/scenarios/S12-scheduler-coordinator
./run.sh
cat report.json
```
