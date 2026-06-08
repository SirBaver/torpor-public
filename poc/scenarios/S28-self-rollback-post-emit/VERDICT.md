# S28 — Rollback tentative post-`emit` refusée

**Date :** 2026-06-03  
**Test :** `cargo test -p os-poc-runtime --release --lib -- s28_self_rollback_post_emit_refused --nocapture`  
**Verdict : PASS**

---

## Setup

| Paramètre | Valeur |
|-----------|--------|
| Agent | `SELF_ROLLBACK_AGENT_WAT` — émet 1 action puis tente `agent_self_rollback(depth=1)` |
| Condition | seq=1, depth=1 → seq(1) < 1+depth(1)=2 → historique insuffisant → code retour -3 |
| Substrat | Linux (D7 : verdict non transférable seL4) |

## Oracle P2

| Invariant | Résultat |
|-----------|----------|
| `actor.last_snapshot()` inchangé après tentative de rollback | **PASS** |
| Aucun `SelfRollback (0x07)` dans le log | **PASS** |
| Exactement 1 entrée dans le log (l'emit initial uniquement) | **PASS** |

## Finding

`agent_self_rollback` retourne `-3` (historique insuffisant) quand `seq ≤ depth`. Le refus est silencieux côté log (aucune trace) et sans effet sur le snapshot. Le commit barrier positionné dans `emit()` côté runtime est infranchissable depuis l'agent WASM.

**Propriété P2 tenue.**
