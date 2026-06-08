# FINDING-3 — Rollback tentative post-`emit` (commit barrier)

**Vecteur :** A-3  
**Propriété attaquée :** P2  
**Use case de référence :** UC-11 / S28  
**Régime :** R1 (effets)

---

## Hypothèse d'attaque

Un agent appelle `agent_self_rollback(depth)` après avoir émis via `emit()`. Hypothèse : le barrier commit n'est pas correctement positionné → l'agent peut effacer une action déjà commitée dans le log, violant P2 (irréversibilité post-commit).

## Oracle

`poc/scenarios/S28-self-rollback-post-emit/VERDICT.md` — harnais Rust déterministe.

Invariant binaire : `agent_self_rollback` appelé après `emit` retourne un code d'erreur non-zéro ET l'entrée émise reste présente et inchangée dans le log. Si le log est modifié, l'oracle émet FAIL.

## Résultat

**PASS**

`agent_self_rollback(depth=1)` retourne `-3` (historique insuffisant : `seq=1 < 1+depth=2`) de façon silencieuse. Snapshot inchangé. Aucun `SelfRollback (0x07)` dans le log. Exactement 1 entrée dans le log (l'emit initial). L'append-only RocksDB rend la mutation structurellement impossible.

Oracle exécuté : `tests::s28_self_rollback_post_emit_refused` (1 run). Voir `poc/scenarios/S28-self-rollback-post-emit/VERDICT.md`.

## Classification

**Propriété tenue** — aucun correctif requis.

## Notes

- Le refus retourne un code `-3` (i32) à l'agent WASM — il n'est pas ignoré. L'agent peut inspecter le code de retour et adapter son comportement.
- Le barrier est positionné dans `emit()` côté runtime, pas dans la logique agent — l'agent WASM ne peut pas l'outrepasser.
