# FINDING-1 — Révocation cap non propagée lors d'un rollback concurrent

**Vecteur :** A-1  
**Propriétés attaquées :** P4 × P2  
**Use case de référence :** UC-9 / S17  
**Régime :** R1 (effets)

---

## Hypothèse d'attaque

Un agent détient une capability déléguée depuis un parent. Le parent initie un rollback concurrent. Hypothèse : la révocation de la capability enfant n'est pas propagée avant que l'enfant ait pu l'utiliser → accès illégitime persistant après rollback.

## Oracle

`poc/scenarios/S17-rollback-cap-cascade/VERDICT.md` — harnais Rust déterministe.

Invariant binaire : après `revoke(cap_id)` déclenchée par rollback parent, `check(child_agent_id, cap_id, scope)` → `false`. Si `check` retourne encore `true`, l'oracle émet FAIL.

## Résultat

**PASS**

La cascade de révocation est O(profondeur de l'arbre de délégation) et s'exécute de façon synchrone avant que l'enfant ne reçoive le prochain message. Aucun état `true` observé après rollback.

Oracle exécuté : `tests::s17_rollback_cap_cascade` (1 run, depth=2 A→B). Voir `poc/scenarios/S17-rollback-cap-cascade/VERDICT.md`.  
Cascade à profondeur k≥4 couverte par `tests::s29_revoke_recursive_deep` (5 nœuds, chain A→B→C→D→E).

## Classification

**Propriété tenue** — aucun correctif requis.

## Notes

- La re-vérification systématique à chaque appel (pas de cache capability) est la défense structurelle qui ferme ce vecteur.
- C_root (émise avant le snapshot) n'est pas affectée — la borne temporelle `ts_S0` est respectée.
- Charge adversariale profonde (depth > 50) : non testée en S17, mais la complexité O(depth) est documentée dans ADR-0005.
