---
layout: default
title: Torpor — la série
---

# Torpor

**Un OS pour des agents : des effets relisables, réversibles et confinés.**

Torpor est un prototype de recherche : un substrat d'exécution pour agents
autonomes qui prend les effets d'un agent et les rend auditables. Journal causal
adressé par le contenu, rollback transactionnel, isolation par capabilities,
atomicité au crash. Chaque borne chiffrée est citée avec son substrat de mesure
et la condition de réfutation écrite *avant* l'expérience.

Cette série de huit articles déroule le système, une propriété à la fois.

## La série

1. [Un OS conçu pour des humains, utilisé par des agents](blog/article-01-os-pour-humains.html) — pourquoi nos OS supposent un humain au clavier, et ce que ça coûte quand l'utilisateur est un agent.
2. [La flèche entre deux décisions d'agent est un hash](blog/article-02-dag-causal-hash.html) — un journal causal adressé par le contenu : toute falsification se voit.
3. [Annuler 500 décisions en 17 millisecondes](blog/article-03-rollback-transactionnel.html) — le rollback transactionnel comme primitive du système, en O(profondeur).
4. [Le coût d'un agent endormi](blog/article-04-densite-dormante.html) — densité hébergée mesurée, densité active bornée par le mur de l'inférence.
5. [Déléguer un accès, et pouvoir le reprendre](blog/article-05-capabilities-red-team.html) — isolation par capabilities, vérifiée à la frontière et éprouvée en red team.
6. [Un crash en plein commit ne laisse rien à moitié](blog/article-06-crash-atomicity-sel4.html) — atomicité au crash, démontrée sur seL4 en émulation QEMU.
7. [Une garantie ne vaut que son noyau](blog/article-07-cinq-paris.html) — les cinq paris architecturaux du projet, et lesquels restent à jouer.
8. [Des effets maîtrisables](blog/article-08-des-effets-maitrisables.html) — la synthèse, sur un pipeline de données conduit par deux agents.

---

*Code sous licence Apache-2.0, documentation sous CC-BY-4.0. Auteur : Joey Leonard.*
