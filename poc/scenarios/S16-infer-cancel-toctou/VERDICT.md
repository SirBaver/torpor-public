# S16 — `agent_infer` annulé pendant `WaitingInference`

**Date :** 2026-06-03  
**Test :** `cargo test -p os-poc-runtime --release --lib -- s16_infer_cancel_toctou --nocapture`  
**Verdict : PASS**

---

## Setup

| Paramètre | Valeur |
|-----------|--------|
| POOL_CAP | 4 |
| SleepyBackend delay | 60 000 ms (jamais atteint) |
| Substrat | Linux (D7 : verdict non transférable seL4) |

## Oracle

Deux invariants vérifiés après rollback d'un agent en `WaitingInference` :

| Invariant | Résultat |
|-----------|----------|
| `pool.active_count() == 0` (aucun slot zombie) | **PASS** |
| `pool.available_permits() == POOL_CAP` (slot restauré) | **PASS** |
| Séquence `0x0C < 0x11 < 0x0E < 0x0B < 0x12` dans log | **PASS** |
| `InferenceCancelled (0x0E)` présent dans log | **PASS** |
| `SchedulerRollback (0x0B)` présent dans log | **PASS** |

## Finding

La fenêtre TOCTOU est fermée : `CancellationToken` déclenché avant `Message::Rollback`, slot libéré immédiatement via drop `OwnedSemaphorePermit`. Aucun slot zombie observable sur cette configuration.

**Propriété P2 × C1 tenue.**
