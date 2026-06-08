# S28 — Self-rollback post-emit refusé (UC-11 / spec 02c §A2 / P2)

**Régime :** R1 (P2 — rollback, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P2 (ligne de démarcation commit_barrier)** : après `commit_barrier + emit`,
l'action est irréversiblement écrite dans le log causal. La seule voie de retour
est `agent_self_rollback`. Mais si l'agent n'a qu'une seule action dans son
historique (seq=1), le rollback depth=1 est refusé (target_seq = 1−1−1 = négatif).
Le store ne change pas, aucune entrée SelfRollback n'est créée.

### Invariants

- La ligne de démarcation est le `commit_barrier` : avant, aucun effet durable ;
  après + `emit`, l'action est dans le log.
- `agent_self_rollback(depth)` nécessite `seq ≥ 1 + depth` pour trouver un
  snapshot cible (target_seq = seq − 1 − depth ≥ 0).
- Le refus est **silencieux** : aucune entrée SelfRollback dans le log,
  `last_snapshot` inchangé.

---

## Protocole

```
Agent (SELF_ROLLBACK_AGENT_WAT)

process([0x00]) → commit_barrier + emit
  → seq = 1, snap0 = store.put_snapshot(...)
  → entry1 appended (Data emit)

process([0x01, 0x01]) → agent_self_rollback(1)
  → check: seq(1) < 1+depth(1)=2 → vrai → retourne -3 (historique insuffisant)

ORACLE 1 : actor.last_snapshot() == snap0  (inchangé)
ORACLE 2 : entries_by_agent → 1 entrée uniquement, aucune SelfRollback (0x07)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Rollback valide (depth=1 après 3 commits) | `a2_self_rollback_valid` |
| Depth > MAX_SELF_ROLLBACK_DEPTH (>3) | `a2_self_rollback_depth_exceeded` |
| Rollback sans aucune history (seq=0) | `a2_self_rollback_no_history` |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s28_self_rollback_post_emit_refused --exact --nocapture
```

---

## Références

- **spec/02c-primitives-agent.md §A2** — agent_self_rollback.
- **ADR-0010** — Protocole emit : commit_barrier comme ligne de démarcation.
- `poc/runtime/src/actor.rs::SELF_ROLLBACK_AGENT_WAT` — module WASM.
- `poc/runtime/src/lib.rs::tests::s28_self_rollback_post_emit_refused` — test.
