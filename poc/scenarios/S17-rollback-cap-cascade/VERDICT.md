# S17 — Rollback + invalidation cap en cascade

**Date :** 2026-06-03  
**Test :** `cargo test -p os-poc-runtime --release --lib -- s17_rollback_cap_cascade --nocapture`  
**Verdict : PASS**

---

## Setup

| Paramètre | Valeur |
|-----------|--------|
| Agents | A (owner C_A) + B (owner C_B déléguée) |
| Profondeur de cascade | 2 niveaux (A → B) |
| Substrat | Linux (D7 : verdict non transférable seL4) |

## Oracle P2 × P4

Après rollback de A vers `seq=0` (avant la délégation) :

| Invariant | Résultat |
|-----------|----------|
| `cap_store.get(C_root)` is `Some` (avant snapshot, non révoquée) | **PASS** |
| `cap_store.get(C_A)` is `None` (émise après snapshot, révoquée) | **PASS** |
| `cap_store.get(C_B)` is `None` (cascade depuis C_A) | **PASS** |
| `cap_store.check(B, C_B, ...)` retourne `false` | **PASS** |
| `SchedulerRollback (0x0B)` dans le log de A | **PASS** |

## Finding

`revoke_owned_after(A, ts_S0)` révoque récursivement C_A et ses dérivées (C_B) via BFS eager. La vérification est re-faite à chaque `check()` sans cache — aucun état obsolète observable. C_root (émise avant S0) n'est pas affectée.

**Note profondeur :** S17 valide la cascade à 1 niveau (A→B). La cascade à profondeur k ≥ 4 est couverte par S29-revoke-recursive-deep.

**Propriété P2 × P4 tenue.**
