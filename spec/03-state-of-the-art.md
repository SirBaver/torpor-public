# 03 — État de l'art

## 1. Méthode de revue

### 1.1 Critères de sélection

Les systèmes retenus satisfont au moins l'un des critères suivants :

- Ils implémentent au moins une des exigences substrat S1–S7 de manière non-triviale et documentée.
- Ils ont produit des résultats mesurables (benchmarks publiés, déploiements à l'échelle) pertinents pour les propriétés P1–P6.
- Ils constituent des preuves de concept de l'architecture visée, même partiellement.
- Ils sont les systèmes de référence sur lesquels la thèse centrale doit se démarquer.

Sont exclus de ce chapitre : les OS généralistes (Linux, Windows, macOS) qui servent de baseline de mesure dans `benchmarks/reference-workload.md`, et les frameworks applicatifs qui réimplémentent en surcouche des propriétés qui devraient être systémiques (Temporal, LangGraph, LangSmith) — ces derniers sont traités en section 4 comme témoins du gap.

### 1.2 Axes d'analyse communs

Pour chaque système, l'analyse suit le même plan :

1. **Référence** — papier fondateur ou documentation principale.
2. **Ce qu'il fait bien** — contribution concrète, avec chiffres si disponibles.
3. **Ce qu'il ne couvre pas** — limites vis-à-vis de S1–S7 et P1–P6.
4. **Ce qui est directement réutilisable** — algorithmes, patterns, ou implémentations utilisables dans le PoC.

---

## 2. Systèmes existants pertinents

---

### 2.1 seL4 — microkernel formellement vérifié

**Référence principale :** [Klein 2009] "seL4: Formal Verification of an OS Kernel", SOSP 2009. Implémentation : github.com/seL4/seL4, licence GPLv2/BSD.

**Ce qu'il fait bien.**

seL4 est le seul microkernel de production dont la correction fonctionnelle est formellement prouvée en Isabelle/HOL [Klein 2009]. Cette preuve couvre la correspondance entre la spécification abstraite (ce que le kernel est censé faire) et l'implémentation C (ce qu'il fait réellement). Elle exclut les side-channels temporels, les bugs du compilateur, et le hardware.

Le modèle de capabilities de seL4 est la référence pour S1 et P4. Chaque objet kernel (thread, page de mémoire, port IPC, CNode) est accessible uniquement via une capability stockée dans un CNode. La délégation se fait par `Mint` (copie avec atténuation) ou `Copy`. La révocation se fait via `Revoke` sur le CNode parent, qui invalide récursivement toutes les dérivées — c'est le mécanisme que l'hypothèse H-revoke adopte directement.

Performance IPC mesurée : ~0,4 µs sur ARMv7 [Heiser 2020 "seL4 is free, what does that mean for you?"], ~1 µs sur x86-64. C'est le plancher de référence pour une communication inter-acteur sécurisée avec vérification de capability.

**Ce qu'il ne couvre pas.**

seL4 est un microkernel, pas un OS acteur. Il ne fournit pas de modèle d'acteur, pas de capture d'état d'agent (S2), pas d'observabilité causale native (S3 — l'IPC est médié mais non instrumenté par défaut), et pas de store content-addressed (P2). Au-dessus de seL4, tout est à construire — le scheduling applicatif, les abstractions acteur, la gestion d'état.

S6 (sources de non-déterminisme substituables) est possible à construire au-dessus mais n'existe pas nativement. S7 (coût par acteur) dépend de la couche acteur construite au-dessus : un espace d'adressage seL4 coûte des ressources kernel non-négligeables ; il faut une politique de partage d'espaces d'adressage entre acteurs de confiance pour atteindre P1.

**Ce qui est directement réutilisable.**

- **Le modèle CNode + Revoke** est le blueprint de H-revoke. L'implémentation de référence pour la propagation récursive de révocation est connue et ses coûts sont mesurés.
- **Les chiffres IPC** (~1 µs sur x86-64) donnent le plancher de latence d'action pour toute architecture où chaque message inter-acteur passe par le kernel. Au-dessus de ce plancher, la latence dépend du modèle acteur, pas du mécanisme de protection.
- **La discipline de minimal TCB** (Trusted Computing Base réduit au maximum) est le principe structurant pour décider ce qui appartient au substrat vs. à la couche acteur.

---

### 2.2 Plan 9 / Inferno — namespaces et protocole 9P

**Référence principale :** [Pike 1990] "Plan 9 from Bell Labs", USENIX Summer 1990. [Dorward 1997] "Inferno", Bell Labs Technical Journal.

**Ce qu'il fait bien.**

Plan 9 généralise "everything is a file" jusqu'à sa conclusion logique : toute ressource (processus, réseau, interface graphique, espace de noms) est exposée comme un système de fichiers accessible via le protocole 9P. Chaque processus a son propre namespace — une vue personnalisée de l'arbre de ressources — ce qui permet une isolation par composition plutôt que par exclusion.

Inferno applique le même modèle dans une VM portable, avec le langage Limbo (GC, typage statique, canaux de communication typés inspirés de CSP [Hoare 1978]).

Le protocole 9P est l'ancêtre des interfaces déclaratives pour ressources système : une ressource est quelque chose qu'on peut `open`, `read`, `write`, `stat`. Cette uniformité est une forme primitive de S3 (causalité observable) et de l'idée d'état déclaratif exposé en section 2.2 de `01-vision.md`.

**Ce qu'il ne couvre pas.**

Plan 9 ne dispose d'aucun modèle de capability au sens de [Dennis & Van Horn 1966] — la sécurité est basée sur des ACL UNIX étendus, liés à l'identité. Il n'y a pas de rollback transactionnel, pas de store content-addressed, pas de log causal structuré. Le modèle de confiance (identité + ACL) est précisément celui que la section 2.3 de `01-vision.md` identifie comme inadapté aux agents IA.

**Ce qui est directement réutilisable.**

- Le principe de **namespace par acteur** (chaque acteur voit une vue différente de l'arbre de ressources) est applicable au modèle de capability : la vue qu'un acteur a des ressources accessibles est l'ensemble des capabilities qu'il détient — pas un chemin dans un arbre global.
- L'uniformité d'interface (tout est une ressource adressable par un protocole unique) est un principe applicable au design de S3 : toute ressource du système est adressable via le même protocole de capability, qu'il s'agisse d'un acteur, d'un sous-arbre du store, ou d'un effet externalisable.

---

### 2.3 NixOS / Nix — store content-addressed et déploiement déclaratif

**Référence principale :** [Dolstra 2006] "The Purely Functional Software Deployment Model", PhD thesis, Utrecht University. Implémentation : github.com/NixOS/nix.

**Ce qu'il fait bien.**

Nix applique le paradigme fonctionnel pur au déploiement de logiciels : chaque artefact (package, configuration, environnement) est le résultat d'une *dérivation* — une fonction pure dont les inputs sont déclarés et hashés. Les outputs sont stockés dans `/nix/store/<hash>-<name>/` et sont immuables. Deux dérivations avec les mêmes inputs produisent exactement le même output (reproductibilité cryptographique).

Le store Nix est un DAG orienté de dérivations identifiées par leur hash SHA-256. La navigation du graphe de dépendances est O(log N) dans le cas d'un arbre équilibré. Le garbage collection élimine les dérivations non-référencées.

C'est l'implémentation la plus mature du principe de content-addressed storage pour des artefacts système. Elle satisfait directement l'infrastructure nécessaire à P2 (rollback O(log N)) et à S2 (capture d'état cohérente) pour des artefacts statiques.

**Ce qu'il ne couvre pas.**

Le store Nix est conçu pour des artefacts de build statiques, pas pour de l'état runtime mutant à haute fréquence. Une dérivation est immuable une fois construite ; il n't existe pas de primitive pour "l'état courant de l'acteur A après son action n°42573". Les mécanismes de garbage collection de Nix sont calibrés pour des artefacts de quelques MB à quelques GB, pas pour des snapshots d'acteurs à 10⁵ écritures/heure.

NixOS (l'OS construite sur Nix) ajoute la déclarativité de configuration système, mais reste fondamentalement un OS Linux — les exigences S2, S3, S4 ne sont pas satisfaites.

**Ce qui est directement réutilisable.**

- **La structure du store** — DAG de contenus adressés par hash, avec partage structurel — est le modèle direct pour le store d'état actoriel. L'état de l'acteur après l'action n°N est un nœud dans le DAG ; le rollback à l'action n°M est un pointeur vers un nœud antérieur. L'implémentation Nix (en particulier la gestion des références et le GC) est une référence d'implémentation.
- **Les dérivations comme unités reproductibles** sont l'analogue des *intentions* (niveau applicatif, `06-glossary.md`) : une intention est une fonction pure `(état_initial, séquence_de_messages) → état_final` dont les inputs sont enregistrés et le résultat est contentaddressable.
- **Bibliothèque de référence :** la librairie `nix-store` C++ et le daemon `nix-daemon` sont des implémentations de production d'un store content-addressed. Le PoC peut s'en inspirer directement, ou utiliser une base de données clé-valeur avec hashing explicite (RocksDB + SHA-256) comme approximation plus légère.

---

### 2.4 Genode — OS framework capability-based

**Référence principale :** [Feske 2014] "Genode OS Framework Foundations", genode.org/documentation/genode-foundations. Implémentation : github.com/genodelabs/genode.

**Ce qu'il fait bien.**

Genode est un framework pour construire des OS capability-based au-dessus de différents microkernels (seL4, Fiasco.OC, hw). Son modèle de composants est structuré : chaque composant déclare ses services (interfaces exportées) et ses besoins (interfaces requises). Les connexions entre composants sont des capabilities. Un composant ne peut interagir qu'avec ce que son parent lui a accordé — modèle sandbox par composition.

Genode satisfait S1 (isolation via microkernel sous-jacent), S3 (IPC via capabilities kernel-mediated), et S4 (drivers comme composants). Sa documentation (les "Genode Foundations") est l'une des meilleures introductions à l'architecture capability-based multi-composants.

**Ce qu'il ne couvre pas.**

Genode n'a pas de modèle acteur, pas de store content-addressed, et pas d'observabilité causale native. C'est un OS framework, pas un système conçu pour la densité de processus légers. Chaque composant Genode a un coût similaire à un thread kernel — incompatible avec S7 à grande échelle (profil B, 10³ à 10⁵ agents simultanés).

**Ce qui est directement réutilisable.**

- Le **modèle de connexion parent-enfant** pour la délégation de capabilities est directement applicable au spawning de sous-agents (profil B, critère e) : quand un agent spawne un sous-agent, il lui délègue une connexion Genode-style — la capability du sous-agent est dérivée de la sienne, avec atténuation.
- La documentation des Genode Foundations est une référence pédagogique de qualité pour les équipes qui abordent les capability systems.

---

### 2.5 BEAM / Erlang OTP — modèle acteur, isolation par VM, supervision

**Référence principale :** [Armstrong 2003] "Making Reliable Distributed Systems in the Presence of Software Errors", PhD thesis, KTH. [Hebert 2013] "Learn You Some Erlang for Great Good!". Implémentation : github.com/erlang/otp.

**Ce qu'il fait bien.**

BEAM est l'implémentation de production la plus mature du modèle acteur pour des systèmes à haute disponibilité. Ses propriétés mesurées sont directement pertinentes pour les cibles de ce projet :

- **Densité de processus :** 10⁶ à 2×10⁶ processus BEAM par machine sur hardware standard. Chaque processus coûte ~300 octets de mémoire minimale et une entrée dans la table des processus. Le scheduler BEAM est un scheduler coopératif par réduction (une "réduction" ≈ un appel de fonction), sans préemption temporelle — ce qui évite les coûts du context-switch kernel à haute fréquence.
- **Création de processus :** ~1 µs sur hardware moderne. Directement applicable à la cible de spawn de sous-agents.
- **Message send (local) :** ~0,5 µs à ~2 µs selon la taille du message. Plancher de référence pour la latence d'action (S3).
- **Persistent data structures :** les termes Erlang sont immuables. Une "modification" d'une structure de données crée une nouvelle version avec partage structurel — la version antérieure est accessible tant qu'une référence la pointe. C'est exactement le mécanisme requis pour S2 (capture cohérente d'état à coût borné).
- **Supervision trees (OTP) :** un superviseur redémarre les processus défaillants à un état initial connu. C'est une forme de tolérance aux pannes qui satisfait partiellement P6 (atomicité crash) — le processus relancé repart d'un état stable prédéfini, pas d'un état arbitraire pré-crash.
- **Run-to-completion par message :** chaque processus traite un message jusqu'à sa fin avant d'en traiter un autre — S5 est satisfait par construction.

**Ce qu'il ne couvre pas.**

Le point de défaillance critique de BEAM vis-à-vis de S1 est les **NIFs (Native Implemented Functions)** : des extensions natives en C/Rust qui s'exécutent dans le heap de la VM, sans frontière de confiance. Une NIF défaillante peut corrompre l'état de n'importe quel processus BEAM. L'écosystème Erlang/Elixir en production utilise massivement les NIFs (pilotes de BDD, crypto, bindings ML) — les interdire revient à interdire l'écosystème.

BEAM ne fournit pas de capability system (S1 ne tient pas face à du code NIF arbitraire), pas d'interposition explicite des effets externes (S4 — les ports sont une approximation, pas une interposition systématique), pas de store content-addressed pour l'état runtime (S2 existe via les persistent structures mais n'est pas intégré à un store adressable par hash), et pas de sources de non-déterminisme substituables (S6 — `erlang:system_time/1` est un appel direct à l'OS).

**Ce qui est directement réutilisable.**

BEAM est le point de départ le plus naturel pour un PoC. Trois éléments sont directement réutilisables :

1. **Le scheduler BEAM** — son modèle de réductions coopératives, son scheduler work-stealing multi-core (chaque core Erlang = un scheduler OS thread), et ses paramètres de tuning (`+S`, `+P`) sont une référence directe pour concevoir un scheduler d'acteurs légers compatible S5 et S7.

2. **Les persistent data structures Erlang** — l'implémentation de la heap per-process avec garbage collection générationnel per-process (pas de GC global) est la référence pour S2. Chaque processus a son propre GC, ce qui évite les pauses globales. Pour le PoC, utiliser une runtime BEAM (Erlang ou Elixir) comme substrat d'expérimentation est le chemin le plus court pour valider P1 (densité) et P2 (rollback via persistent structures).

3. **OTP GenServer + Supervisor** — le pattern `GenServer` (un acteur avec état persistant, callback `handle_call/handle_cast`) est exactement le modèle d'agent visé. Le PoC peut implémenter un agent profil B comme un `GenServer` OTP avec une couche de store content-addressed ajoutée, pour mesurer P2 sans construire de runtime from scratch.

**Chiffres de référence pour le PoC :**

| Métrique | Valeur BEAM mesurée | Cible PoC |
|----------|-------------------|-----------|
| Processus simultanés (inactifs) | 10⁶–2×10⁶ | ≥ 5× agents Docker (P1) |
| Coût mémoire par processus inactif | ~300 bytes + heap | À mesurer sur workload W1 |
| Latence message send local | 0,5–2 µs | Plancher latence action |
| Création de processus | ~1 µs | Plancher spawn sous-agent |
| Durée GC per-process sur heap 50 MB | ~1–5 ms | À mesurer (impact P3) |

---

### 2.6 Unison — langage content-addressed

**Référence principale :** [Chiusano & Bjarnason 2021] "Unison: A new approach to distributed programming", unisonweb.org/docs. Implémentation : github.com/unisonweb/unison.

**Ce qu'il fait bien.**

Unison est un langage où toute définition (fonction, type, valeur) est identifiée par le hash de son arbre syntaxique normalisé — pas par son nom. Renommer une fonction ne casse aucune dépendance. La base de code est un DAG content-addressed, pas une collection de fichiers texte mutables.

La conséquence directe : un programme Unison est reproductible par construction. Deux versions différentes d'une même fonction peuvent coexister dans la même base de code sans conflit de noms. L'historique de la base de code est un DAG de hashes, navigable en O(1) par identifiant.

**Ce qu'il ne couvre pas.**

Unison est un langage, pas un OS. Il ne fournit pas d'isolation entre acteurs (S1), pas d'IPC médié (S3), pas d'interposition des IO (S4). Son modèle distribué (Unison Cloud) est intéressant mais distinct du problème substrat.

**Ce qui est directement réutilisable.**

- **L'identification par hash** de chaque définition est le modèle pour identifier les *intentions* et les *actions* dans le log causal : `action_id = hash(agent_id ‖ parent_action_id ‖ payload)`. Ce schéma garantit l'unicité globale sans coordination centrale et rend le log causal content-addressed.
- **La normalisation de l'arbre syntaxique avant hashing** est applicable à la sérialisation des états d'acteur avant stockage dans le store : normaliser la représentation avant de hasher garantit que deux états sémantiquement équivalents produisent le même hash (déterminisme de P5).

---

### 2.7 WASM Component Model + WASI Preview 2

**Référence principale :** [Bytecode Alliance 2023] "WebAssembly Component Model", component-model.bytecodealliance.org. [WASI Preview 2 2024], wasi.dev. Runtimes : Wasmtime (Mozilla/Bytecode Alliance), WAMR (Intel), WasmEdge.

**Ce qu'il fait bien.**

Le WebAssembly Component Model est l'architecture la plus aboutie en 2026 pour une isolation par typage sans MMU dédiée (mécanisme S1(b)). Un composant WASM est un module dont les imports et exports sont déclarés via des interfaces WIT (WebAssembly Interface Types) — des types algébriques compilés vers le bytecode WASM. Le composant ne peut accéder qu'aux ressources pour lesquelles il a un import déclaré : il n'y a pas d'accès ambient à la mémoire, aux syscalls, ou au réseau.

WASI Preview 2 standardise les interfaces système (IO, réseau, sockets, horloge) comme des imports WASM, médiatisés par le runtime. Un composant qui veut accéder à l'horloge doit importer `wasi:clocks/wall-clock` ; si le runtime injecte une horloge fictive, le composant ne peut pas le détecter — ce qui satisfait directement S6.

Performance mesurée sur Wasmtime 14+ (2024) :
- Temps d'instanciation d'un composant : ~50–200 µs (contre ~1 µs pour un processus BEAM).
- Overhead d'un appel cross-component via interface typée : ~100–500 ns.
- Empreinte mémoire par composant inactif : quelques KB de bytecode + heap propre.

**Ce qu'il ne couvre pas.**

S2 (capture d'état cohérente) n'est pas standardisé dans WASI Preview 2. Le Wasm Snapshot proposal est en discussion mais non stabilisé en 2026. Sérialiser l'état d'un composant WASM en cours d'exécution n'est pas trivial : la stack WASM et les valeurs locales ne sont pas directement accessibles depuis l'extérieur du runtime.

S3 (causalité observable à chaque réception/émission) dépend du runtime : Wasmtime n'instrumente pas nativement chaque appel cross-component dans un log causal. C'est à construire comme un wrapper autour des interfaces WIT.

Le temps d'instanciation (~100 µs) est 100× plus lent que le spawn BEAM (~1 µs). Pour des agents long-running (profil B), le coût de spawn est amorti sur la durée de vie — mais pour des workloads W3 avec spawning fréquent de sous-agents, cet overhead est mesurable.

**Ce qui est directement réutilisable.**

- **Wasmtime + WASI Preview 2** est le runtime d'isolation le plus déployable en 2026 pour un PoC qui cible S1(b) (isolation par typage). La chaîne d'outils (Rust → WASM Component, WIT bindings) est mature.
- **Les interfaces WIT** sont le format naturel pour définir les capabilities inter-acteurs : une capability est un import WIT que le runtime accorde ou non à un composant. C'est P4 (isolation par capabilities) implémentée au niveau toolchain.
- **L'injection de WASI clocks** est la démonstration de S6 : remplacer `wasi:clocks/wall-clock` par une implémentation déterministe dans les tests est le mécanisme exact pour valider P5 (déterminisme de transition) et SEF-6.
- **Wasmtime en Rust** expose une API embarquable ; construire un runtime acteur léger autour de Wasmtime (un acteur = un composant WASM, message passing via une queue Rust, store content-addressed via RocksDB) est une architecture de PoC viable en quelques semaines de travail.

---

### 2.8 Singularity / Midori (Microsoft Research) — référence architecturale

**Référence principale :** [Hunt & Larus 2007] "Singularity: Rethinking the Software Stack", ACM SIGOPS OSR. [Duffy 2015] "15 years of concurrency", joeduffyblog.com (référence sur Midori).

**Ce qu'il fait bien.**

Singularity (2003–2007) et son successeur Midori (2008–2015) sont les implémentations les plus complètes d'un OS construit sur des principes proches de ceux de ce projet. Ils satisfont l'ensemble des exigences S1–S7 par construction :

- **S1 (isolation) :** Software Isolated Processes (SIPs) dans Singularity. Chaque processus est un module Sing# (extension de C#) vérifié par le compilateur et le vérificateur de bytecode MSIL. Pas de mémoire partagée entre SIPs — uniquement des canaux typés. Aucun code natif non vérifié.
- **S2 (capture d'état) :** dans Midori, les objets sont immuables par défaut (via le système de types), ce qui rend les snapshots triviaux.
- **S3 (causalité observable) :** les canaux typés de Singularity sont la surface IPC ; chaque échange est un appel typé, instrumentable nativement.
- **S4 (effets externalisables) :** le manifeste de chaque SIP déclare ses effets autorisés. Le compilateur enforce que le processus ne peut émettre que les effets déclarés.
- **S5, S6, S7 :** par construction, via le modèle de types et le scheduler Midori.

Midori a également atteint des performances compétitives avec Linux sur des workloads systèmes : les benchmarks internes de Microsoft citent par Duffy [2015] montrent une latence IPC inférieure à Linux et une densité de processus légers supérieure — sans isolation MMU par processus.

**Pourquoi ce n'est pas le point de départ du PoC.**

Singularity a été abandonné en 2012. Midori a été abandonné vers 2015, sans publication complète des sources. Le codebase n'est pas disponible. La chaîne d'outils (Sing#, Spec#, Bartok compiler) est non maintenue et non reproductible.

Singularity/Midori est la preuve de concept la plus convaincante que l'architecture visée est réalisable et performante — mais elle ne produit pas de code réutilisable directement.

**Ce qui est réutilisable.**

- Les écrits de Joe Duffy sur Midori (blog joeduffyblog.com, séries "The Error Model", "15 years of concurrency", "Safe Systems Programming in Rust and C++") sont la référence conceptuelle la plus riche disponible publiquement. Chaque décision architecturale de Midori (pas de mémoire partagée, canaux typés, manifest-declared capabilities, garbage collection par région) est documentée avec ses motivations et ses coûts.
- Le modèle de **manifest-declared capabilities** de Singularity (chaque composant déclare dans son manifeste les effets qu'il peut produire, vérifiés au chargement) est le pendant statique du commit barrier hybride (S4) : les effets statiquement déclarables sont auto-triggés ; les effets dynamiques requièrent un `commit()` explicite.

---

### 2.9 Temporal, Antithesis, Foundation DB — témoins du gap applicatif

Ces systèmes ne sont pas des substrats OS mais des frameworks applicatifs. Leur existence confirme le gap identifié en `01-vision.md` section 3.3 : les propriétés P2, P3, et P5 sont suffisamment demandées pour être réimplémentées en couche applicative, faute de primitives systémiques.

**Temporal** [Temporal.io 2019] — durable execution en surcouche applicative. Un workflow Temporal persiste son historique d'événements et peut reprendre après crash en rejouant l'historique. C'est P2 (rollback) + P3 (traçabilité) implémentés dans une queue de messages persistante. Coût : chaque opération Temporal est une écriture dans un backend (Cassandra, PostgreSQL) — latence de l'ordre de la milliseconde, pas de la microseconde. Temporal ne fournit pas S1 (isolation entre workflows), S4 (interposition des IO), ou S7 (densité d'agents légers).

**Antithesis** [Reynolds 2023] / **Foundation DB** [Kulkarni 2023 "Testing Distributed Systems with Simulation"] — simulation déterministe pour tester des systèmes distribués. L'idée centrale : si toutes les sources de non-déterminisme (réseau, disque, horloge) sont contrôlées par un simulateur, les tests sont reproductibles et les bugs sont trouvables par fuzzing déterministe. C'est P5 (déterminisme de transition) appliqué à la couche test. Antithesis construit un hyperviseur qui intercepte tous les accès non-déterministes — c'est S6 implémenté au niveau hyperviseur plutôt qu'au niveau substrat. Coût : overhead de virtualisation permanent, pas de garantie de densité.

**Ce que ces systèmes prouvent :** les ingénieurs qui font tourner des agents en production ont besoin de P2, P3, et P5 — suffisamment pour payer le coût d'une couche applicative supplémentaire. Si ces propriétés étaient des primitives substrat, le coût serait structurellement moindre.

---

## 3. Tableau de synthèse

Pour chaque propriété P1–P6, quel système l'approche le mieux, et avec quelles limites.

| Propriété | Meilleure approximation existante | Limite principale | Référence directe pour le PoC |
|-----------|----------------------------------|-------------------|-------------------------------|
| P1 Densité | BEAM/Erlang (10⁶ acteurs/machine) | NIFs cassent S1 ; pas de capability system | Scheduler BEAM, chiffres de référence |
| P2 Rollback | Nix store (DAG content-addressed) | Conçu pour artefacts statiques, pas état runtime | Structure du store Nix ; persistent structures BEAM |
| P3 Traçabilité causale | Temporal (log d'événements persistant) | Applicatif, pas systémique ; latence ms, pas µs | Modèle event sourcing de Temporal |
| P4 Isolation capabilities | seL4 (CNode + Revoke formellement vérifié) | Pas de modèle acteur ; S2 absent | Modèle CNode, chiffres IPC |
| P5 Déterminisme transition | Antithesis / Foundation DB simulation | Implémenté au niveau hyperviseur/test, pas substrat | Modèle d'injection de non-déterminisme |
| P6 Atomicité crash | OTP Supervisor (redémarrage à état stable) | Convention de design, pas garantie transactionnelle | Pattern GenServer + état persisté |

Aucun système existant ne satisfait simultanément P1–P6 sur un substrat unique conçu pour ce profil. Les systèmes qui s'en approchent le plus (Singularity/Midori) ne sont pas disponibles.

---

## 4. Le gap — ce qui n'existe pas

Le gap n'est pas dans les algorithmes ou les techniques individuelles. Chaque propriété P1–P6 a une implémentation de référence quelque part :

- P1 → BEAM scheduler
- P2 → Nix store
- P3 → event sourcing (Temporal, EventStoreDB)
- P4 → seL4 capabilities
- P5 → Antithesis simulation
- P6 → OTP supervision

**Le gap est dans l'intégration.** Aucun système ne combine ces propriétés dans un substrat unique conçu pour le profil agent B (durée 1h–1 mois, 10⁴–10⁸ actions, supervision ponctuelle, spawning de sous-agents). Les systèmes qui le font partiellement (BEAM) ont des trous critiques dans le modèle de confiance (NIFs). Le seul système qui le fait pleinement (Singularity/Midori) n'est pas disponible.

La deuxième dimension du gap est le profil cible. aucun des systèmes ci-dessus a été conçu en ayant en tête un agent IA long-running comme utilisateur primaire. BEAM a été conçu pour des switch téléphoniques. Nix a été conçu pour la reproductibilité de builds. seL4 a été conçu pour des systèmes embarqués critiques. Leurs choix de conception reflètent ces contextes. Le profil B (agent stochastique, supervision asymétrique, rollback fréquent, log causal requêtable à l'échelle) introduit des contraintes que ces systèmes n'ont pas eu à résoudre.

### 4.1 Implications pour le PoC

Le tableau suivant résume, pour chaque hypothèse bloquante du projet, quel système existant permet de la tester le plus rapidement.

| Hypothèse à invalider | Système existant à utiliser | Travail de PoC minimal |
|----------------------|----------------------------|----------------------|
| H-densité-hébergée : ≥5× Docker idle | BEAM/Erlang en baseline | Benchmark W1 idle sur BEAM vs Docker : mesurer ratio overhead par agent |
| H-densité-active : ≥2× Docker débit actif | BEAM/Erlang en baseline | Benchmark W1 actif sur BEAM vs Docker : mesurer débit actions/s et p99 latence |
| H-revoke : CNode+TTL à coût < 5% CPU | seL4 ou simulation en userspace | Benchmark de révocation sur arbre synthétique de N capabilities |
| H-rollback-perf : ≤ 100ms pour N=100 | Nix store ou RocksDB content-addressed | Micro-benchmark snapshot/restore d'un état 50 MB sur 100 actions |
| H-tracing : O(1) lookup p99 ≤ 10ms sur 10⁸ actions | EventStoreDB ou RocksDB + index | Benchmark de lookup sur log synthétique de 10⁸ entrées |
| H-wasm-isolation : isolation par typage sans overhead MMU | Wasmtime + WASI Preview 2 | Benchmark densité de composants WASM vs processus Linux |

**Architecture de PoC recommandée.**

L'architecture qui minimise le travail de construction tout en testant les hypothèses bloquantes est :

```
Wasmtime (isolation S1(b), S4, S6 via WASI)
  + scheduler Rust async (Tokio) pour S5 et S7
  + store RocksDB content-addressed pour S2 et P2
  + log causal append-only indexé (RocksDB column family) pour P3
  + capability tracking en mémoire (hash map agent_id → capability set) pour P4
```

Cette architecture est entièrement en Rust (Wasmtime est écrit en Rust, RocksDB a des bindings Rust matures, Tokio est le scheduler async de référence). Elle est déployable sur Linux hôte sans modifier le kernel, ce qui correspond aux limites explicites du prototype phase 2 définies dans `04-hypotheses.md`. Elle teste directement les hypothèses H-densité-hébergée et H-densité-active (composants WASM vs containers), H-rollback-perf (store RocksDB), et H-tracing (log indexé).

Le seul point non couvert par cette architecture est H-revoke (arbre de dérivation des capabilities) — qui peut être ajouté comme une couche supplémentaire en mémoire sans modifier le substrat Wasmtime.
