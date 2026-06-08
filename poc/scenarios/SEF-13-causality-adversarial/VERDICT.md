# SEF-13 — Traçabilité causale adversariale (P3, campagne ADR-0053 §D-P3)

**Date :** 2026-05-30
**Binaire :** `sef13-runner` (release)
**Verdict global : PASS (3/3)**

---

## Gate (Q2) — Acyclicité du DAG

`CausalLog::append` (lib.rs:382) et `agent_add_cause` (actor.rs:~1320) ne vérifient pas l'acyclicité.
**Mais cycles non-constructibles par design :**
append-only + SHA-256 content-addressed + existence-check (B-light) = un cycle A→B/B→A
exigerait un fixed-point SHA-256 (infaisable). **V3.4 non-constructible en pratique.**
Propriété P3-DAG-acyclique garantie structurellement, non par check explicite.

---

## V3.3a — action_id forgé (zéro faux-positif) : PASS

**Scénario :** 10 000 `action_id` aléatoires (LCG 32 bytes) → `log.get(forgé)` doit retourner `None`.

- Lookups forgés : 10 000
- Faux-positifs : **0**

**Finding :** zéro faux-positif confirmé. SHA-256 collision-résistant en pratique.
P3a (lookup point isolé) ne retourne pas d'entrée fantôme sous forgerie.

---

## V3.3b — intégrité content-addressed : PASS

**Scénario :** Pour chaque `action_id` réel (1001 entrées) :
`log.get(id)` retourne `Some(entry)` et `entry.action_id() == id`.

- Action_ids vérifiés : 1001
- Introuvables : **0**
- Violations d'intégrité : **0**

**Finding :** intégrité SHA-256 vérifiée sur l'ensemble du log. L'entrée retournée est
authentiquement celle dont le hash est la clé — propriété content-addressed instanciée
sous requête adversariale.

---

## V3.4 — DAG cyclique : PASS (par construction — non applicable)

Cycles non-constructibles via l'API publique (voir Gate Q2 ci-dessus).
La propriété P3-DAG-acyclique est garantie structurellement sans check explicite.
Aucun harness de régression requis.

---

## Verdict P3 (campagne adversariale, dimensions intégrité)

P3a (intégrité content-addressed + zéro faux-positif) : **PASS**.
V3.1/V3.2 (latence sous concurrence) : **conditionnels** — régime cache honnête + workload défini requis (ADR-0053 §D-P3, critère go/no-go V3.1/V3.2). Non exercés dans ce scénario.
V3.4 (DAG cyclique) : **non-constructible par design** — finding positif.

**P3 intégrité : PASS (campagne adversariale, substrat Linux PoC — non transférable seL4, ADR-0050 §D7)**

## Références

- `decisions/0053-cadrage-campagne-p2-p3-p5.md` §D-P3 (vecteurs, oracle G-P3)
- `poc/runtime/src/bin/sef13_runner.rs`
- `poc/causal-log/src/lib.rs:186-190` (`LogEntry::action_id()` — SHA-256 content-addressed)
- `poc/causal-log/src/lib.rs:382-409` (`CausalLog::append` — pas de check acyclicité)
- `poc/runtime/src/actor.rs:~1320-1356` (`agent_add_cause` — existence-check uniquement)
