# 01 — Vision

## Sommaire prévisionnel

1. Le problème : les OS actuels sont conçus pour des humains
   - 1.1 Hypothèses implicites des OS contemporains
   - 1.2 Ce que ces hypothèses coûtent quand l'utilisateur est un agent IA
2. Le constat : ce qu'un agent IA fait différemment d'un humain
   - 2.1 Profil d'accès au système (fréquence, parallélisme, déterminisme attendu)
   - 2.2 Rapport à l'état : lecture déclarative vs navigation implicite
   - 2.3 Rapport à la confiance : capabilities vs identité
   - 2.4 Portée épistémique : LLM agents vs agents autonomes en général
3. Ce qu'on vise
   - 3.1 Énoncé du problème en une phrase falsifiable
   - 3.2 Ce que "réussir" veut dire — critères observables
   - 3.3 Pourquoi maintenant — contexte technique et académique
4. Ce qu'on ne vise pas (renvoi vers 05-non-goals.md)

---

## 1. Le problème : les OS actuels sont conçus pour des humains

### 1.1 Hypothèses implicites des OS contemporains

Les OS contemporains (Linux, Windows, macOS) ont été conçus entre les années 1960 et 1990, pour des utilisateurs humains opérant en temps interactif. Leurs choix de conception — largement reconduits depuis — reposent sur sept hypothèses qui n'ont jamais été rendues explicites parce qu'elles semblaient universelles. Elles ne l'étaient pas.

**H-os-1 — L'utilisateur perçoit le temps en millisecondes.**
Le scheduler Linux CFS [Molnar 2007] priorise la latence interactive : les time slices par défaut sont de l'ordre de 4 à 100 ms, calibrées pour qu'un humain perçoive le système comme réactif. Le paramètre `HZ=250` (250 interruptions d'horloge par seconde) reflète ce calibrage. L'ordonnancement est optimisé pour minimiser la latence perçue par un utilisateur devant un terminal, pas pour maximiser le throughput d'un grand nombre d'agents concurrents.

**H-os-2 — L'interface est du texte non structuré, lisible par un humain.**
Unix a généralisé le principe "tout est un fichier" [Ritchie & Thompson 1974] : l'état du système est exposé via `/proc`, `/sys`, et des commandes dont la sortie est du texte brut. `ls`, `ps`, `ip addr`, `netstat` produisent des formats lisibles par un humain, mais non structurés. La composition se fait par pipe, avec du texte comme format d'échange universel. Cette décision de conception suppose un humain capable de lire, filtrer et interpréter ce texte.

**H-os-3 — L'état du système s'accumule implicitement.**
Il n'existe pas de primitive système pour obtenir une représentation déclarative et complète de l'état courant. L'état est distribué entre le système de fichiers (fichiers modifiés), la mémoire des processus (variables, buffers), les connexions réseau ouvertes, les descripteurs de fichiers, les entrées de registre (Windows), etc. Un humain navigue cet état par convention et habitude, en reconstruisant mentalement ce qui s'est passé. Il n'existe pas de snapshot cohérent et instantané.

**H-os-4 — La confiance est liée à l'identité.**
Le modèle de sécurité Unix repose sur l'identité (UID, GID, groupes). Les droits d'accès sont des attributs permanents liés à l'identité d'un utilisateur ou d'un processus. Un processus root a accès à tout. Ce modèle suppose que l'identité est stable, connue à l'avance, et que la confiance accordée à une identité est constante dans le temps [Dennis & Van Horn 1966 identifient déjà cette limite]. Il n'existe pas de primitive native pour déléguer un droit restreint et révocable à un sous-processus sans lui donner tous les droits du processus parent.

**H-os-5 — Le niveau de parallélisme est borné par la cognition humaine.**
Les abstractions de multitâche (processus, threads, terminaux virtuels) sont conçues pour qu'un humain puisse les superviser. `htop` affiche typiquement quelques dizaines à quelques centaines de processus. Chaque processus porte un overhead fixe : espace d'adressage virtuel (plusieurs MB de pages mappées), entrée dans la table des processus du noyau, pile noyau dédiée (8 à 64 KB par thread sur Linux). Ce coût fixe par entité est acceptable quand le nombre d'entités est borné par ce qu'un humain peut gérer.

**H-os-6 — La persistance est mutable et sans historique garanti.**
Les systèmes de fichiers classiques (ext4, NTFS, APFS) sont des structures mutables : écraser un fichier le détruit. L'historique n'est pas conservé par défaut. Des mécanismes de snapshot existent (ZFS, btrfs, LVM snapshots, VSS), mais avec une granularité volumétrique — ils snapshotent un volume entier, pas l'état d'un processus particulier. CRIU [Checkpoint/Restore In Userspace] permet de sérialiser un processus, mais avec une complexité O(N) sur la taille de l'état et des durées typiques de plusieurs secondes.

**H-os-7 — L'observabilité est reconstructive et interactive.**
Les outils d'observabilité sur Linux (`strace`, `audit`, `/proc`, `perf`, `eBPF`) sont conçus pour être interrogés interactivement par un humain qui diagnostique un problème. `strace` produit du texte brut à partir de syscalls. Le subsystème `audit` logue les syscalls dans des fichiers rotatifs. Ces outils n'ont pas été conçus pour produire nativement un log causal structuré, requêtable programmatiquement, avec une garantie de latence de lookup.

---

### 1.2 Ce que ces hypothèses coûtent quand l'utilisateur est un agent IA

Chaque hypothèse identifiée en 1.1 devient une contrainte concrète pour un agent autonome long-running (profil B, défini en section 2.1). Le tableau ci-dessous en résume les coûts ; les détails suivent.

| Hypothèse | Contrainte pour l'agent IA | Nature du coût |
|-----------|---------------------------|----------------|
| H-os-1 Latence perceptuelle | Scheduling inadapté aux milliers d'agents concurrents | Performance — overhead constant |
| H-os-2 Texte non structuré | Parsing fragile de l'état système | Fiabilité + performance |
| H-os-3 État implicite | Impossible de snapshotter proprement | Fonctionnel — rollback impossible nativement |
| H-os-4 Confiance par identité | Modèle de confiance inadapté aux capabilities dérivables | Sécurité — sur-permission systématique |
| H-os-5 Parallélisme borné | Densité d'agents limitée par overhead fixe par processus | Performance — coût prohibitif à grande échelle |
| H-os-6 Persistance mutable | Rollback applicatif doit être réimplémenté à chaque agent | Fonctionnel + duplication de travail |
| H-os-7 Observabilité reconstructive | Traçabilité causale absente ou coûteuse | Fonctionnel — audit impossible à l'échelle |

**Coût de H-os-1 — Scheduling inadapté.**
Un agent IA long-running (profil B) traite entre 10⁴ et 10⁸ actions au cours de sa vie, avec un profil de charge soutenu et peu interactif. Le CFS, optimisé pour la latence interactive, introduit des préemptions inutiles pour ce profil. Plus grave : un système qui fait tourner N agents simultanément, chacun implémenté comme un processus Linux, génère N×overhead_scheduling par unité de temps. À N=1 000 agents, même un overhead de 1 ms par scheduling event par agent représente 1 000 ms de CPU de scheduling par seconde — soit 1 cœur entier dédié au scheduling pur.

**Coût de H-os-2 — Interfaces non structurées.**
Un agent qui doit lire l'état du système via `ls`, `ps`, ou `cat /proc/meminfo` est exposé à : des changements de format entre versions du noyau, des locales qui modifient le formatage des nombres, des encodages inattendus dans les noms de fichiers, et des races conditions entre la lecture et la modification de l'état. Ce coût est permanent, non-nécessaire si l'interface était structurée, et source d'une classe entière de bugs que les agents réels rencontrent en production (parsing d'un `ls` qui change selon la locale, parsing de `ps` dont le format varie entre distributions).

**Coût de H-os-3 — État implicite.**
Un agent long-running qui veut se checkpointer pour pouvoir être rollbacké doit résoudre un problème non-trivial : qu'est-ce que "son état" ? La mémoire du processus (capturée par CRIU), les fichiers qu'il a modifiés (non tracés par le FS), les connexions réseau ouvertes (état du noyau, pas de l'application), les messages en transit (dans les buffers des sockets). Il n'existe pas de primitive unifiée pour capturer l'état d'un agent de manière cohérente. CRIU s'en approche mais exclut les effets réseau et a un coût O(taille de l'état) en temps de snapshot.

**Coût de H-os-4 — Confiance par identité.**
Un agent IA n'a pas une identité stable au sens Unix. Son comportement varie selon son état interne, son contexte de tâche, et les instructions reçues. Lui accorder des droits root parce qu'il est "de confiance" revient à accorder des droits illimités à une entité dont le comportement n'est pas déterministe. La pratique actuelle — faire tourner les agents avec des droits restreints et contenir via AppArmor/seccomp — est une mitigation externe, pas une solution architecturale. Elle n'offre pas la délégation fine et révocable de capabilities nécessaire pour les agents qui spawnen des sous-agents (profil B, critère e).

**Coût de H-os-5 — Densité limitée.**
Un container Docker minimal consomme entre 100 MB et plusieurs GB de RAM selon son contenu. Le overhead d'orchestration (kubelet, CNI plugin, kube-proxy) ajoute plusieurs dizaines de MB par nœud, plus un overhead par pod. Sur une machine de 16 GB (hardware de référence de ce projet), une baseline raisonnablement tunée peut faire tourner entre 50 et 200 agents concurrents selon leur profil mémoire. L'hypothèse de densité 5x (propriété P1) s'appuie sur le fait que cet overhead est en grande partie dû aux abstractions conçues pour un parallélisme borné par la cognition humaine, pas par les contraintes physiques du hardware.

**Coût de H-os-6 — Persistance mutable.**
L'absence de rollback transactionnel natif force chaque agent à réimplémenter sa propre logique de versioning : journalisation applicative, bases de données SQLite locales, git pour les fichiers texte, etc. Cette duplication est coûteuse en développement, incohérente entre agents, et ne bénéficie d'aucune garantie système (un crash entre deux écritures applicatives peut laisser un état incohérent). Les agents qui nécessitent du rollback fiable (tous les agents long-running sérieux) portent ce coût individuellement.

**Coût de H-os-7 — Observabilité reconstructive.**
Produire la chaîne causale complète d'une action sur Linux requiert de corréler : les logs d'audit (syscalls), les logs applicatifs (si l'agent en produit), les traces réseau (si l'action implique du réseau), et potentiellement des dumps d'état mémoire. Cette reconstruction est une opération O(volume de logs), non-garantie (les logs peuvent être rotatifs ou tronqués), et typiquement dans la fourchette de plusieurs secondes à plusieurs minutes pour des logs de production. Pour un agent qui a exécuté 10⁷ actions, l'audit d'une action particulière n'est pas une opération interactive faisable sans infrastructure dédiée.

---

## 2. Le constat : ce qu'un agent IA fait différemment d'un humain

### 2.1 Profil d'accès au système

L'utilisateur cible de ce projet est un **agent autonome long-running**, défini précisément comme suit :

- **(a) Durée de vie** : entre 1 heure et 1 mois. Ni un processus éphémère (quelques secondes), ni une application permanente sans redémarrage prévu. L'agent doit survivre à des redémarrages du runtime, maintenir son identité et son état à travers ces interruptions.
- **(b) État persistant** : l'agent maintient un état persistant entre actions — un index de connaissances, un journal d'actions, un contexte de tâche. Cet état n'est pas reconstruit à chaque session.
- **(c) Volume d'actions** : entre 10⁴ et 10⁸ actions au cours de sa vie, au sens niveau système — message inter-acteur. Pour un agent à 10⁵ actions par heure actif 8h/jour pendant 1 mois, cela représente environ 2,4×10⁷ actions. La granularité est celle des échanges inter-acteurs, pas des instructions machine.
- **(d) Supervision ponctuelle** : l'agent ne dispose pas d'un superviseur humain pour chaque action. La supervision humaine intervient à des points de contrôle : événements déclenchés par le système (anomalie détectée, action à fort impact en attente d'autorisation), ou revues périodiques initiées par le superviseur. Entre ces points de contrôle, l'agent est autonome.
- **(e) Délégation** : l'agent peut spawner des sous-agents avec des capabilities sous-ensemble des siennes. Ces sous-agents peuvent eux-mêmes spawner, jusqu'à une profondeur définie par les capabilities héritées.

Ce profil (dit "profil B") se distingue des autres usages potentiels d'un OS par des agents IA : il n'est ni un agent batch (durée courte, pas d'état persistant), ni un service permanent sans supervision (durée infinie, pas de point de contrôle), ni un agent interactif (supervision humaine pour chaque action).

### 2.2 Rapport à l'état : lecture déclarative vs navigation implicite

Un humain navigue l'état du système par convention et mémoire procédurale : il sait que les logs sont dans `/var/log`, que la configuration réseau est dans `/etc/network/interfaces`, que les processus sont visibles dans `htop`. Cette navigation est implicite, cumulative, et repose sur une connaissance du contexte acquise par l'expérience.

Un agent IA n'a pas cette mémoire procédurale. À chaque exécution, il dispose uniquement de son état persistant explicite (ce qu'il a décidé de sauvegarder) et de ce que le système lui expose via son interface. Il ne "sait" pas où les choses sont — il doit les trouver, les interroger, les parser.

Cette différence a deux conséquences architecturales.

**Lecture : l'état doit être exposé comme donnée structurée, pas comme texte à naviguer.** Un agent qui interroge l'état du système ne doit pas avoir à parser la sortie de `ps aux` ou à inférer la topologie réseau à partir de `ip addr`. L'état doit être exposable sous une forme structurée, directement consommable sans parsing : un graphe de processus, une liste typée de ressources, un index requêtable de capabilities actives. Ce n'est pas une question d'ergonomie — c'est une question de fiabilité et de coût : le parsing de texte non structuré est fragile, coûteux, et source d'une classe de bugs évitable.

**Écriture : l'état voulu doit être exprimable de manière déclarative.** Un humain qui veut modifier l'état du système exécute une séquence de commandes impératives (`mkdir`, `chmod`, `systemctl start`). L'état résultant est la conséquence d'une série de transitions que l'humain compose mentalement. Un agent bénéficie d'un modèle différent : déclarer l'état voulu ("je veux que ce répertoire existe avec ces permissions et ce contenu"), laisser le système calculer la séquence de transitions nécessaire et la vérifier. C'est le principe de NixOS [Dolstra 2006] appliqué non pas au déploiement de packages, mais à l'état runtime du système.

La distinction imperatif/déclaratif n'est pas un luxe d'ergonomie. Elle est structurante : dans un modèle déclaratif, l'état courant et l'état voulu sont des objets de première classe que le système peut comparer, versionner et auditer. Dans un modèle impératif, l'état courant est une conséquence opaque d'une histoire de commandes que personne ne conserve.

---

### 2.3 Rapport à la confiance : capabilities vs identité

Le modèle de confiance Unix est binaire et statique : un processus a une identité (UID/GID) à laquelle sont attachés des droits. Ces droits sont déterminés à l'avance, s'appliquent uniformément à toutes les actions du processus, et ne peuvent pas être délégués de manière sélective à un sous-processus sans lui transférer l'identité elle-même (setuid) ou utiliser une couche externe (sudo, AppArmor, seccomp).

Ce modèle repose sur deux hypothèses qui ne tiennent pas pour un agent IA :

**Hypothèse 1 : l'identité est stable et prévisible.** Un processus Unix qui tourne en tant qu'`alice` se comporte de manière prévisible selon ce qu'`alice` est censée faire. Un agent IA a un comportement qui varie selon son état interne, son contexte de tâche, les instructions reçues, et — dans le cas d'un LLM — la stochasticité de l'inférence. L'identité `agent_xyz` ne dit rien sur ce que l'agent fera au prochain pas de temps.

**Hypothèse 2 : la confiance est un attribut de l'entité, pas de l'action.** Unix suppose que si on fait confiance à `alice`, on lui donne accès à ses fichiers pour toutes ses actions. Il n'existe pas de primitive pour dire "je fais confiance à `alice` pour lire ce fichier dans ce contexte, mais pas pour l'effacer, et cette confiance expire dans 5 minutes". Les workarounds (ACL, AppArmor profiles, seccomp filtres) sont des approximations externes, pas des primitives du modèle de sécurité.

Le modèle capability résout les deux problèmes. Une capability est un token que l'agent détient et présente pour accéder à une ressource [Dennis & Van Horn 1966]. Elle est :
- **Spécifique** : elle donne accès à une ressource précise avec des droits précis, pas à une classe de ressources.
- **Non-ambient** : l'agent ne peut accéder qu'à ce pour quoi il détient une capability. Il n'existe pas de droits implicites.
- **Révocable** : le superviseur asymétrique peut retirer une capability à tout moment, ce qui limite immédiatement ce que l'agent peut faire — sans dépendre de la coopération de l'agent.
- **Attenuable** : quand un agent spawne un sous-agent, il ne peut lui déléguer qu'un sous-ensemble de ses propres capabilities. Un sous-agent ne peut pas avoir plus de droits que son parent [propriété d'attenuation, Saltzer & Schroeder 1975].

Cette dernière propriété est particulièrement importante pour le profil B. Un agent qui spawne 10 sous-agents pour paralléliser une tâche doit pouvoir leur déléguer les capabilities nécessaires à leur sous-tâche, et seulement celles-là. Un sous-agent compromis (par un prompt injection, un bug, ou un comportement divergent) ne peut alors affecter que les ressources pour lesquelles il détient des capabilities — pas l'ensemble des ressources de l'agent parent.

Le modèle capability n'est pas une nouveauté théorique : seL4 [Klein 2009], CHERI [Watson 2015], Genode [Feske 2014], et le langage E [Miller 2006] en font une démonstration à des niveaux de maturité variés. Ce projet applique ce modèle au niveau du runtime OS d'un agent IA, là où ces systèmes l'appliquent respectivement au niveau du microkernel, du hardware, du framework OS, et du langage de programmation.

---

### 2.4 Portée épistémique : LLM agents vs agents autonomes en général

#### 2.4.1 Le problème de l'outil de mesure

Les LLMs sont des modèles entraînés sur de la production humaine. Leur profil d'exécution — latence d'inférence en centaines de millisecondes, mémoire organisée en clés sémantiques, raisonnement exprimé en langage naturel, granularité d'action calée sur la vitesse de lecture — est un artefact de cet entraînement. En ce sens, les LLMs sont les agents IA les plus proches des humains : ils raisonnent *comme* des humains, lentement, en langage.

Ce projet critique les OS conçus pour les humains. Si ses hypothèses et son dimensionnement sont calibrés sur des agents LLM, il risque de critiquer une cage avec un outil issu de cette même cage — et de concevoir un OS pour *des humains qui ne dorment pas*, plutôt que pour *des agents dont le profil cognitif est fondamentalement différent*.

D'autres paradigmes d'IA ont des profils radicalement différents :

| Paradigme | Fréquence d'action | Nature de l'état | Rapport au langage |
|-----------|-------------------|------------------|-------------------|
| LLM agent (actuel) | 10²–10⁵ actions/h | Sémantique, clé-valeur | Natif |
| Agent RL (jeux, contrôle) | 10⁶–10⁹ actions/s | Vecteur numérique compact | Absent |
| Pipeline ML (vision, NLP) | Batch, asynchrone | Tenseurs intermédiaires | Aucun |
| Agent planificateur (PDDL) | 10²–10⁵ actions/h | Symbolique, discret | Partiel |
| Système de contrôle (robotique) | 10³–10⁴ commandes/s | Signal continu | Absent |

Un agent RL jouant à chess émet 10⁶ actions par seconde. Un système de contrôle robotique cadencé à 1 kHz ne peut pas se permettre un log causal avec p99 ≤ 10ms par action : ce serait 10⁷ µs d'overhead par seconde, soit 10 cœurs CPU de pur logging. Le dimensionnement de P2 et P3 est incorrect pour ces profils.

#### 2.4.2 Ce qui est invariant vs. ce qui est dimensionné pour les LLMs

La distinction utile pour lire ce projet est la suivante :

**Invariant pour tout agent autonome sous supervision humaine :**
- Un historique causal des actions est nécessaire pour l'audit et la supervision.
- Un mécanisme de rollback est nécessaire pour l'intervention.
- Un modèle de délégation révocable des droits est nécessaire pour l'isolation des sous-agents.
- Les abstractions de l'OS ne doivent pas supposer que l'agent navigue un état implicite par convention.

Ces propriétés (P2, P3, le modèle de capabilities) ne dépendent pas de la cognition linguistique. Elles sont des propriétés de la *relation entre agents et ressources sous supervision*, indépendamment de la nature des agents.

**Dimensionné pour les LLM agents (hypothèses à réviser pour d'autres paradigmes) :**
- H-profil-B : durée 1h–1 mois, 10⁴–10⁸ actions/lifetime. Ce corridor est cohérent avec les agents LLM actuels. Un agent RL ou un système de contrôle invaliderait cette hypothèse par le haut (fréquence) ou par le bas (durée de vie d'un épisode d'entraînement).
- P3 seuil ≤ 10ms : adapté à des débits de 10³–10⁵ actions/heure. Inadapté à 10⁶ actions/seconde sans échantillonnage.
- Le modèle de mémoire clé-valeur sémantique : artefact du paradigme LLM. Un agent RL n'a pas de "mémoire" au sens d'un store clé-valeur.

#### 2.4.3 La valeur épistémique des LLMs comme sujets de test

Les LLMs sont de mauvais agents pour mesurer la performance d'un OS pour agents autonomes en général. Ils sont en revanche d'excellents agents pour *révéler les trous de conception* — parce qu'ils opèrent à la vitesse humaine, verbalisent leur raisonnement, et rendent les problèmes architecturaux lisibles.

Le lab de ce projet l'a confirmé empiriquement : les trois décisions architecturales les plus importantes (DAG causal, snapshot auto-contenu, capabilities au niveau système) ont émergé d'observations sur des LLMs qui échouaient de manière *visible et compréhensible*. Un agent RL aurait produit les mêmes échecs structurels, mais sans la verbosité qui les rend diagnosticables.

Cette propriété des LLMs — "faire échouer les bonnes choses de manière lisible" — est la raison pour laquelle ils restent pertinents comme sujets de validation fonctionnelle, indépendamment de leur inadéquation comme sujets de validation de performance.

#### 2.4.4 Discipline de lecture pour ce document

Toute hypothèse ou propriété dans ce projet doit être lue à travers cette grille :

- **Invariante** (valide pour tout agent autonome sous supervision) : mentionné explicitement comme tel, ou dérivable des principes §2.1–§2.3.
- **Dimensionnée LLM** : calibrée sur les profils d'agents LLM actuels. Valide pour les déploiements 2024–2026. À réviser quand d'autres paradigmes dominent ou cohabitent.
- **Dépendante du paradigme** : sans sens hors du paradigme LLM (ex. : schéma mémoire clé-valeur sémantique).

En l'absence de marquage explicite, supposer que le dimensionnement est calibré LLM. C'est honnête : les seuls agents en production aujourd'hui qui correspondent au profil B sont des agents LLM.

---

## 3. Ce qu'on vise

### 3.1 Énoncé du problème en une phrase falsifiable (thèse centrale)

**Thèse :** Un OS conçu pour des agents autonomes long-running peut garantir, par construction, trois propriétés que les OS contemporains (Linux, Windows, macOS) n'offrent qu'en partie ou au prix de couches applicatives coûteuses :

1. **Rollback transactionnel d'état système en O(log N)** sur N actions depuis le dernier commit barrier, avec une durée bornée ≤ 100ms pour N=100.
   *État de validation : PASS — substrat Linux. SEF-2 PASS (5/5 runs, 17–20 ms pour depth=500, largement sous cible ≤ 100 ms). Implémentation : O(depth), pas O(log N) — la revendication O(log N) a été retirée dans `spec/02` (ADR-0051 §D1) ; la borne temporelle reste tenue.*

2. **Traçabilité causale complète et requêtable en O(1)** par action (lookup par identifiant d'action), avec p99 ≤ 10ms.
   *État de validation : PASS — substrat Linux. SEF-5 PASS (3/3 passes, p99 1,4–1,9 ms sur 10⁸ actions en lecture seule, NVMe PCIe, ADR-0026). Fonctionnel seL4 (C.8+, via redb/virtio-blk). Non abouti : D-P3a (mesure de latence sur board réelle ou NVMe passthrough seL4) — bloqué infrastructure, QEMU non recevable comme substrat de mesure (ADR-0046).*

3. **Densité d'agents par unité de hardware au moins 5x supérieure à Linux+containers** sur le workload W1 défini dans `benchmarks/reference-workload.md`.
   *État de validation : PARTIEL. T6-qualif K=3 : Wasmtime vs Docker+Python ×4 500–7 375× (H-densité partiellement validé, ADR-0026). Non abouti : comparaison rigoureuse vs Linux+containers telle que définie par ce critère (densité active à latence d'action équivalente). Raison : résultats Linux PoC non transférables sur substrat seL4-natif cible — pas un benchmark manquant, une décision explicite (décision architect 2026-05-27, ADR-0049 §D3).*

Cette thèse est falsifiable : elle sera réfutée si l'un des benchmarks définis dans `benchmarks/reference-workload.md` ou l'un des scénarios d'équivalence fonctionnelle définis dans `benchmarks/equivalence-scenarios.md` n'est pas satisfait par le système sur le hardware de référence.

> **Portée hardware (2026-05-23, révisé 2026-06-08) :** Les propriétés P1–P6 sont définies indépendamment du hardware. Les bornes chiffrées publiées à ce stade sont celles **mesurées sur hardware consumer** (AMD Ryzen 5 PRO 4650U + WD SN530 NVMe PCIe, classe 2) — elles caractérisent ce substrat Linux/NVMe, et non une cible de production. Sur ce hardware, le cap I/O admission control est de 14 agents/s (borne basse toutes classes mesurées). Avec un cycle W1 moyen de 5 s, cela représente un régime stable d'environ 70 agents simultanément actifs — pas plusieurs centaines. La qualification d'un hardware serveur PCIe Gen4 (qui aurait relevé ce cap vers ~100 agents/s) a été **délibérément abandonnée le 2026-05-27 (décision architect)**, et non reportée : les latences absolues mesurées sur Linux/NVMe ne sont pas transférables au substrat de stockage seL4-natif cible, donc qualifier un serveur ne prédirait rien sur la stack réelle (même motif que §3.1 P1, ADR-0049 §D3). La borne conservatrice de 14 agents/s reste la référence jusqu'à un prototype de stockage seL4-natif. Voir `decisions/0026-regime-cache-reference-p3a.md` et `spec/07-plafonds-architecturaux.md §3.3`.

### 3.2 Ce que "réussir" veut dire — critères observables

Le projet réussit si les trois conditions suivantes sont simultanément satisfaites sur le hardware de référence :

**Critère 1 — Densité (P1) :** Le nombre maximum d'agents W1 simultanément actifs sur le système est au moins 5x supérieur au nombre maximum sur la baseline Linux+containers, à p99 de latence d'action équivalent.
*Verdict : satisfait sur l'empreinte dormante uniquement. Ce qui est mesuré et PASS, c'est la densité **hébergée** (P1a / H-densité-hébergée) : Wasmtime vs Docker+Python ×4 500–7 375× sur la RAM par agent dormant (T6-qualif, cible ×5 largement dépassée). La densité **active** que ce critère énonce — débit d'agents W1 actifs à p99 de latence d'action équivalente, soit P1b — n'est pas établie : la comparaison stricte vs Linux+containers a été abandonnée pour non-transférabilité au substrat seL4-natif (voir §3.1 P1).*

**Critère 2 — Rollback (P2) :** Le scénario SEF-2 (rollback à l'action n°500 parmi 1 000 actions) est satisfait avec un hash d'état correct et une durée de rollback ≤ 100ms sur workload W2.
*Verdict : satisfait — SEF-2 PASS (5/5 runs, 17–20 ms pour depth=500, substrat Linux).*

**Critère 3 — Traçabilité (P3) :** Le scénario SEF-5 (lookup causal par `action_id`) est satisfait avec une réponse complète en p99 ≤ 10ms sur un log de 10⁸ actions.
*Verdict : satisfait sur substrat Linux en charge statique — SEF-5 PASS (p99 ≤ 1,9 ms sur 10⁸ actions, lecture seule). Non satisfait sous write concurrent actif (borne 10 ms non garantie sous compaction RocksDB, ADR-0032). Mesure sur substrat seL4 hardware réel : non exécutée (D-P3a, voir §3.1 P3a).*

Ces critères sont détaillés dans `spec/02-properties.md`. Les scénarios sont détaillés dans `benchmarks/equivalence-scenarios.md`.

### 3.3 Pourquoi maintenant — contexte technique et académique

**Les agents long-running sont réels depuis 2023–2025.**

Jusqu'en 2022, les agents IA étaient principalement des démonstrateurs académiques ou des systèmes étroitement contrôlés. À partir de 2023, des systèmes comme Devin (Cognition AI), SWE-agent [Yang 2024], Claude Code (Anthropic), et leurs équivalents ont été déployés en production sur des tâches ouvertes, longue durée, avec accès au système de fichiers, au terminal, et au réseau. Ces systèmes tournent sur Linux+containers — non pas parce que c'est le bon OS pour eux, mais parce que c'est l'OS disponible.

Les limites documentées de cette approche ne sont plus théoriques. Les praticiens qui opèrent ces systèmes rencontrent les mêmes problèmes en production : absence de rollback propre quand l'agent fait une erreur irréversible, observabilité pauvre (on ne sait pas *pourquoi* l'agent a pris une décision, seulement *quoi* il a fait), isolation coûteuse (chaque agent dans son container, avec l'overhead correspondant), et état qui dérive sur des runs longs sans primitive de checkpoint fiable.

**Les workarounds applicatifs confirment le gap.**

L'existence de Temporal [Temporal.io 2019], LangGraph [LangChain 2024], et Durable Execution frameworks est le signal le plus clair que l'OS ne fournit pas les primitives nécessaires. Ces outils réimplémentent, au niveau applicatif, des propriétés qui devraient être des primitives système : Temporal offre de la durable execution (rollback implicite sur timeout, replay déterministe) ; LangGraph offre de la gestion d'état explicite pour des graphs d'agents ; les deux réimplémentent des formes de transactions et de tracing causal au-dessus de Linux.

Cette réimplémentation applicative est le symptôme classique d'une abstraction manquante au bon niveau. Les mêmes propriétés ont été réimplémentées à la couche applicative dans d'autres domaines avant d'être intégrées au niveau système : la gestion de mémoire (GC applicatif avant que les langages le fournissent nativement), la persistance transactionnelle (SQLite réimplémentée dans chaque app avant les SGBD embarqués systématiques), la conteneurisation (chroot applicatif avant les namespaces kernel).

**Les briques théoriques sont matures.**

Les composants nécessaires à un OS pour agents existent individuellement, formellement vérifiés ou prouvés à l'échelle industrielle :

- **Capability systems** : seL4 [Klein 2009] a démontré la vérification formelle d'un microkernel capability-based ; CHERI [Watson 2015] l'a implémenté au niveau hardware en production (Morello, 2022).
- **Stockage content-addressed** : Nix/NixOS [Dolstra 2006] déploie du stockage content-addressed en production depuis 20 ans ; Git l'utilise pour le versioning depuis 2005.
- **Modèle d'acteurs à isolation forte** : BEAM/Erlang [Armstrong 2003] fait tourner des systèmes télécom à des millions d'acteurs concurrents depuis 1986. Les propriétés de tolérance aux pannes et d'isolation des processus BEAM sont vérifiées à l'échelle industrielle.
- **Tracing causal** : les travaux sur la causalité dans les systèmes distribués [Lamport 1978 ; Fidge 1988 — vecteurs d'horloge] fournissent les fondements formels d'un log causal requêtable.

Ce qui n'existe pas encore, c'est l'intégration de ces propriétés dans un OS conçu de bout en bout pour le profil d'agent B — sans les compromis d'un OS généraliste qui doit aussi servir des utilisateurs humains interactifs.

**Le risque est la consolidation autour des mauvaises primitives.**

Les décisions d'infrastructure ont un fort effet de path dependence : une fois que l'écosystème des agents IA s'est construit sur Linux+containers+Temporal, le coût de migration vers des primitives OS plus adaptées devient prohibitif. Ce phénomène est documenté dans l'histoire des OS : les syscalls POSIX, conçus dans les années 1970 pour des systèmes à temps partagé, sont toujours la surface d'interface principale des OS en 2026, malgré leurs inadéquations connues [Elphinstone & Heiser 2013].

Le moment pertinent pour concevoir les bonnes primitives est avant que l'écosystème se fossilise — pas après. Ce projet a débuté comme exercice de spécification pour formuler ces primitives, et a ensuite produit un PoC Rust/Wasmtime/RocksDB validé de bout en bout (phases 5–10) et une stack seL4 opérationnelle sur QEMU virt AArch64 (jalons C.1→C.11-prov). Les propriétés énoncées dans ce document sont donc à la fois des propositions conceptuelles et des propriétés partiellement implémentées et mises à l'épreuve.

---

## 4. Ce qu'on ne vise pas

Les non-objectifs explicites du projet sont documentés dans `spec/05-non-goals.md`. Les trois exclusions structurantes sont :

- **N-rollback-ext** : pas de compensation des effets de bord externes au nœud.
- **N-concurrency-intra** : pas de concurrence intra-agent ; la concurrence est obtenue par décomposition en sous-agents.
- **N-interface-humaine** : pas d'interface humaine interactive. Le superviseur asymétrique dispose d'un client externe qui parle le protocole du système avec des capabilities privilégiées, mais ce n'est pas une interface interactive au sens classique.

Le rôle du superviseur humain est précisément celui d'un **superviseur asymétrique** : il observe (log causal complet), intervient (révocation de capabilities, suspension d'agents, rollback forcé), et autorise (signature d'actions à fort impact). Il n'exécute pas de tâches via l'OS — il n'a pas de session interactive. Son interface est un client externe qui communique avec le système via le même protocole que les agents, avec des capabilities différentes.
