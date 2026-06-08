# ADR-0004 — Schéma mémoire structuré : namespaces et clés canoniques

**Date :** 2026-05-12
**Statut :** Acceptée

---

## Contexte

Le lab phase 2 a confirmé empiriquement l'hypothèse H-mémoire-schema : deux agents avec exactement le même system prompt ont mémorisé "Dupont" (nom de famille utilisateur) sous deux clés distinctes — `user.family.last_name` et `user.family.name`. La convergence de nommage ne peut pas être garantie par instruction dans le prompt à < 7B paramètres.

Ce résultat rejoint une observation antérieure (lab phase 1.5, leçon L6) : le modèle avait écrit `first_name=Joey` puis lu depuis `name`, ne trouvant pas son propre enregistrement.

Un store clé-valeur non contraint devient structurellement incohérent dès qu'il y a plusieurs agents : chaque agent nomme librement les clés, les valeurs s'écrasent sans détection de conflit, et aucune traversée du store ne permet de reconstruire une vue cohérente de l'état partagé.

La correction n'est pas au niveau du prompt (instruction-following insuffisant) ni du modèle (c'est une propriété de l'espace des représentations sémantiques). Elle doit être au niveau de l'outil : le store doit imposer un schéma.

## Décision

L'API mémoire de production imposera trois niveaux de structuration :

1. **Namespaces par agent** : chaque clé est prefixée par l'identifiant de l'agent ou du domaine (`agent_id/key` ou `domain/key`). Un agent ne peut écrire que dans son namespace, sauf capabilities explicitement déléguées.

2. **Registre de clés canoniques partagées** : un ensemble de clés canoniques cross-agents est défini dans un schéma partagé (ex. `user.name`, `user.email`, `session.goal`). Ces clés ont un type défini et une validation à l'écriture. Les agents qui veulent accéder à l'état partagé utilisent les clés canoniques, pas des clés libres.

3. **Résolution de conflits explicite** : si deux agents écrivent la même clé canonique avec des valeurs différentes, le store signale le conflit plutôt que d'écraser silencieusement. Le protocole de résolution est défini par le domaine (last-write-wins, merge, escalade vers l'orchestrateur).

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|---|---|---|---|
| Prompt engineering (system prompt plus détaillé) | Aucune migration | Ne fonctionne pas à < 7B paramètres (confirmé empiriquement) | Réfutée par le lab |
| Convention de nommage documentée (sans enforcement) | Simple | Aucun enforcement — même problème à terme | Insuffisant |
| Store clé-valeur libre (actuel) | Pas de changement | Incohérence inter-agents incontrôlable | Rejeté — confirmé en phase 2 |
| Namespaces + schéma (retenu) | Cohérence garantie par le runtime | Migration de l'API, schéma à maintenir | Retenu — seule solution fiable |
| Base de données relationnelle avec typage fort | Cohérence maximale | Surdesign pour cette phase | Rejeté — prématuré en phase 3 |

## Conséquences

**Positives :**
- La cohérence de l'état mémoire partagé est garantie par le runtime, pas par convention
- Les traversées inter-agents (un agent lit ce qu'un autre a écrit) deviennent fiables
- Le log causal peut distinguer les écritures dans un namespace personnel vs une clé canonique partagée

**Négatives / coûts acceptés :**
- L'API `POST /memory` doit accepter et valider le namespace
- Un registre de schéma est nécessaire (même minimal : un fichier JSON versionné)
- Les agents existants (tests, smoke tests) devront être mis à jour pour utiliser les namespaces

**Neutres / à surveiller :**
- Le store clé-valeur nu reste utilisable en interne comme couche de persistence — il n'est pas exposé directement aux agents
- La granularité du namespace (par agent vs par domaine) sera calibrée en phase 3 selon les patterns d'accès observés

## Implémentation planifiée

Phase 3 : introduire le namespace comme préfixe dans l'API mémoire. Le schéma canonique initial couvre les domaines `user.*` et `session.*`. Validation de type : chaîne de caractères non vide. Résolution de conflits : last-write-wins par défaut, escalade opt-in.

## Références

- `lab/LESSONS.md` §L6 — observation initiale (phase 1.5)
- `lab/LESSONS.md` §L8 — confirmation empirique (phase 2)
- `lab/tests/smoke_test.sh` §P2.3 — test H-mémoire-schema
- `spec/04-hypotheses.md` §H-mémoire-schema — hypothèse et résultat empirique
- ADR liée : ADR-0003 (modèle causal DAG — namespaces par agent cohérents avec session_id)

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
