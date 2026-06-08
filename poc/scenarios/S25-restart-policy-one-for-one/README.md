# S25 — Isolation de faute one_for_one (UC-6 / ADR-0013 / ADR-0014)

**Régime :** R1 (P4 — isolation par défaut, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P4** : isolation de faute par défaut (sémantique `one_for_one`). Agent A (boucle
infinie, `AgentProfile::Algo`) est tué par le watchdog. Agent B (normal) continue
sans être affecté. Le crash de A n'a aucun effet sur B : B peut encore traiter
des messages et n'a pas d'`AgentCrash` dans son log.

### Sémantique one_for_one

Chaque agent est une task Tokio indépendante (ADR-0013). Le crash d'une task ne
propage pas d'erreur aux autres. La `rest_for_one` est une politique superviseur
optionnelle (non implémentée dans le Scheduler par défaut).

---

## Acteurs

| Nom | Module | Profil | Rôle |
|-----|--------|--------|------|
| Agent A | INFINITE_LOOP_AGENT_WAT | Algo | Boucle infinie → AgentCrash |
| Agent B | AGENT_WAT | LlmShort (défaut) | Agent normal → continue après crash de A |

---

## Protocole

```
Scheduler
  register(A, Algo)
  register(B, défaut)

send(A, "loop")        → A entre en boucle infinie
[watchdog kill A < 4s]

ORACLE isolation (1/2) :
  entries_by_agent(A) contient AgentCrash (0x13)

send(B, [0x00])        → B traite normalement
attente 200ms

ORACLE isolation (2/2) :
  entries_by_agent(B) ne contient PAS AgentCrash
  entries_by_agent(B) non vide (B a bien émis)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Politique rest_for_one | Non implémentée (superviseur décide — UC-23) |
| Redémarrage automatique de A | Hors scope PoC (GC + restart = ADR futur) |
| Crash de A pendant que B attend une cap de A | UC-9 / S17 |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s25_restart_policy_one_for_one --exact --nocapture
```

---

## Références

- **ADR-0013** — Architecture acteur : isolation des tasks Tokio.
- **ADR-0014** — Politique de supervision et cycle de vie.
- **ADR-0025** — Watchdog et profils (Algo = 100ms).
- `poc/runtime/src/lib.rs::tests::s25_restart_policy_one_for_one` — test.
