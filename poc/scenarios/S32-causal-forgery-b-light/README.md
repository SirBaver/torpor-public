# S32 — Forgerie causale B-light mono-tenant (UC-20 / ADR-0036 / P3)

**Régime :** R1 (P3 — limite intégrité causale, validation négative)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P3 (intégrité causale — limite documentée)** : `agent_add_cause` implémente
le niveau B-light (existence-check dans le log). En mono-tenant, un agent B peut
citer l'`action_id` d'un autre agent A sans avoir reçu de message de A. Le runtime
accepte la citation — le DAG prétend "B a réagi à A", mais B n'a que copié un
`action_id` depuis le log partagé.

Ce n'est PAS un bug : la garantie B-light est documentée dans ADR-0036. Le critère
de sortie B-fort (multi-tenant) exigerait une capability cross-agent sur `add_cause`.
UC-20 **écrit le critère de sortie B-fort**, il ne prétend pas que le PoC le satisfait.

### B-light vs B-fort (ADR-0036)

| Niveau | Check | Garantie | PoC actuel |
|---|---|---|---|
| B-light | existence de l'`action_id` dans le log | DAG acyclique, entrées authentiques | ✅ implémenté |
| B-fort | capability cross-agent + réception du message | Lien causal sémantiquement correct | ⏳ multi-tenant |

---

## Attaque

```
Agent A : process([0x00]) → A1 (action_id_A)

Agent B : process([0x00]) → B0   ← indépendant de A

Fuite : B "lit" action_id_A dans le log partagé (mono-tenant, aucune barrière)
Forgerie : B.process([0x04, ...action_id_A...])
           → add_cause(action_id_A) → log.get(action_id_A) → Some(_) → OK (0)
           → commit_barrier + emit → B1

RÉSULTAT : parent_ids(B1) = [B0, action_id_A]
           Le DAG prétend "B a réagi à A" — FAUX.
```

---

## Oracles

```
Oracle 1 (B-light accepté) : process_one([forge_msg]) réussit, B1 ≠ B0

Oracle 2 (DAG trompé) : parent_ids(B1) contient action_id_A

Oracle 3 (limite) : A n'a émis qu'une seule action (aucun message vers B),
                    B0 ne cite pas A (indépendance confirmée avant forgerie)
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| `action_id` aléatoire (forgé) | SEF-7/SEF-13 : refus -3 (`action_id` inconnu) |
| Flood `add_cause` > 16 | `a3_add_cause_flood_capped` |
| B-fort (capability cross-agent) | UC-20 est le critère de sortie, pas sa réalisation |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s32_causal_forgery_b_light_monotenant --exact --nocapture
```

---

## Références

- **ADR-0036** — `agent_add_cause` B-light vs B-fort ; capability cross-agent en multi-tenant.
- **ADR-0003** — Protocole `agent_add_cause`, format parent_ids.
- `poc/runtime/src/actor.rs` `agent_add_cause` (l.1331-1367) — implémentation B-light.
- `poc/scenarios/S18-add-cause-merge/` — pendant légitime (UC-1, merge réel).
- `poc/runtime/src/lib.rs::tests::s32_causal_forgery_b_light_monotenant` — test.
