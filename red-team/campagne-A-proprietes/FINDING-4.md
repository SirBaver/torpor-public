# FINDING-4 — Orphelin de compensation non détecté

**Vecteur :** A-4  
**Propriété attaquée :** P6  
**Use case de référence :** UC-12 / S19  
**Régime :** R1 (effets)

---

## Hypothèse d'attaque

Un agent ouvre un journal de compensation (`0x11 CompensationOpen`) puis crashe avant `0x12 CompensationClose`. Les blocs de données écrits dans le ContentStore pendant la compensation deviennent des orphelins — non référencés par aucune entrée du log. Hypothèse : la routine de reconstruction ne détecte pas l'orphelin → violation de P6 (cohérence store/log).

## Oracle

`poc/scenarios/S19-compensation-orphelin/VERDICT.md` — harnais Rust déterministe.

Invariant binaire : après reconstruction, `iter_header_data_hashes()` ∩ complement(`iter_block_hashes()`) = ∅. Si un bloc présent dans le store n'a pas de référence dans le log, l'oracle émet FAIL.

## Résultat

**PASS**

S19 injecte manuellement un `CompensationOpen (0x11)` sans `0x12` suivant — reproduit l'état laissé par `CrashPoint::AfterCompensationOpen` sans kill réel. Après scan : `open_set` non-vide (orphelin détecté, `target_agent_id` présent). ContentStore inchangé (dernier snapshot stable, pas d'état partiel). I-CSR préservé.

Oracle exécuté : `tests::s19_compensation_orphelin` (1 run). Voir `poc/scenarios/S19-compensation-orphelin/VERDICT.md`.

## Classification

**Propriété tenue** — aucun correctif requis.

## Notes

- L'invariant mesuré est la détection de l'orphelin 0x11 via `open_set`. La direction inverse (log → store) est l'invariant I-CSR, validé séparément (ICSR-drop-caches, 2026-05-30).
- GC orphelins (`gc_orphans.rs`) reste sur HOLD (ADR-0055 D7 — déclencheur non atteint). La détection est instrumentée ; la suppression automatique n'est pas activée.
