# S24 — Watchdog WASM budget (UC-5 / ADR-0025)

**Régime :** R1 (isolation d'exécution, propriété d'effet — indépendant de la topologie d'inférence)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

Budget d'exécution par `AgentProfile`. Un agent en boucle infinie avec le profil
`Algo` est interrompu coopérativement par le watchdog (interruption par époque
Wasmtime). `AgentCrash (0x13)` dans le log. Le runtime ne bloque pas.

### Budget Algo (ADR-0025)

`Algo = 10 ticks × 10ms = 100ms`. L'agent est terminé bien avant le plafond
`LlmShort` (5s), démontrant l'isolation de budget entre profils.

---

## Protocole

```
Agent (INFINITE_LOOP_AGENT_WAT, AgentProfile::Algo)

msg = "trigger" → agent entre en boucle infinie
[watchdog époque → interrupt < 100ms]

ORACLE :
  AgentCrash (0x13) dans le log de l'agent
  elapsed < 4s (budget Algo << LlmShort)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Budget LlmLong (30s) | `t_llm_long_profile_allows_30s` couvre la non-interruption |
| Isolation entre deux agents (A crash, B survit) | S25 (one_for_one) |
| Watchdog sur seL4 | non transférable (D7) |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s24_watchdog_budget --exact --nocapture
```

---

## Références

- **ADR-0025** — Watchdog WASM, `AgentProfile`, interruption coopérative par époque.
- `poc/runtime/src/actor.rs::INFINITE_LOOP_AGENT_WAT` — module WASM boucle infinie.
- `poc/runtime/src/watchdog.rs` — implémentation watchdog.
- `poc/runtime/src/lib.rs::tests::s24_watchdog_budget` — test.
