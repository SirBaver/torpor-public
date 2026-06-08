# S4 — Rollback scheduler + révocation caps (D5 + D8)

## Ce qui est testé

Un agent (`rollback_target`) est rollbacké par le scheduler **pendant
qu'une inférence est en cours**. Les capabilities accordées après le
snapshot cible sont révoquées. Démontre :

- **Q5.1 ADR-0019** : un rollback pendant `WaitingInference` annule
  proprement la `Future` d'inférence (`CancellationToken`) et trace
  `InferenceCancelled (0x0E)`.
- **D8 ADR-0007** : les caps émises strictement après le snapshot cible
  sont retirées du store.
- **D5 (résolu, cf. L43)** : `Message::Rollback` est traité dans
  `run_loop` après l'annulation, l'agent reste `Active`.

## Acteur

| Nom | Source | Rôle |
|-----|--------|------|
| `rollback_target` | `agent-sdk/examples/rollback_target.rs` | Construit l'historique (phase 0x01), puis lance une inférence longue (phase 0x02) |

## Protocole

```
Phase 0x01 — construit l'historique (snapshot S1)
  barrier + emit "history:pre_rollback_target"

Phase 0x02 — lance une inférence longue (SleepyBackend 60s en test)
  match infer("Long computation - this will be cancelled.") {
    Ok(n)          → barrier + emit résultat + terminate
    Err(CANCELLED) → retourner SANS terminate (le run_loop doit
                     consommer Message::Rollback pour tracer
                     SchedulerRollback(0x0B) et révoquer les caps)
    Err(other)     → barrier + emit "infer_error" + terminate
  }
```

**Invariant critique.** Quand `infer()` retourne `INFER_CANCELLED`,
l'agent ne doit **pas** appeler `terminate()`. Le `Message::Rollback`
est dans l'inbox du `run_loop` ; si l'agent se termine prématurément,
le message n'est jamais traité, la cap n'est jamais révoquée, et le
log causal n'a pas son `SchedulerRollback`.

## Séquence orchestrée par le test

1. Phase 0x01 → snapshot **S1** (`seq=0`).
2. Grant cap **C1** à l'agent (après S1).
3. Phase 0x02 → l'agent entre en `WaitingInference`.
4. **80 ms** après → `scheduler.rollback(&agent, 0)` :
   - `cancel_fn(&agent_id)` → `CancellationToken` déclenché.
   - `inbox.send(Message::Rollback { target_seq: 0 })` → traité après
     `Cancelled` côté agent.

## Assertions

- `pool.is_active(agent_id) == false` après rollback.
- Cap **C1 absente du store** (D8 ADR-0007).
- `InferenceCancelled (0x0E)` présent dans le log.
- `SchedulerRollback (0x0B)` présent dans le log.
- `payload[9]` du `SchedulerRollback` = `caps_invalidated ≥ 1`.

## Ce qui n'est PAS testé

- **Rollback simultané de plusieurs agents.** Un seul agent ici.
- **Rollback en cascade** (parent rollbacké → enfants rollbacks).
- **Crash du runtime entre `cancel_fn` et le traitement du
  `Message::Rollback`.** Atomicité crash hors scope.
- **Révocation des caps **délégées** à d'autres agents** avant le
  rollback. ADR-0007 §propagation : couvert par l'invalidation
  lazy `check()` mais non observable en S4 (un seul agent).
- **Annulation d'une inférence **non bloquée** par sémaphore** (cas où
  le slot vient d'être acquis mais le SleepyBackend n'a pas encore
  démarré son sleep). Race théorique, non testée.
- **Bornes de temps** sur la durée totale du rollback (depuis `cancel_fn`
  jusqu'à `SchedulerRollback` émis).

## Comment relancer

```sh
cd poc/
export CXXFLAGS="-include cstdint"
cargo build --target wasm32-unknown-unknown -p agent-sdk --examples --release
cargo test -p os-poc-runtime --release -- tests::s4_scheduler_rollback --exact
```

## Prérequis

- Rust + cible `wasm32-unknown-unknown`.
- Pas d'Ollama (`SleepyBackend(60s)`).

## Références

- ADR-0007 — invalidation caps post-rollback (D8).
- ADR-0019 §Q5.1 — annulation pendant `WaitingInference`.
- L43 LESSONS — résolution D5 (`Message::Rollback` câblé).
- `poc/runtime/src/lib.rs :: tests::s4_scheduler_rollback`.
