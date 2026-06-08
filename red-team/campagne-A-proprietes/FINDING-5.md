# FINDING-5 — Forgerie causale citant vraie action cross-agent

**Vecteur :** A-5  
**Propriété attaquée :** P3  
**Use case de référence :** UC-20 / S32  
**Régime :** R1 (effets)

---

## Hypothèse d'attaque

Un agent A appelle `agent_add_cause(action_id_B)` pour insérer une action de l'agent B comme parent causal, sans que B ait jamais délégué ou envoyé ce lien. `action_id_B` est un SHA-256 d'une action existante — il peut être lu dans le log. Hypothèse : le runtime ne vérifie pas que la relation causale est légitime (autorisée entre agents) → un agent peut forger un DAG causal qui altère la reconstruction de la causalité inter-agents.

## Oracle

`poc/scenarios/S32-causal-forgery-b-light/VERDICT.md` — harnais Rust déterministe.

Invariant testé : un `action_id` connu de B est lisible dans le log. `agent_add_cause(action_id_B)` depuis A est appelé. L'oracle vérifie si le lien est accepté et enregistré.

## Résultat

**LIMITE DOCUMENTÉE**

`agent_add_cause` ne vérifie que l'**existence** de l'`action_id` dans le log (check B-light). Il ne vérifie pas que la relation causale est autorisée entre agents A et B. Le lien est donc accepté et enregistré.

Ce comportement est **documenté et intentionnel** pour le profil B-light (mono-tenant, ADR-0036) : le déploiement mono-tenant suppose que tous les agents opèrent dans le même domaine de confiance. La vérification B-fort (cross-agent capability + multi-tenant) est sur DORMANT.

## Classification

**Limite documentée (B-light mono-tenant)** — aucun patch requis dans le régime actuel.

| Condition | État |
|---|---|
| Profil B-light mono-tenant (actuel) | Limite documentée, intentionnelle |
| Profil B-fort multi-tenant (ADR-0036) | DORMANT — déclencheur : second tenant réel |

## Notes

- ADR-0036 définit B-fort comme "vérification de la relation causale via capabilities cross-agent". Ce n'est pas un oubli — c'est une dette instruite avec déclencheur explicite.
- La forgerie causale dans un déploiement mono-tenant ne compromet pas les autres propriétés : P2 (irréversibilité), P4 (capabilities), P6 (cohérence store) restent intactes.
