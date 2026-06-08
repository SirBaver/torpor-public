# S27 — Contrat emit (UC-8 / ADR-0010 / P6 nominal)

**Régime :** R1 (P6 — atomicité crash, chemin nominal, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P6 (chemin nominal)** : la séquence `commit_barrier → store_put → log_append`
maintient la cohérence I-CSR après chaque `process_one`. Toute entrée dans le log
causal a son `hash_after` présent dans le `ContentStore`. La chaîne
(`hash_before → hash_after`) est continue entre deux cycles consécutifs.

### Invariants (ADR-0010)

1. `hash_after(entry_N) == last_snapshot_N` (cohérence store ↔ log).
2. `store.get_header(last_snapshot_N)` retourne `Some(...)` (I-CSR nominal).
3. `hash_before(entry_N+1) == hash_after(entry_N)` (chaîne continue).

---

## Protocole

```
Agent (AGENT_WAT)

process([0x00]) → commit_barrier + emit
  → snap1 = store.put_snapshot(...)
  → entry1 appended to log (hash_after = snap1)

ORACLE 1 : log.get(action1).hash_after == snap1
ORACLE 2 : store.get_header(snap1) == Some(...)

process([0x00]) → commit_barrier + emit
  → snap2 = store.put_snapshot(...)
  → entry2 appended to log (hash_before = snap1, hash_after = snap2)

ORACLE 3 : entry2.hash_before == snap1    (chaîne continue)
ORACLE 4 : entry2.hash_after == snap2
ORACLE 5 : store.get_header(snap2) == Some(...)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Crash entre store_put et log_append | S15 (régime concurrent + drop_caches) |
| Power-loss | UC-26 (hardware qualifié) |
| I-CSR sous SIGKILL + cache invalidé | ICSR-drop-caches / S15 |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s27_emit_contract --exact --nocapture
```

---

## Références

- **ADR-0010** — Protocole emit : séquence `commit_barrier → store_put → log_append`.
- **ADR-0027** — Durabilité log (no-force suffisant pour chemin nominal).
- **spec/02-properties.md §P6** — Atomicité crash.
- `poc/runtime/src/durability.rs::verify_icsr` — oracle I-CSR utilisé en mode adversarial.
- `poc/runtime/src/lib.rs::tests::s27_emit_contract` — test.
