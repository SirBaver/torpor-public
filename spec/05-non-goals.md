# 05 — Non-objectifs

## 1. Pourquoi documenter les non-objectifs

### 1.1 Éviter la dérive de périmètre (scope creep)

Un non-objectif est une affirmation explicite qu'une propriété ou fonctionnalité ne fait pas partie du périmètre de ce projet, accompagnée d'une motivation. Sans cette documentation, les non-objectifs restent implicites et deviennent des sources de confusion lors des décisions de conception : on risque de sur-concevoir pour des cas qui ne sont pas dans le scope, ou d'invalider des décisions architecturales au nom d'un besoin qui n'a jamais été retenu.

### 1.2 Distinguer "pas maintenant" de "jamais"

Certains non-objectifs sont des exclusions définitives (hors scope pour des raisons fondamentales de conception). D'autres sont des exclusions de phase 1 qui pourraient être revisitées. Cette distinction est indiquée pour chaque entrée.

---

## 2. Non-objectifs explicites

---

### N-rollback-ext — Pas de compensation des effets externes

**Énoncé :** Ce projet ne fournit pas de mécanisme de compensation des effets de bord externes au nœud — c'est-à-dire des messages réseau ayant quitté la machine, des appels API tiers, ou des opérations sur des systèmes externes.

**Motivation :** La compensation transactionnelle d'effets distribués (le modèle "saga") est un problème non résolu en général dans la littérature des systèmes distribués. Inclure ce mécanisme dans l'OS lui-même reviendrait soit à le résoudre (hors du périmètre de ce projet), soit à donner une illusion de résolution dangereuse. La compensation de tels effets est et reste une responsabilité applicative : c'est à l'agent, ou à la couche applicative au-dessus de l'OS, de déclarer des actions compensatoires si son domaine métier l'exige.

Référence : [Garcia-Molina & Salem 1987] "Sagas".

**Portée du rollback garantie :** Le rollback dans ce projet s'applique exclusivement à l'état local (voir définition dans `06-glossary.md`). Les effets ayant franchi un commit barrier ne sont pas affectés.

**Classification :** Exclusion définitive — cette limitation est constitutive du modèle de système, pas une décision de phase 1.

---

### N-concurrency-intra — Pas de concurrence intra-agent

**Énoncé :** Ce projet ne supporte pas la concurrence intra-agent, c'est-à-dire l'existence de threads multiples au sein d'un même agent. Un agent est un acteur unique, séquentiel du point de vue de ses actions. La concurrence est obtenue exclusivement par décomposition explicite en sous-agents.

**Motivation :** Cette contrainte n'est pas une simplification d'implémentation — c'est un choix de modèle qui entraîne trois conséquences souhaitées :

1. **Cohérence ACID simplifiée.** Chaque acteur étant séquentiel, la cohérence de son état est garantie par construction sans mécanisme de verrou intra-agent.
2. **Observabilité sans ambiguïté.** Toute action est atomiquement attribuée à un agent unique. Il n'y a pas de compétition entre threads pour la causalité d'une action.
3. **Décomposition explicite des responsabilités.** La parallélisation nécessite un choix explicite de l'architecte de l'agent : spawner des sous-agents avec des rôles distincts et des capabilities appropriées. Ce choix est documenté dans le log causal, pas masqué par un scheduler interne.

**Classification :** Exclusion définitive — ce choix est structurant pour le modèle d'acteur, le modèle de causalité et les garanties transactionnelles.

---

### N-interface-humaine — Pas d'interface humaine interactive

**Énoncé :** Ce projet ne fournit pas d'interface humaine interactive : pas de terminal, pas de GUI, pas de shell interactif, pas d'interface conçue pour une interaction humaine en temps réel.

**Motivation :** L'OS est conçu pour les agents IA comme utilisateurs principaux. Fournir une interface humaine équivalente diluerait la proposition de valeur sur deux fronts :

1. **Hypothèses de conception incompatibles.** Une interface humaine suppose des latences perceptuelles (< 100ms pour le feedback visuel), une représentation visuelle, une navigation implicite d'état, et une tolérance à l'ambiguïté. Ces hypothèses sont contradictoires avec les primitives conçues pour des agents qui opèrent à 10⁴–10⁸ actions par vie et n'ont pas de "perception" au sens humain.
2. **Dilution du périmètre.** Construire une interface humaine de qualité est un projet en soi. L'inclure risque d'allouer des ressources de conception à un problème qui n'est pas la proposition de valeur centrale.

**Ce qui est fourni à la place :** L'interface de supervision humaine est un client externe read-heavy qui parle le même protocole que les agents, avec des capabilities différentes (privilégiées). Ce client permet au superviseur asymétrique d'observer (log causal), d'intervenir (révocation, suspension, rollback forcé) et d'autoriser (signature d'actions à fort impact). Il ne constitue pas une interface interactive au sens classique.

**Classification :** Exclusion de phase 1. Une interface de supervision humaine plus ergonomique pourrait être construite en phase 2 comme un client externe au-dessus du protocole du système, sans modifier le cœur de l'OS.

---

## 3. Sujets adjacents délibérément ignorés

### 3.1 Compatibilité avec les OS existants

Ce projet ne vise pas à fournir une couche de compatibilité POSIX, une API syscall Linux, ou une interface de portabilité vers les OS existants. Les workloads de référence (W1, W2, W3 définis dans `benchmarks/reference-workload.md`) sont conçus pour être exécutés nativement sur ce système, pas pour être portés depuis Linux.

<!-- TODO: à compléter si la question de l'interopérabilité avec des binaires Linux est soulevée en phase de conception. -->

### 3.2 Interface humaine (GUI, CLI, accessibilité)

Voir N-interface-humaine ci-dessus. Ce sujet est documenté comme non-objectif de fond, pas seulement comme sujet adjacents.

### 3.3 Déterminisme d'exécution

Ce projet ne garantit pas le déterminisme d'exécution — c'est-à-dire que deux instances du système exposées aux mêmes inputs ne produiront pas nécessairement les mêmes timings, le même ordre de scheduling, ou les mêmes valeurs dépendant de l'horloge wall-clock. Ce que le système garantit est le déterminisme de transition d'état : mêmes inputs dans le même ordre → mêmes outputs et même état final (voir SEF-6 dans `benchmarks/equivalence-scenarios.md`). Le déterminisme d'exécution complet suppose des contraintes de scheduling déterministe quasi impossibles à satisfaire sans un hyperviseur dédié ou un modèle d'exécution monothread strict.

---

## 4. Ce qui pourrait devenir un objectif en phase 2

- Une interface de supervision humaine ergonomique (client externe au-dessus du protocole du système) — voir N-interface-humaine.
- Des mécanismes de compensation applicative outillés (bibliothèques de sagas, helpers de rollback métier) — complémentaires à N-rollback-ext, mais hors du cœur de l'OS.
- La concurrence intra-agent dans un modèle restreint (par exemple, agent avec des "fibres" légères et visibilité totale du scheduler) — si les cas d'usage le justifient et si les garanties causales peuvent être préservées.
