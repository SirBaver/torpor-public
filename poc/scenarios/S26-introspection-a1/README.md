# S26 — A1 introspection (UC-7 / spec 02c)

**Régime :** R1 (P3 côté agent — traçabilité causale, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**A1** : auto-connaissance causale de l'agent via `agent_introspect`. L'agent lit
`last_action_id`, `seq` et `lifecycle_state` avant de prendre une décision. Le
résultat est émis dans le log causal (type `Introspect = 0x06`) pour preuve.

### Invariants

- Cycle 1 : `seq=0`, `last_action_id=zeros` (avant le 1er `commit_barrier`).
- Cycle 2 : `seq=1`, `last_action_id` non-zero (cycle 1 a émis une action).
- Le `seq` dans le payload reflète l'état *avant* le `commit_barrier` courant
  (non-enregistrant : l'introspect ne fait pas avancer le seq seul).

---

## Protocole

```
Agent (INTROSPECT_AGENT_WAT)

process("first") → agent_introspect → payload P0 [seq=0, last_action=zeros]
                   commit_barrier + emit(Introspect, P0)

process("second") → agent_introspect → payload P1 [seq=1, last_action≠zeros]
                    commit_barrier + emit(Introspect, P1)

ORACLE :
  log contient ≥2 entrées Introspect (0x06)
  P0 : seq=0, last_action_id=zeros
  P1 : seq=1, last_action_id non-zero
```

---

## Format du payload Introspect (INTROSPECT_PAYLOAD_LEN = 74 bytes)

```
[  0.. 32] last_action_id  : [u8; 32] — zeros si absent
[ 32.. 40] seq             : u64 LE
[ 40.. 72] last_snapshot   : [u8; 32] — zeros si absent
[      72] flags           : u8 — bit 0 = last_action set, bit 1 = last_snapshot set
[      73] lifecycle_state : u8
```

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s26_introspection_a1 --exact --nocapture
```

---

## Références

- **spec/02c-primitives-agent.md §A1** — agent_introspect.
- `poc/runtime/src/actor.rs::INTROSPECT_AGENT_WAT` — module WASM.
- `poc/runtime/src/actor.rs::INTROSPECT_PAYLOAD_LEN` — format payload.
- `poc/runtime/src/lib.rs::tests::s26_introspection_a1` — test.
