# S31 — Flood `0x14` au-delà de 32 ressources distinctes

**Date :** 2026-06-03  
**Test :** `cargo test -p os-poc-runtime --release --lib -- s31_audit_flood_beyond_bound_32 --nocapture`  
**Verdict : LIMITE DOCUMENTÉE**

---

## Setup

| Paramètre | Valeur |
|-----------|--------|
| Borne anti-DoS | 32 resources distinctes par action (ADR-0051 §D2) |
| Séquence | 101× `"bn"` → set={bn}, F1 ; 31 resources `r01..r31` → set=32/32 ; `"secret-33"` → F2 |
| Substrat | Linux (D7 : verdict non transférable seL4) |

## Oracle P4 (double)

| Invariant | Résultat |
|-----------|----------|
| `"secret-33"` refusé côté store (isolation P4 intacte) | **PASS** |
| `"secret-33"` absent du log (`log_resources`) | **LIMITE DOCUMENTÉE** |
| `"secret-33"` présent dans le témoin hors-bande (`witness`) | confirmé |
| Sentinel F2 (entrée agrégée sans resource) émis dans le log | **PASS** |
| `Masked = {"secret-33"}` | confirmé |

## Sortie runtime

```
UC-19 (limite documentée ADR-0051 D2) : set saturé (32/32), 'secret-33' masquée, sentinel F2 émis. Masked={"secret-33"}
```

## Finding

L'accès `"secret-33"` est **refusé** (P4 isolation tenue). En revanche, la 33ème resource distincte n'est pas nommément attribuée dans le log — seul le sentinel F2 (`rate_limited=0x01`) indique qu'un overflow s'est produit. Le trou d'audit est lui-même auditable (F2 non-silencieux).

**Classification : limite documentée (borne anti-DoS intentionnelle — ADR-0051 §D2).**  
L'isolation est intacte ; c'est la complétude du log qui est bornée.
