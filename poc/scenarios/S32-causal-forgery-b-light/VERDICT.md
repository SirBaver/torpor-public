# S32 — Forgerie causale B-light (mono-tenant)

**Date :** 2026-06-03  
**Test :** `cargo test -p os-poc-runtime --release --lib -- s32_causal_forgery_b_light_monotenant --nocapture`  
**Verdict : LIMITE DOCUMENTÉE**

---

## Setup

| Paramètre | Valeur |
|-----------|--------|
| Agent A | produit A1, sans interaction avec B |
| Agent B | lit `a1_id` depuis le log partagé mono-tenant, appelle `add_cause(a1_id)` |
| Profil | B-light (existence-check seul, ADR-0036) |
| Substrat | Linux (D7 : verdict non transférable seL4) |

## Oracle P3

| Invariant | Résultat |
|-----------|----------|
| `add_cause(a1_id)` retourne 0 (forgerie acceptée par B-light) | **LIMITE** |
| `parent_ids(B1)` contient `a1_id` | confirmé |
| `parent_ids(B0)` ne contient pas `a1_id` (baseline indépendante) | confirmé |
| A n'a émis qu'une seule action (aucun message vers B) | confirmé |

## Finding

En mono-tenant, tous les agents partagent le même `CausalLog` en lecture. `agent_add_cause` vérifie uniquement l'**existence** de l'`action_id` dans le log (check B-light), pas la légitimité de la relation causale entre agents. La forgerie est acceptée et inscrite dans le DAG.

**Ce comportement est intentionnel** : le déploiement mono-tenant suppose un domaine de confiance unique (ADR-0036 R3 résiduel).

**Classification : limite documentée (B-light mono-tenant).**  
Le critère de sortie B-fort = capability cross-agent sur `agent_add_cause` (multi-tenant, ADR-0036 — déclencheur : second `TenantId` distinct).
