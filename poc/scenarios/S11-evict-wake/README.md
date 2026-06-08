# S11 — Cycle éviction/réveil

**ADR :** ADR-0030 §FutureWork  
**Propriétés :** cycle `evict()` / `wake()` — débloque SchedulerCoordinator  
**Date :** 2026-05-22

---

## Objectif

Valider que le cycle complet éviction → dormant → réveil conserve la continuité causale de chaque agent. Pré-requis pour le `SchedulerCoordinator` décrit dans ADR-0030 §FutureWork.

## Mécanisme

### `Message::Evict { reply }`

Envoyé par le scheduler à un agent actif. La `run_loop` :
1. Émet un événement `Lifecycle::Suspended (0x02)` dans le log causal.
2. Capture l'état minimal (`EvictedState { id, seq, last_snapshot, last_action }`).
3. Envoie l'état via le canal `reply`.
4. Se termine → la tâche Tokio se termine, la mémoire WASM est libérée.

### `Scheduler::evict_agent(agent_id)`

Envoie `Message::Evict`, attend la réponse oneshot, stocke `EvictedState` dans `Scheduler::dormant`.

### `ActorInstance::restore_from_evicted()`

Crée une nouvelle instance WASM avec `AgentState.{seq, last_snapshot, last_action}` restaurés depuis `EvictedState`. Le prochain `commit_barrier` produira un snapshot dont `parent == last_snapshot_before_evict`.

### `Scheduler::wake_agent(agent_id, engine, module, store_ref, log_ref)`

Récupère l'`EvictedState` depuis `dormant`, appelle `restore_from_evicted`, enregistre l'instance avec `register`. Le caller est responsable d'acquérir le permit C2 (`IoAdmissionQueue`) avant d'appeler `wake_agent` (contrat ADR-0030 §D3).

## Propriétés vérifiées

| ID | Description | Seuil |
|----|-------------|-------|
| **P-α** | tous les agents sont dormants après éviction | `dormant_count == n_agents` |
| **P-α post** | table dormant vide après réveil | `dormant_count == 0` |
| **P-β** | état dormant préserve `seq` et `last_snapshot` | `seq == n_actions && last_snapshot.is_some()` |
| **P-γ** | snapshot `last_snapshot` existe dans ContentStore avec `header.seq == seq-1` | lookup `get_header` |
| **P-δ** | log causal contient un événement `Suspended (0x02)` par agent | `suspended_count >= n_agents` |

## Paramètres

| Paramètre | Défaut | Description |
|-----------|--------|-------------|
| `n_agents` | 3 | Nombre d'agents |
| `n_actions` | 10 | Actions par agent avant éviction |
| K_RUNS | 3 | Répétitions |

## Résultats (2026-05-22)

| Propriété | Valeur observée | Statut |
|-----------|-----------------|--------|
| P-α : dormant_after_evict | 3/3 | pass |
| P-α post : dormant_after_wake | 0/3 | pass |
| P-β : seq=10, has_snapshot=true | 3/3 agents | pass |
| P-γ : header.seq == 9 | 3/3 | pass |
| P-δ : suspended_events | 3 | pass |

Verdict : **3/3 pass**

## Portée et limites

- **Permit C2 non géré dans ce scénario** : `wake_agent` est appelé directement sans passer par `IoAdmissionQueue`. En régime réel, le `SchedulerCoordinator` gate le `wake` derrière un permit C2. Le scénario S11 valide le mécanisme éviction/réveil ; la coordination C2 est testée par S10.
- **SchedulerCoordinator non implémenté** : pool d'agents dormants + décision d'admission conjointe C1+C2 reste une FutureWork. Le cycle evict/wake livré ici est la précondition nécessaire.

## Reproductibilité

```bash
cd poc/scenarios/S11-evict-wake
./run.sh
cat report.json
```
