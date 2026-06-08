# S23 — Canal de validation — chemin timeout (UC-4 / ADR-0013 / ADR-0014)

**Régime :** R1 (P4 — isolation, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P4** : chemin timeout du canal A3. L'agent émet `ValidationRequest (0x08)`,
aucune réponse ne vient. Après `validation_timeout_ms`, le runtime injecte
automatiquement `ValidationResponse` avec verdict `Timeout`. L'agent reprend
`Active` sans action de l'extérieur. Pas de retry automatique (ADR-0014 D14.c).

### Complément au chemin nominal

Le chemin nominal (`Approved`/`Rejected`) est couvert par S1. S23 couvre le
**chemin timeout** : pas de superviseur → timeout → décision par défaut de l'agent.

---

## Protocole

```
Agent (VALIDATION_AGENT_WAT, timeout=50ms)

msg[0x00] → build history (baseline)
msg[0x02, 0x01] → request_validation(risk=1) → AwaitingValidation
[aucune ValidationResponse envoyée]
attente 250ms (> 50ms timeout)

ORACLE :
  ValidationRequest (0x08) dans le log
  ValidationResponse verdict=Timeout (2) dans le log (ADR-0014 D14.d)

msg[0x03] → agent traite le message sans bloquer (Active confirmé)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Chemin Approved/Rejected | S1 (supervision algorithmique) |
| Retry après timeout | ADR-0014 D14.c : pas de retry automatique |
| Superviseur LLM | frontière LLM = non-objectif (spec/08 §0.1) |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s23_validation_timeout --exact --nocapture
```

---

## Références

- **ADR-0013** — Canal de validation A3 (primitives agent).
- **ADR-0014** — Politique de supervision : timeout fixe 30s, pas de retry.
- `poc/runtime/src/actor.rs::VALIDATION_AGENT_WAT` — module WASM.
- `poc/runtime/src/lib.rs::tests::s23_validation_timeout` — test.
