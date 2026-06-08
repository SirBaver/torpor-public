# S19 — Orphelin de compensation non détecté

**Date :** 2026-06-03  
**Test :** `cargo test -p os-poc-runtime --release --lib -- s19_compensation_orphelin --nocapture`  
**Verdict : PASS**

---

## Setup

| Paramètre | Valeur |
|-----------|--------|
| Méthode injection | `CompensationOpen (0x11)` ajouté manuellement au log sans `0x12` |
| Note | Reproduit l'état d'un crash après `CrashPoint::AfterCompensationOpen` sans kill réel |
| Substrat | Linux (D7 : verdict non transférable seL4) |

## Oracle P6

| Invariant | Résultat |
|-----------|----------|
| `open_set` non vide après scan des entrées scheduler (orphelin 0x11 détecté) | **PASS** |
| `open_set` contient `target_agent_id` | **PASS** |
| `actor.last_snapshot()` inchangé (ContentStore intact, pas d'état partiel) | **PASS** |

## Finding

L'orphelin `CompensationOpen` sans `CompensationClose` est détecté par la logique de reconstruction inline (scan des entrées scheduler, maintien d'un `open_set`). Le ContentStore reste dans son dernier état stable — aucun snapshot fantôme n'est apparu. I-CSR préservé.

**GC orphelins (`gc_orphans.rs`) reste sur HOLD** : la suppression automatique n'est pas activée (ADR-0055 D7 — déclencheur D4 non atteint). La détection est instrumentée.

**Propriété P6 tenue (détection) — suppression différée.**
