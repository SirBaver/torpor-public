# S17 — Rollback + invalidation cap en cascade (UC-9 / ADR-0007 / ADR-0005)

**Régime :** R1 (P2 × P4 — propriétés d'effet, actives indépendamment de la topologie d'inférence)
**Substrat :** Linux. Verdict non transférable à seL4 (D7).

---

## Ce qui est testé

**P2 × P4** : rollback d'une action ayant délégué des caps, avec invalidation récursive O(depth).

Après le rollback d'un agent A vers un snapshot antérieur à la délégation :
1. La cap de A (émise après le snapshot) est révoquée via `revoke_owned_after`.
2. `revoke_owned_after` appelle désormais `revoke(id)` sur chaque victime,
   ce qui **cascade** récursivement aux dérivées — caps déléguées à des sous-agents
   à toutes les profondeurs (O(depth)).
3. Le sous-agent B voit sa cap révoquée au prochain accès.
   **Pas de cache** : `check()` re-vérifie à chaque appel (ADR-0005 invariant).

### Oracle

P2 × P4 est violée si, après le rollback de A :
- **(a)** `cap_store.get(C_A_id)` retourne `Some(...)` (la cap de A n'a pas été révoquée).
- **(b)** `cap_store.get(C_B_id)` retourne `Some(...)` (la cap déléguée à B n'a pas été révoquée en cascade).
- **(c)** `cap_store.check(B, C_B_id, ...)` retourne `true` (B peut accéder après rollback).

---

## Acteur(s)

| Nom | Source | Rôle |
|-----|--------|------|
| Agent A | WAT inline (commit_barrier seul) | Détient C_root, crée snapshot, délègue à B |
| Agent B | WAT inline (commit_barrier seul) | Reçoit C_B déléguée, tente accès après rollback |
| Scheduler | runtime | Exécute le rollback de A |

Pas d'inférence. Test de cap store pur.

---

## Protocole

```
Test (main)
───────────────────────────────────────────────────────────────────
[Setup]
  Supervisor → grant_root(A, rw+delegate, "/data") → C_root
  A.process_one(0x01) → barrier() → snapshot S0 de A (C_root est AVANT S0)
    ↓ ts_S0 = ts du snapshot
[Delegation après snapshot]
  Supervisor → grant_root(A, rw+delegate, "/data") → C_A  (issued_at > ts_S0)
  cap_store.delegate(C_A, A→B, read, "/data/sub") → C_B   (owned by B)
  B sait qu'il a C_B → cap_store.check(B, C_B) == true ✓

[Rollback de A vers S0]
  Scheduler.rollback(A, target_seq=0)
    → revoke_owned_after(A, ts_S0) :
       • C_A (owned by A, issued après S0) → revoke(C_A)
       • revoke(C_A) cascade → revoke(C_B) (owned by B, enfant de C_A)
       • cap_store.get(C_A) == None ✓
       • cap_store.get(C_B) == None ✓

ORACLE :
  cap_store.get(C_A)          == None  (révoquée)
  cap_store.get(C_B)          == None  (cascade)
  cap_store.check(B, C_B, ..) == false (accès refusé)
  caps_invalidated dans 0x0B  >= 1
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Cascade à profondeur k > 2 | UC-16 (révocation récursive profonde) — teste spécifiquement O(depth) sous profondeur variable |
| Cap révoquée puis re-déléguée | Les caps sont immuables une fois révoquées (ADR-0007 §Conséquences) |
| Sub-agent B qui tente l'accès via un WASM running | Le test vérifie via `cap_store.check()` directement ; l'effet 0x14 dans le log est testé par UC-19 |
| Caps émises AVANT le snapshot | Ne doivent PAS être révoquées — vérifié par le test (C_root reste valide) |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s17_rollback_cap_cascade --exact --nocapture
```

---

## Prérequis

- `SIMPLE_AGENT_WAT` (agent WAT inline dans lib.rs, partagé)
- SleepyBackend non nécessaire (pas d'inférence)
- Rust stable

---

## Références

- **ADR-0007** — Invalidation des capabilities lors d'un rollback.
- **ADR-0005** — Design capabilities, `check()` sans cache (invariant re-vérification).
- **S4-scheduler-rollback** — Précurseur : tests présence 0x0E/0x0B + 1 niveau de cap révoquée.
- `poc/capabilities/src/lib.rs::revoke_owned_after` — modifié pour cascade (UC-9).
- `poc/capabilities/src/lib.rs::revoke` — révocation récursive O(depth).
- `poc/runtime/src/actor.rs::Message::Rollback` — appelle `revoke_owned_after`.
