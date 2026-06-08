# S16 — `agent_infer` annulé pendant `WaitingInference` (UC-10 / ADR-0019 §Q5.1 / ADR-0024)

**Régime :** R1+R2
- R1 : rollback (P2) — propriété d'effet, actif partout
- R2 : libération du slot C1 (pool d'inférence) — propriété de ressource, inférence locale uniquement

**Substrat :** Linux. Verdict non transférable à seL4 (D7).

---

## Ce qui est testé

Le cas TOCTOU central : un agent en état `WaitingInference` est ciblé par un
`Scheduler::rollback`. Ce scénario valide deux invariants distincts :

1. **P2 — Rollback** (R1) : l'agent revient à son snapshot cible ; le log causal
   trace la séquence complète de la transaction de compensation.

2. **C1 — Libération immédiate du slot** (R2) : le slot sémaphore d'inférence est
   libéré dès la cancellation, sans zombie. `available_permits()` revient au
   niveau initial.

### Fenêtre TOCTOU ciblée

Entre `InferencePool::submit` (slot acquis, agent en `WaitingInference`) et le
retour de `agent_infer` à l'agent, un `Scheduler::rollback` peut intervenir.
L'invariant est :
- Le `CancellationToken` est déclenché **avant** l'envoi de `Message::Rollback`.
- `InferenceCancelled (0x0E)` est émis par la future Tokio annulée.
- Le slot est libéré immédiatement (OwnedSemaphorePermit drop).
- L'agent reprend avec code retour `Cancelled (4)`.
- `Message::Rollback` est en tête d'inbox, consommé au prochain `recv()`.

### Séquence observable canonique (ADR-0019 §Q5.1 + ADR-0024 D1)

```
[Scheduler] → CompensationOpen (0x11)
[InferPool] → InferenceCancelled (0x0E)
[Scheduler] → SchedulerRollback (0x0B)
[Scheduler] → CompensationClose (0x12)
```

Précédée de `InferenceRequest (0x0C)` émis par l'agent à l'entrée de `agent_infer`.

---

## Acteur(s)

| Nom | Source | Rôle |
|-----|--------|------|
| `rollback_target` | `agent-sdk/examples/rollback_target.rs` | Phase 0x01 : historique ; Phase 0x02 : inférence longue |
| `SleepyBackend` | runtime interne | Simule une inférence de 60 s (annulée avant) |

---

## Protocole

```
Test (main)                              rollback_target              Scheduler
─────────                                ──────────────               ─────────
send(0x01) ─────────────────────────────→ phase_build_history()
                                           barrier(); emit(hist)
wait 50ms
                                                                       register(actor)
send(0x02) ─────────────────────────────→ phase_long_inference()
                                           agent_infer(60s) ───────→ [WaitingInference]
wait 80ms  ← pool.is_active() = true
rollback(target_seq=0) ─────────────────────────────────────────────→ cancel()
                                           ← Cancelled(4)             emit(0x11)
                                                                       emit(0x0E)  ← cancel fut
                                           Message::Rollback ──────→ emit(0x0B)
                                           [rollback applied]          emit(0x12)
wait 400ms

ORACLE :
  pool.active_count() == 0          — aucune inférence active
  pool.available_permits() == k     — slot libéré (anti-zombie)
  séquence ts_ms : 0x0C < 0x11 < 0x0E < 0x0B < 0x12
```

---

## Critère de falsification

Le test échoue (P6 violée) si :
- `pool.active_count() > 0` après rollback → **slot zombie** (C1)
- `pool.available_permits() < POOL_CAP` après rollback → **slot non libéré** (C1)
- Ordre ts_ms incorrect entre les événements → **violation de séquence** (P2/P3)
- `InferenceCancelled` absent du log → **cancellation non tracée**
- `SchedulerRollback` absent du log → **rollback non appliqué**

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Inférence réelle (Ollama) | F1 : SleepyBackend suffisant pour ce cas ; la latence réelle ne change pas l'invariant |
| Plusieurs agents en WaitingInference simultanément | UC-23 (scénario tempête) |
| Power-loss pendant WaitingInference | S15 + UC-23 |
| Capability révoquée pendant WaitingInference | ADR-0019 §Q5.2 : pas de TOCTOU (re-vérif à chaque accès store) |

---

## Comment relancer

```bash
cd poc
# Compiler l'agent WASM (debug suffit pour les tests)
CXXFLAGS="-include cstdint" cargo build --target wasm32-unknown-unknown \
  -p agent-sdk --examples

# Lancer le test
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s16_infer_cancel_toctou --exact --nocapture
```

---

## Prérequis

- `rollback_target.wasm` compilé (partagé avec S4)
- SleepyBackend (pas d'Ollama requis)
- Rust stable, `wasm32-unknown-unknown`

---

## Références

- **ADR-0019 §Q5.1** — Sémantique d'annulation pendant `WaitingInference`.
  Séquence canonique : `cancel → send Rollback → log SchedulerRollback`.
- **ADR-0024 D1** — Journal de compensation (0x11/0x12) encadrant la transaction.
- **ADR-0022** — File d'inférence bornée ; `available_permits()` = invariant C1.
- **S4-scheduler-rollback** — Précurseur : teste la présence des émissions mais pas l'ordre.
- `poc/runtime/src/lib.rs::tests::s16_infer_cancel_toctou` — implémentation.
- `poc/runtime/src/inference/mod.rs::available_permits()` — méthode ajoutée pour S16.
