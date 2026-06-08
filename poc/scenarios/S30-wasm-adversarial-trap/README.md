# S30 — WASM adversarial trap + isolation (UC-18 / ADR-0048 / P4)

**Régime :** R1 (P4 — isolation WASM sandbox, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7) sans re-validation W^X matérielle.

---

## Ce qui est testé

**P4 (sandbox WASM)** : un accès mémoire hors-bornes (OOB) dans un agent WASM
est contenu par Wasmtime — le trap ne s'échappe pas du sandbox, ne corrompt pas
le ContentStore ni le CausalLog du runtime. L'agent qui trap émet AgentCrash
(0x13, cause=ProcessFailed 0x01). Un autre agent (AGENT_WAT) continue à émettre
normalement après le crash, et ses entrées respectent l'invariant I-CSR (hash_after
présent dans le ContentStore).

### Vecteurs testés

| Vecteur | WAT | Wasmtime trap |
|---------|-----|--------------|
| OOB mémoire (offset 0x10000 = au-delà page 1) | `OOB_TRAP_AGENT_WAT` | `MemoryOutOfBounds` |
| `unreachable` | `TRAP_AGENT_WAT` | `UnreachableCodeReached` |

S30 utilise `OOB_TRAP_AGENT_WAT` comme vecteur principal (non couvert ailleurs).
Le vecteur `unreachable` est couvert par `t_process_one_trap_emits_agent_crash`.

### Différence avec S24/S25

- S24/S25 : isolation par **epoch watchdog** (`Trap::Interrupt`), boucle infinie.
- S30 : isolation par **sandbox WASM** (`Trap::MemoryOutOfBounds`), trap synchrone.
  La cause dans le payload est la même (`ProcessFailed = 0x01`), le mécanisme est différent.

---

## Protocole

```
Scheduler
  register(A = OOB_TRAP_AGENT_WAT)
  register(B = AGENT_WAT)

send(A, "trigger") → i32.load(0x10000) → MemoryOutOfBounds → AgentCrash(0x01)

ORACLE 1 : entries_by_agent(A) contient AgentCrash(0x13), payload[0]=0x01
ORACLE 2 : entries_by_agent(A) ≠ entries_by_agent(B) (isolation)

send(B, [0x00]) → commit_barrier + emit → entrée B dans log

ORACLE 2 : entries_by_agent(B) ne contient PAS AgentCrash
ORACLE 3 (I-CSR) : toutes les entrées commitées de B ont hash_after dans ContentStore
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| div0 (i32.div_s avec diviseur 0) | Même mécanisme (ProcessFailed 0x01) |
| `unreachable` | `t_process_one_trap_emits_agent_crash` |
| Isolation seL4 (W^X matérielle) | UC-24 (⏳ stack seL4) |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s30_wasm_adversarial_trap_isolation --exact --nocapture
```

---

## Références

- **ADR-0048** — Isolation WASM : sandboxing Wasmtime, mémoire linéaire bornée.
- **ADR-0015 D15.2** — AgentCrash : cause=ProcessFailed (0x01) pour tout trap non-watchdog.
- `poc/runtime/src/actor.rs::OOB_TRAP_AGENT_WAT` — module WASM OOB.
- `poc/runtime/src/lib.rs::tests::s30_wasm_adversarial_trap_isolation` — test.
