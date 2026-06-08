# docs/design/ — RFC & documents d'exploration

Ce dossier contient des **documents de conception non tranchés** : explorations,
RFC, propositions encore réfutables. Il complète `decisions/` sans s'y substituer.

## Distinction de genre — pourquoi deux dossiers

| | `decisions/` (ADR) | `docs/design/` (RFC) |
|---|---|---|
| **Statut** | Tranché | En exploration |
| **Engagement** | **Contraignant** tant que non amendé/remplacé (cf. CLAUDE.md §Conformité aux ADR) | **Aucun** — réfutable, abandonnable |
| **Question** | « Voici ce qu'on fait, et pourquoi » | « Voici un problème ouvert et des pistes » |
| **Cycle de vie** | Acceptée → amendée → remplacée | DRAFT → (promu en ADR \| rejeté \| abandonné) |

Mélanger une RFC ouverte dans `decisions/` corromprait l'invariant
« un ADR est contraignant tant qu'il n'est pas amendé » : une RFC n'engage personne.
Inversement, une décision tranchée n'a rien à faire ici — elle doit devenir un ADR.

## Statuts d'une RFC

| Label | Sens |
|-------|------|
| **DRAFT** | En cours d'exploration. N'engage rien. Peut être incomplète. |
| **PRÊTE** | Exploration close, critères de passage en ADR atteints — candidate à devenir un ou plusieurs ADR. |
| **ABANDONNÉE** | Piste explorée puis écartée. Conservée pour mémoire (pourquoi on n'a *pas* fait ça). |

Une RFC qui aboutit ne reste pas ici : elle **engendre un ou plusieurs ADR** dans
`decisions/`, et son statut passe à **PRÊTE** avec un renvoi vers ces ADR.

## Index

| N° | Titre | Statut | Engendre |
|----|-------|--------|----------|
| [0001](0001-flotte-declarative.md) | Flotte déclarative : composer une flotte d'agents sans recompiler le runtime | **ABANDONNÉE** (2026-06-07) | — (P-forte non confirmée ; famille 4 ; cf. §8) |
| [0002](0002-dispatch-pilote-contenu-llm.md) | Dispatch de flotte piloté par le contenu d'un emit LLM (famille 4) | **DRAFT — GELÉE** (2026-06-08) | — (prototype fait : cœur faisable ; gel décidé à N=2 ; réveil = 3ᵉ cas famille 4) |
