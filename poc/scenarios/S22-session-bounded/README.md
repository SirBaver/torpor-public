# S22 — Session bornée (UC-3 / ADR-0012)

**Régime :** R1 (P3 — traçabilité causale, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P3** : frontière de session déclenchée à N_max actions. Après la frontière,
la première action de la nouvelle session cite le Lifecycle=Checkpointed, pas
un snapshot pré-frontière. Pas de mémoire cross-session sans citation explicite
via `agent_add_cause`.

### Invariants (ADR-0012)

1. `SessionBoundary (0x0A)` dans le log à la N-ième action.
2. `lifecycle = Checkpointed` après la frontière.
3. `session_id` incrémenté, `action_count` remis à zéro.
4. 1re action nouvelle session : `parent_ids` contient le Checkpointed (pas un snapshot pré-frontière).

---

## Protocole

```
Agent (SESSION_AGENT_WAT, session_max_actions=3)

process[0x00] → action 1 (session 1)
process[0x00] → action 2 (session 1)
process[0x00] → action 3 → FRONTIÈRE
                            SessionBoundary (0x0A) émis
                            Lifecycle=Checkpointed
                            session_id = 2, action_count = 0

process[0x00] → action 1 de la session 2
                parent_ids contient Checkpointed (pas l'action 3 directement)

ORACLE :
  session_id == 2
  SessionBoundary (0x0A) dans le log
  parent_ids(action1_session2) contains Checkpointed_id
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Borne durée (24h) | `session_boundary_duration_configurable` couvre ce cas |
| Citation cross-session (add_cause) | UC-1/S18 couvrent `agent_add_cause` |
| Reconstitution depuis une frontière | S13 (persistence-restart) |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s22_session_bounded --exact --nocapture
```

---

## Références

- **ADR-0012** — Session bornée, `session_max_actions`, `SessionBoundary`.
- `poc/runtime/src/actor.rs::SESSION_AGENT_WAT` — module WASM.
- `poc/runtime/src/lib.rs::tests::s22_session_bounded` — test.
