# S31 — Audit flood au-delà de la borne 32 (UC-19 / ADR-0051 §D2 / P4)

**Régime :** R1 (P4 — limite d'audit bornée, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P4 (complétude d'audit — limite documentée)** : l'attribution des refus `0x14` est
garantie pour un ensemble borné de resources distinctes par fenêtre (≤ 32, ADR-0051 §D2).
Au-delà de cette borne, un sentinel d'overflow (F2, `rate_limited=0x01`) signale le trou
sans réintroduire le DoS de log — mais la resource n'est plus attribuée nominativement.

### Distinction avec SEF-9

| Scénario | Attaque | Angle mort | Résolution |
|---|---|---|---|
| SEF-9 | Flood scalaire > 100 sur 1 resource | Masquage d'une resource NOUVELLE post-flood | Correctif #6 (agregation par resource, borne 32) |
| **S31** | Saturation du set (32/32) + flood scalaire > 100 | Masquage d'une 33ème resource | **Limite documentée** ADR-0051 D2 — borne intentionnelle anti-DoS |

SEF-9 est un **correctif** : le masquage était un bug. S31 est une **limite** : le masquage
au-delà de 32 resources distinctes est par design (bornage anti-DoS du log).

### Invariants

1. P4-isolation tient : le refus est effectif (retourne -1, aucun accès accordé).
2. `witness` hors-bande capture "secret-33" (vérité-terrain).
3. `log_resources` ne contient PAS "secret-33" après saturation du set.
4. Un sentinel F2 (`rate_limited=0x01`, resource absente) est émis.

---

## Attaque

```
Étape 1 : flood "bn" × 101 → F1 (sentinel scalaire), set = {"bn"}, count=101
           aggregate_emitted=true, rate-limit dépassé

Étape 2 : "r01".."r31" (31 resources distinctes)
           → set se remplit à 32/32 (has_room=false)
           → chaque resource est new-with-room → attribuée malgré count > 100 (correctif #6)

Étape 3 : "secret-33" → set plein + count > 100 + is_new_resource + !set_overflow_emitted
           → F2 : sentinel overflow de set (is_aggregated=true, resource omise)
           → "secret-33" NON attribuable dans le log
```

---

## Protocole et oracles

```
Oracle 1 (P4-isolation) : get "secret-33" → retour 0xFF (refusé)
Oracle 2 (limite audit)  : "secret-33" ∈ witness \ log_resources (masquée par F2)
Oracle 3 (sentinel F2)   : ∃ entrée CapabilityDenied (0x14) avec rate_limited=0x01
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Flood scalaire seul < 32 resources | SEF-9 (régression #6) |
| Overflow F2 avec plusieurs resources masquées | Un seul sentinel par fenêtre (set_overflow_emitted) |
| Reset de fenêtre (> 1s) | Hors timing de ce test unitaire |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s31_audit_flood_beyond_bound_32 --exact --nocapture
```

---

## Références

- **ADR-0051 §D2** — borne 32 resources distinctes par fenêtre, sentinel F2.
- **ADR-0050 §D3** — oracle hors-bande `cap_denied_witness`.
- `poc/runtime/src/actor.rs` `emit_cap_denied` — logique F1/F2 (l.889-994).
- `poc/scenarios/SEF-9-confused-deputy-audit/VERDICT.md` — correctif #6 (l'ancêtre).
- `poc/runtime/src/lib.rs::tests::s31_audit_flood_beyond_32` — test.
