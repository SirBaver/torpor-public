# ADR-0003 — Modèle causal : adoption de `caused_by[]` (DAG) dès la phase 2

**Date :** 2026-05-12
**Statut :** Acceptée

---

## Contexte

Le lab phase 1 a implémenté un modèle causal en arbre : chaque action a exactement un parent (`caused_by` scalaire). L'auto-résolution du parent ("dernière action globale") était correcte en mono-utilisateur séquentiel. La phase 1.6 a introduit le `session_id` pour corriger la contamination inter-sessions (deux agents parallèles partageaient le même parent), ce qui est un patch sur l'arbre.

Deux propriétés structurelles de ce modèle en arbre resteront fausses à mesure que l'architecture gagne en complexité :

- **Le spawn inter-session** : quand un orchestrateur O crée le sous-agent A, le lien O→A doit être fourni explicitement (passed as `caused_by` à la première action de A). Aucune inférence automatique n'est possible sans context partagé.

- **Le merge** : quand deux branches parallèles (agent A et agent B) contribuent toutes deux à une action de synthèse C, un `caused_by` scalaire ne peut capturer que l'un des deux parents. La causalité réelle est un DAG (C a deux parents : A et B).

La décision de migrer vers `caused_by[]` (tableau) est connue depuis le début mais a été reportée. L'expérience du lab (leçons L1 et L7) montre que chaque report ajoute une migration de schéma sous contrainte. Il est moins coûteux de décider maintenant.

## Décision

Le modèle causal adopte `caused_by[]` (tableau d'action_ids) à partir de la phase 2. Un tableau vide signifie "action racine". Un tableau à un élément est le cas nominal (équivalent à l'arbre actuel). Deux éléments ou plus représentent un merge explicite.

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|---|---|---|---|
| Arbre + session_id (actuel) | Schéma simple, déjà implémenté | Spawn inter-session manuel, merge impossible | Patch insuffisant dès phase 3 |
| DAG `caused_by[]` | Expressif, correct pour tous les cas | Migration de schéma | Retenu — coût de migration augmente avec le temps |
| `caused_by` explicite obligatoire (aucune auto-résolution) | Discipline maximale | Ergonomie dégradée pour clients simples | Rejeté — trop contraignant pour le cas nominal |
| Vecteurs d'horloge (Lamport/vector clocks) | Ordering distribué correct | Complexité d'interrogation, overhead storage | Rejeté — surdesign pour cette phase |

## Conséquences

**Positives :**
- Le log causal peut représenter des merges de branches parallèles (pattern orchestrateur + N sous-agents convergeant)
- Aucune migration de schéma supplémentaire n'est nécessaire pour les phases 3 et 4
- La traversée causale (qui a causé quoi ?) reste un simple index sur action_id

**Négatives / coûts acceptés :**
- Migration de schéma SQLite dans le lab (ALTER TABLE actions ADD COLUMN caused_by_list TEXT — stocker JSON, ou table de jointure)
- L'auto-résolution devient `caused_by[] = [last_action_of_session]` par défaut, ce qui est rétrocompatible avec le cas nominal

**Neutres / à surveiller :**
- Les outils de visualisation du log causal devront gérer des nœuds à plusieurs parents
- La session scoping reste utile comme convention de nommage même avec `caused_by[]`

## Références

- `lab/LESSONS.md` §L1/L7 — observations empiriques ayant motivé cette décision
- `lab/daemon/actions.py::resolve_caused_by` — implémentation actuelle à migrer
- `spec/04-hypotheses.md` — H-profil-B (spawn de sous-agents comme pattern courant)
- ADR liée : ADR-0002 (choix substrat, log causal append-only)

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
