# S13 — Persistance d'état après redémarrage (SEF-1)

## Propriété vérifiée

**SEF-1** : Persistance de l'état d'un agent au-delà de la durée de vie du runtime
(spec `benchmarks/equivalence-scenarios.md §SEF-1`).

## Scénario

Le binaire `sef1-runner` exécute deux phases dans un seul run :

### Phase 1 — exécution pré-arrêt

1. ContentStore + CausalLog ouverts sur des chemins disque réels.
2. Agent WASM minimal (`AGENT_WAT`) exécute **N = 100 actions**.  
   Chaque action déclenche `commit_barrier + emit` → un bloc 64 octets + un snapshot + une entrée de log.
3. On capture :
   - `H_before` = `hash_after` du dernier `ActionResult` (= `last_snapshot` de l'agent)
   - `data_hash` = `SnapshotHeader.data_hash` de `H_before`
   - `block_content` = `ContentStore.get_block(data_hash)` (64 octets)
   - `N_log` = nombre total d'`action_id`s dans l'index secondaire
4. Shutdown propre : `drop(tx)` → `run_loop` émet `Terminated` et se termine → `handle.await` → drop des `Arc<ContentStore>` et `Arc<CausalLog>` → verrous fichiers RocksDB libérés.

### Phase 2 — vérification post-redémarrage

Les mêmes chemins sont réouverts (nouveaux `Arc`s, nouvelles instances RocksDB).

| Propriété | Vérification |
|-----------|-------------|
| **P-α** Header intact | `get_header(H_before)` → `Some(header)` avec `seq` et `parent` identiques |
| **P-β** Log intact | `query_by_agent_range(agent_id).len() >= N_log` |
| **P-γ** Bloc identique | `get_block(data_hash)` → mêmes 64 octets bit-à-bit |
| **P-δ** Chaîne causale | `ActorInstance::restore_from_evicted(EvictedState{last_snapshot=H_before})` + une action → `hash_before == H_before` |

## Critère de validation

`K_RUNS = 5` répétitions, toutes pass.

## Exécution

```bash
cd poc
./scenarios/S13-persistence-restart/run.sh
# Ou avec paramètres :
N_ACTIONS=100 K_RUNS=5 ./scenarios/S13-persistence-restart/run.sh
```

## Portée

- **Dans le périmètre** : persistance ContentStore + CausalLog sur NVMe local après arrêt propre du runtime Tokio.
- **Hors périmètre** : power-loss / kernel panic (traité par SEF-4 + ADR-0027) ; migration entre nœuds ; réplication.

## Note sur la baseline

Linux+Docker passe SEF-1 via les volumes Docker correctement configurés. Ce SEF est donc une **métrique de performance** (coût du redémarrage, latence de reprise) plutôt qu'un différenciateur catégoriel — voir `benchmarks/equivalence-scenarios.md §SEF-1 Note sur la baseline`.
