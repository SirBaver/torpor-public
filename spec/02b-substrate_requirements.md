# 02b — Exigences sur le substrat

## 1. Pourquoi ce document existe

### 1.1 Distinction entre propriétés du système et exigences sur la couche d'exécution

Les propriétés P1 à P6 énoncées dans `02-properties.md` décrivent ce que le système, vu de l'extérieur, garantit à ses utilisateurs (agents et superviseur asymétrique). Elles sont formulées sans référence à un substrat d'exécution particulier — c'est-à-dire sans préjuger de la couche technique sur laquelle le système est implémenté (microkernel, runtime managé sur OS hôte, OS langage, unikernel multi-tenant, hyperviseur léger, etc.).

Les propriétés ne sont cependant pas réalisables sur n'importe quel substrat. Elles imposent à la couche d'exécution sous-jacente un ensemble de contraintes précises. Ce document les rend explicites.

La séparation entre propriétés et exigences est délibérée. Elle permet :

- d'évaluer la spec indépendamment d'une réalisation particulière, et donc de préserver la falsifiabilité de la thèse centrale même si plusieurs implémentations alternatives apparaissent ;
- de comparer rigoureusement les substrats candidats (chapitre 03) sur une grille commune, plutôt que de choisir un substrat puis d'en justifier le choix a posteriori ;
- de ne pas trancher prématurément le choix de l'artefact cible — une décision qui demande des arbitrages techniques substantiels et qui fera l'objet d'un ADR dédié, postérieurement à la stabilisation de la spec.

### 1.2 Posture : spec d'abord, choix d'artefact ensuite

Ce projet est un exercice de spécification, pas d'implémentation (voir `01-vision.md` section 3.3 et `README.md`). Conformément à cette posture, le présent document énonce les contraintes que la couche d'exécution doit satisfaire sans choisir laquelle des architectures candidates retenir. Les substrats analysés au chapitre 03 sont évalués par rapport à cette grille ; le choix d'un substrat de référence pour une éventuelle phase d'implémentation est documenté séparément en ADR.

### 1.3 Niveau d'abstraction retenu

Le filtrage par les exigences S1–S7 converge vers un niveau d'abstraction précis, commun à toutes les familles survivantes :

> **Couche d'exécution d'acteurs typés avec IPC médié, communication exclusivement par message, mutation d'état via primitives contrôlées par le runtime, et observabilité interposée à chaque réception et émission.**

Ce niveau est plus haut qu'un microkernel pur (qui n'impose pas de modèle acteur), plus bas qu'un framework applicatif (qui ne peut garantir S1, S3, S4 sans soutien substrat), et orthogonal au choix entre exécution sur bare-metal, sur hyperviseur, ou sur OS hôte. Il exclut structurellement tout substrat où le code agent est du natif libre interagissant directement avec le noyau hôte. La section 5 détaille les conséquences.

---

## 2. Méthode

### 2.1 Une exigence est une assertion vérifiable sur ce que la couche d'exécution doit fournir

Chaque exigence est formulée comme une assertion testable : un substrat candidat doit pouvoir être confronté à l'exigence et classé comme la satisfaisant, la satisfaisant partiellement, ou ne la satisfaisant pas. Une exigence non discriminante (que tout substrat satisferait trivialement) n'a pas sa place dans ce document.

### 2.2 Distinction entre exigences dures et molles

Les exigences sont classées en deux catégories :

- **Exigences dures.** Sans elles, au moins une propriété parmi P1 à P6 ou un scénario d'équivalence fonctionnelle (SEF-1 à SEF-6) est strictement non-satisfaisable. Un substrat qui ne fournit pas une exigence dure est éliminé.
- **Exigences molles.** Sans elles, une propriété est dégradée mais reste atteignable au prix d'un overhead documenté. Un substrat qui ne fournit pas une exigence molle reste candidat, mais avec une note explicite sur la propriété affectée.

### 2.3 Lien avec les propriétés et les SEF

Chaque exigence référence explicitement les propriétés et les scénarios qu'elle conditionne. Le lien inverse (de chaque propriété vers les exigences qu'elle requiert) est résumé dans le tableau de l'annexe 6.1.

---

## 3. Les exigences

---

### S1 — Frontière de confiance entre agents

**Catégorie :** Dure.

**Énoncé :** Le substrat doit fournir un mécanisme garantissant qu'un acteur en exécution ne peut, à aucun moment et par aucun moyen, lire ou écrire l'état d'un autre acteur sans détenir une capability explicite l'y autorisant. Cette garantie doit tenir face à du code potentiellement malveillant, défaillant, ou divergent — pas seulement face à des acteurs coopératifs.

**Mécanismes acceptables.** Trois familles, et seulement trois :

- **(a) Protection mémoire matérielle (MMU/IOMMU).** Chaque acteur dans son propre espace d'adressage. Modèle classique des processus Linux, des microkernels comme seL4. Coût : overhead structurel par espace d'adressage (entrées de table de pages, structures noyau associées).
- **(b) Isolation par typage vérifié.** L'isolation est garantie statiquement par un compilateur ou un vérificateur de bytecode au chargement. Tous les acteurs partagent un espace d'adressage unique ; les frontières de confiance sont enforcées par l'absence de pointer arithmetic non contrôlé et par l'impossibilité d'évasion vers du code natif non vérifié. Modèle : Singularity SIPs, JVM bytecode + module system, WASM Component Model. Coût : contrainte forte sur le langage et le runtime des acteurs ; toute évasion vers du code natif arbitraire détruit la garantie.
- **(c) Capabilities matérielles.** Chaque pointeur est un capability tagué et non-forgeable au niveau hardware. Modèle : CHERI. Coût : matériel non commodity en 2026 (Morello est un prototype industriel ; le déploiement serveur n'est pas généralisé).

**Mécanismes exclus.**

- Toute combinaison où l'isolation repose sur la coopération volontaire de l'acteur (par exemple : "l'acteur ne fait pas de pointer arithmetic dangereux par convention").
- Toute combinaison où l'isolation repose sur des contrôles dynamiques contournables — sandbox seccomp avec brèches connues, AppArmor en mode complain, etc.
- Toute combinaison où du code natif non vérifié peut s'exécuter dans le même espace d'adressage que les acteurs sans frontière matérielle. Ceci inclut notamment BEAM avec NIFs (Native Implemented Functions) arbitraires, qui partagent le heap de la VM et peuvent corrompre l'état d'autres acteurs en cas de bug.

**Conséquence.** Si le système doit héberger du code natif arbitraire pour faire de l'inférence LLM in-process — cas plausible des workloads W1/W2/W3 si l'inférence est exercée nativement — les options (b) seule et (c) seule deviennent contraintes. (b) interdit le natif non vérifié, ce qui restreint le runtime d'inférence à du code vérifiable (WASM, bytecode managé). (c) impose CHERI hardware, non commodity. Reste (a) pure ou des combinaisons hybrides (par exemple : (a) entre groupes d'agents, (b) à l'intérieur d'un groupe).

**Propriétés et SEF conditionnés :** P4 (directement), P1 (indirectement via le coût du mécanisme retenu), SEF-3.

---

### S2 — Capture cohérente d'état d'agent à coût borné

**Catégorie :** Dure.

**Énoncé :** Le substrat doit permettre, pour tout acteur et à tout instant entre deux actions, de capturer une représentation complète et cohérente de son état local au sens du glossaire (état interne de l'acteur, contenu de sa boîte aux lettres, messages en transit interne au nœud qui lui sont destinés et n'ont pas encore été reçus). La capture doit satisfaire trois conditions :

1. **Localité.** Le coût de la capture est borné par la taille de l'état de l'acteur concerné, pas par la taille totale du système. La capture d'un agent ne doit pas requérir une suspension globale ni une opération O(N agents).
2. **Cohérence.** La capture est faite à un point où l'acteur ne traite aucune action — soit passivement (entre deux actions), soit activement (le scheduler le maintient hors-CPU pendant la capture). Aucune mutation de l'état de l'acteur capturé ne peut être appliquée par un autre acteur du système pendant la capture.
3. **Concurrence.** D'autres acteurs peuvent continuer à s'exécuter pendant la capture d'un agent donné. La capture n'est pas un stop-the-world.

**Mécanismes acceptables.**

- Persistent data structures avec partage structurel (style Erlang, ou Clojure persistent collections) : une capture est essentiellement gratuite, c'est un pointeur vers la version courante de la racine.
- Copy-on-write par-acteur, avec un store content-addressed ou un mécanisme équivalent permettant l'accès aux versions antérieures par hash.
- Journaling write-ahead où chaque transition d'état est appliquée d'abord à un journal append-only, et où la capture est l'enregistrement du dernier offset committé.

**Mécanismes exclus.**

- Toute approche basée sur un fork du processus complet (CRIU, fork-then-snapshot) : le coût n'est pas borné par la taille de l'agent mais par la taille de l'image mémoire complète, et la concurrence n'est pas préservée.
- Toute approche basée sur un snapshot de volume (LVM, ZFS, btrfs subvol snapshot) : la granularité est trop grossière (volume entier) et le coût n'est pas borné par la taille de l'agent.
- Toute approche où l'acteur peut allouer de la mémoire arbitraire via des primitives non médiées par le runtime (`mmap` direct, `malloc` libre dans un espace partagé), car la cohérence de la capture ne peut alors plus être garantie.

**Conséquence.** Le substrat ne peut pas être un OS classique laissant chaque acteur gérer son état sans contraintes. Le substrat impose une **discipline de mutation d'état** : toute modification de l'état d'un acteur passe par une primitive contrôlée par le runtime. C'est un engagement architectural fort qui doit être affiché dès la conception du runtime des acteurs.

**Propriétés et SEF conditionnés :** P2 (directement), P6 (directement), SEF-2, SEF-4, SEF-1.

---

### S3 — Causalité observable de chaque réception et émission

**Catégorie :** Dure.

**Énoncé :** Le substrat doit interposer chaque réception et chaque émission de message entre acteurs. Pour chaque action ainsi interposée, le substrat doit exposer au système d'observabilité au minimum les informations suivantes :

- identifiant unique de l'action (action_id) ;
- identifiant de l'acteur source (agent_id émetteur) ;
- identifiant de l'acteur destinataire (agent_id récepteur, ou destination externe identifiée) ;
- timestamp logique compatible avec une relation de causalité partielle — pas nécessairement wall-clock, mais permettant d'établir un ordre causal cohérent (vecteurs de Lamport ou équivalent) ;
- identifiants des capabilities exercées pour autoriser l'action.

L'interposition doit être **systématique** : il ne doit pas exister de chemin de communication inter-acteur qui contourne l'instrumentation du substrat.

**Mécanismes acceptables.**

- IPC kernel-mediated avec hook d'instrumentation (modèle seL4 + serveurs).
- Channels typés du runtime, où le `send` est une primitive du substrat (modèle Erlang, Singularity, WASM Component Model).
- Tout mécanisme garantissant que l'instrumentation ne peut pas être contournée par l'acteur.

**Mécanismes exclus.**

- Mémoire partagée libre entre acteurs.
- Sockets Unix ou pipes utilisés directement par les acteurs sans interposition.
- Syscalls noyau directs pour la communication.
- Toute reconstruction *a posteriori* de la causalité à partir de logs noyau (audit, eBPF) : la complétude n'est pas garantie, et la latence de lookup ne peut pas tenir la borne p99 ≤ 10ms exigée par P3.

**Conséquence.** L'IPC est un point de passage obligatoire. La communication inter-agents est un appel typé du substrat, pas un appel système générique. Cette contrainte exclut implicitement les modèles à mémoire partagée libre — y compris la mémoire partagée *au sein* d'un acteur, qui est de toute façon hors-modèle par le non-objectif `N-concurrency-intra` de `05-non-goals.md`.

**Propriétés et SEF conditionnés :** P3 (directement), P4 (la capability exercée doit être tracée à la source), SEF-3, SEF-5.

---

### S4 — Frontière contrôlée des effets externalisables

**Catégorie :** Dure.

**Énoncé :** Le substrat doit interposer toute opération produisant un effet observable hors du nœud, et permettre au runtime de :

- **détecter** l'opération avant sa finalisation matérielle (avant que le paquet ait quitté la NIC, avant que l'écriture ait été flushée vers un device persistant non géré par le store local content-addressed, avant qu'un signal ait atteint un processus externe) ;
- **déclencher un commit barrier automatique** sur la liste fermée définie dans `06-glossary.md` (envoi réseau, écriture sur device externe, etc.) ;
- **bloquer ou différer** l'opération selon l'état transactionnel courant de l'acteur émetteur — en particulier : bloquer l'opération si l'acteur est dans une transaction non commit et que l'effet n'est pas dans la liste auto-trigger.

La liste des effets considérés comme externalisables est fermée et conservative : en cas de doute sur le caractère externalisable d'une opération, l'opération est traitée comme externe (et donc soumise à interposition).

**Mécanismes acceptables.**

- Drivers IO médiés par le substrat — modèle microkernel avec serveurs de devices, où chaque IO passe par un IPC kernel-mediated.
- Runtime vérifié avec syscalls typés — modèle Singularity, où le manifeste de chaque IPC déclare ses effets et où le compilateur enforce que les acteurs ne peuvent émettre que les effets déclarés.
- Hyperviseur léger interceptant les IO virtualisées — modèle Firecracker ou équivalent, où la frontière VM est la frontière d'interposition.
- WASI (WebAssembly System Interface) : tous les effets passent par des imports déclarés du module, sans accès direct aux syscalls de l'OS hôte.

**Mécanismes exclus.**

- Modèle Linux user/kernel boundary classique où les acteurs émettent directement des syscalls vers le réseau, le système de fichiers, ou d'autres devices, sans couche d'interposition. Ce modèle ne satisfait pas S4 sans ajout d'un médiateur (LD_PRELOAD, ptrace, eBPF d'enforcement) qui réintroduit les exigences ci-dessus.

**Conséquence.** L'IO externe est un objet de première classe du modèle, pas un trou dans le modèle. Cette exigence est probablement la plus discriminante du document : elle exclut Linux comme substrat direct sans une couche d'interposition substantielle. Elle est aussi celle qui rend possible le modèle de commit barrier hybride conservateur (auto-trigger + `commit()` explicite) défini dans `06-glossary.md`.

**Propriétés et SEF conditionnés :** P2 (la portée du rollback est définie par cette frontière), P6 (l'atomicité crash dépend de la cohérence entre état persisté et effets externalisés), SEF-2, SEF-4.

---

### S5 — Ordonnancement préservant la séquentialité par-acteur

**Catégorie :** Dure.

**Énoncé :** Le substrat doit garantir qu'un acteur traite ses actions séquentiellement : à un instant `t`, au plus une action d'un acteur donné est en cours d'exécution. Le substrat peut multiplexer plusieurs acteurs sur un cœur ou répartir les acteurs sur plusieurs cœurs ; il ne peut pas exécuter en parallèle deux actions d'un même acteur.

Le modèle d'exécution doit être **run-to-completion par message** : une action commencée s'exécute jusqu'à sa fin (réception complète et traitement, ou émission complète) avant qu'une autre action du même acteur soit commencée.

**Mécanismes acceptables.**

- Modèle d'acteur classique avec mailbox et boucle run-to-completion (Erlang, Akka avec dispatchers single-threaded, modèle d'acteur original [Hewitt 1973]).
- Modèle co-routine coopératif où chaque acteur est une co-routine et où le scheduler n'interrompt jamais l'exécution d'une action en cours.
- Project Loom (virtual threads) **si et seulement si** le runtime contraint chaque acteur à un unique virtual thread — auquel cas une grande partie de l'attrait de Loom (concurrence interne facile) est explicitement abandonnée.

**Mécanismes exclus.**

- Tout substrat qui expose aux acteurs des primitives de threads, fibres, ou async/await avec parallélisme réel intra-acteur, sans encapsulation par le runtime.
- Tout substrat où le scheduler peut décider d'exécuter en parallèle deux actions du même acteur sur des cœurs différents.

**Conséquence.** Cette exigence formalise au niveau substrat le non-objectif `N-concurrency-intra` de `05-non-goals.md`. Elle est rarement difficile à satisfaire pour les substrats acteur natifs ; elle est difficile pour les substrats généralistes (JVM nue, runtime Linux classique) où il faut activement contraindre les acteurs à ne pas paralléliser.

**Propriétés et SEF conditionnés :** P3 (l'attribution causale dépend de la séquentialité), P4 (la révocation de capability est cohérente parce que l'acteur ne traite qu'une action à la fois), P6 (l'atomicité de transaction est définie sur une séquence linéaire d'actions), SEF-3, SEF-4, SEF-5, SEF-6.

---

### S6 — Source d'horloge et d'entropie isolable et substituable

**Catégorie :** Molle, mais structurante.

**Énoncé :** Le substrat doit fournir aux acteurs l'accès à toute source de non-déterminisme via des primitives explicites du substrat, et doit permettre la substitution de ces primitives lors d'un replay. Les sources concernées incluent au minimum :

- horloge wall-clock (`now()`, `clock_gettime` ou équivalent) ;
- générateur d'aléas (`random()`, accès à `/dev/urandom` ou équivalent) ;
- résultats d'inférence stochastique, si l'inférence est exposée comme primitive du substrat ;
- toute autre source de variation entre exécutions identiques par ailleurs.

En mode replay, ces primitives doivent retourner les valeurs enregistrées dans une trace, plutôt que d'invoquer la source réelle.

**Mécanismes acceptables.**

- API explicite du runtime fournissant `clock()`, `random()`, `inference()`, etc., avec un mode replay qui injecte des valeurs enregistrées.
- Approche style Foundation DB simulator ou Antithesis : toute source de non-déterminisme est wrappée par le runtime de simulation.
- Pour la rétrocompatibilité avec des charges Linux : un substrat utilisant `rr` (Mozilla) ou un équivalent comme couche d'enregistrement-replay externe, avec l'overhead correspondant.

**Mécanismes exclus.**

- Tout substrat où les acteurs peuvent lire l'horloge système ou un générateur d'aléas directement sans passer par une primitive substituable.

**Pourquoi molle.** Sans S6, P1, P2, P3, P4, P6 restent satisfaisables. Mais P5 (déterminisme de transition) et le SEF-6 deviennent non-vérifiables, ce qui dégrade la falsifiabilité de la spec et la débogabilité du système. C'est un coût fonctionnel important mais qui ne casse pas l'architecture.

**Propriétés et SEF conditionnés :** P5 (directement), SEF-6.

---

### S7 — Coût d'overhead par-acteur borné indépendamment du nombre total d'acteurs

**Catégorie :** Dure pour P1.

**Énoncé :** Le substrat doit garantir que faire tourner N acteurs inactifs (en attente de message) consomme au total O(N) ressources, avec une constante par-acteur compatible avec la cible de densité 5× Docker sur W1.

Plus précisément, en référence au hardware de référence défini dans `benchmarks/reference-workload.md` (8 cœurs, 16 GB RAM, NVMe SSD local) :

- le coût mémoire par acteur inactif doit être au plus environ 20% du coût d'un container Docker minimal sur le même hardware ;
- le coût CPU de scheduling par acteur inactif doit être négligeable face au CPU consommé par le traitement d'actions sous workload W1.

**Mécanismes acceptables.**

- Acteurs partageant un espace d'adressage unique avec isolation par typage (modèle BEAM, Singularity SIP, WASM Component Model) : coût par acteur dominé par la taille de son état applicatif, sans surcharge fixe d'espace d'adressage.
- Microkernel à coût d'IPC ultra-faible (modèle seL4 : IPC ~0.5µs, structures kernel minimales par thread) : coût par acteur faible mais avec un poids par espace d'adressage qui dépend de l'architecture.
- Unikernels colocalisés via virtualisation matérielle légère (Firecracker, Kata) : coût par acteur dominé par la taille de l'image de l'unikernel et par l'overhead de la VM.

**Mécanismes exclus.**

- Tout substrat où chaque acteur paie l'overhead complet d'un processus Linux (table de pages dédiée, structures `task_struct`, stack noyau de 8–64 KB).
- Tout substrat où chaque acteur paie l'overhead d'un container OCI complet (rootfs, namespaces, cgroups, dépendances userspace).

**Conséquence.** P1 contraint structurellement le substrat. Tout substrat où la frontière d'isolation est aussi lourde qu'un namespace Linux complet est disqualifié. Cette exigence est cohérente avec S1 : si le mécanisme d'isolation choisi pour S1 est (b) typage ou (c) capabilities matérielles, S7 est facile à satisfaire ; si c'est (a) MMU classique avec un espace d'adressage par acteur, S7 impose alors un microkernel avec un coût d'espace d'adressage très faible — pas un OS généraliste.

**Propriétés et SEF conditionnés :** P1 (directement), conditionne la viabilité de la thèse centrale.

---

## 4. Tableau de discrimination des substrats candidats

Le tableau ci-dessous applique les sept exigences à chaque substrat candidat. Les valeurs utilisées :

- **✓** : le substrat satisfait l'exigence sans modification structurelle.
- **~** : le substrat satisfait partiellement l'exigence, ou la satisfait au prix d'une couche supplémentaire significative.
- **✗** : le substrat ne satisfait pas l'exigence dans sa configuration de référence.

La colonne **Rôle** distingue : **Candidat** (substrat candidat pour l'implémentation), **Baseline** (conservé uniquement comme baseline de mesure pour P1), **Référence** (référence architecturale ou de densité, non candidat), **Hors-modèle** (non applicable au profil visé).

| Substrat | S1 | S2 | S3 | S4 | S5 | S6 | S7 | Verdict | Rôle |
|---|---|---|---|---|---|---|---|---|---|
| Linux + Docker + K8s | ✓ MMU | ✗ CRIU O(N) | ✗ audit reconstructif | ✗ syscalls directs | ✓ par convention | ~ via `rr` | ✗ overhead container | Échec S2, S3, S4, S7 | **Baseline** P1 uniquement |
| Linux + processus + cgroups | ✓ MMU | ✗ idem CRIU | ✗ idem audit | ✗ syscalls directs | ✓ | ~ | ✗ overhead processus | Échec S2, S3, S4, S7 | **Baseline** P1 uniquement |
| BEAM/Erlang production | ✗ NIFs partagent heap | ✓ persistent structures | ✓ `send` interposable | ~ via ports | ✓ run-to-completion | ✓ avec discipline | ✓ acteurs légers | Échec S1 avec NIFs | **Référence** architecture acteur |
| Runtime acteur typé (BEAM-dérivé, sans natif libre) | ✓ si typage seul | ✓ par construction | ✓ par construction | ~ S4 à construire | ✓ par construction | ✓ par construction | ✓ par construction | Compatible — à construire | **Candidat** |
| JVM + Loom (virtual threads) | ~ heap partagé sans frontière | ~ pas de support natif | ~ instrumentation possible | ✗ syscalls Java directs | ~ si discipliné | ~ avec discipline | ✓ virtual threads | Faible S1, S4 | **Référence** densité (S7) uniquement |
| seL4 + serveurs + couche acteur | ✓ MMU + capabilities kernel | ~ à construire | ✓ IPC kernel-mediated | ✓ drivers en serveurs | ✓ par construction | ✓ par construction | ~ dépend couche acteur | Compatible — couche acteur à construire | **Candidat** |
| Singularity / SIPs | ✓ par typage + MSIL | ✓ par construction | ✓ channels typés | ✓ manifeste explicite | ✓ par construction | ✓ par construction | ✓ par construction | Compatible | **Référence** architecturale (non maintenu) |
| MirageOS / unikernels mono-tenant | ✗ un agent = une instance | ✓ par construction | ✗ pas multi-agent | ✓ | ✓ | ✓ | ✗ pas multi-tenant | Hors-modèle | **Hors-modèle** |
| WASM Component Model + WASI Preview 2 | ✓ par typage + sandbox | ~ snapshots WASI possibles | ✓ component interfaces | ✓ WASI Preview 2 mediated | ✓ | ✓ | ✓ | Compatible — conditionnel stabilisation Preview 2 | **Candidat** |
| CHERI + microkernel | ✓ capabilities hardware | ~ dépend couche acteur | ~ dépend couche acteur | ✓ | ✓ par discipline | ✓ par discipline | ✓ | Compatible — hardware non commodity 2026 | **Candidat** (horizon 2028+) |

---

## 5. Conséquences pour la phase de conception

### 5.1 Substrats candidats survivants

Quatre familles de substrats survivent au filtre des sept exigences :

1. **Runtime acteur typé sans natif libre** — inspiré de BEAM mais à construire : un runtime où les acteurs s'exécutent dans un espace de bytecode ou de types vérifiés, sans possibilité d'appeler du code natif arbitraire. BEAM/Erlang production ne survit pas tel quel en raison des NIFs qui partagent le heap de la VM (échec S1 face à du code NIF défaillant). Le candidat est un dérivé qui interdit l'évasion vers du natif non vérifié — ce qui implique soit de contraindre le toolchain (pas de NIFs), soit de construire un runtime nouveau s'inspirant du modèle BEAM. L'écosystème Erlang/OTP reste une référence architecturale.
2. **Microkernel + couche acteur construite au-dessus** — seL4 ou Genode fournissent S1, S3, S4 nativement ; S2 et la couche acteur sont à construire au-dessus.
3. **WASM Component Model + WASI Preview 2** — candidat conditionnel à la stabilisation de WASI Preview 2 (wasi:sockets, component interfaces complètes). En 2026, Wasmtime, WAMR et WasmEdge implémentent Preview 2 à des degrés variables. Le ✓ sur S4 suppose que le runtime d'exécution n'expose aucun syscall direct à l'hôte hors des imports WASI déclarés — ce qui est la promesse du Component Model mais qui doit être vérifié pour chaque runtime cible.
4. **CHERI + microkernel** — capabilities hardware qui résolvent S1 et S4 de manière élégante, mais sur hardware non commodity en 2026 (Morello est un prototype industriel ARM ; pas de déploiement serveur généralisé). Candidat pertinent à horizon 2028+.

### 5.2 Substrats exclus et leur rôle résiduel

- **Linux + Docker + K8s** et **Linux + processus + cgroups** : exclus comme substrats (S2, S3, S4, S7 non satisfaits). Conservés comme **baseline de mesure** pour P1 — c'est contre eux que la thèse de densité 5× se mesure.
- **BEAM/Erlang production** : exclu comme substrat candidat (NIFs cassent S1). Conservé comme **référence architecturale** pour le modèle acteur, la supervision tree, et la gestion de l'état persistent.
- **JVM + Loom** : exclu (heap partagé sans frontière de confiance, syscalls directs). Conservé comme **référence de densité** pour S7 — Project Loom démontre la faisabilité de 10⁶ virtual threads sur JVM.
- **MirageOS / unikernels mono-tenant** : hors-modèle (un agent par instance). Conservé comme **référence de densité hardware par instance** — non commensurable au profil multi-tenant visé.
- **Singularity / SIPs** : compatible sur toutes les exigences, mais système non maintenu depuis 2012. Conservé comme **référence architecturale** — la preuve de concept la plus complète du niveau d'abstraction visé.

### 5.3 Niveau d'abstraction de la spec

Voir section 1.3 pour la formulation synthétique. Le choix de l'artefact cible parmi les quatre familles survivantes est laissé à un ADR ultérieur.

---

## 6. Annexes

### 6.1 Lien entre exigences et propriétés

Le tableau ci-dessous résume, pour chaque propriété, les exigences qui la conditionnent.

| Propriété | Exigences requises | Commentaire |
|-----------|--------------------|-----------|
| P1 Densité | S7 (dure), S1 (indirectement via le choix du mécanisme) | S7 est l'exigence directe ; S1 contraint le mécanisme et son coût. |
| P2 Rollback | S2, S4 | S2 pour la capture, S4 pour la frontière de la portée du rollback. |
| P3 Traçabilité | S3, S5 | S3 pour la causalité observable, S5 pour la cohérence de l'attribution. |
| P4 Isolation | S1, S3 | S1 pour la frontière de confiance, S3 pour tracer les capabilities exercées. |
| P5 Déterminisme | S6 (directe), S5 (séquentialité requise pour le replay) | S6 est directement liée ; S5 est nécessaire pour reproduire l'ordre. |
| P6 Atomicité crash | S2, S4 | Corollaire de P2, mêmes exigences. |

### 6.2 Lien entre exigences et SEF

| SEF | Exigences requises |
|-----|---------------------|
| SEF-1 Persistance après redémarrage | S2 |
| SEF-2 Rollback à une action précise | S2, S4 |
| SEF-3 Isolation des capabilities | S1, S3 |
| SEF-4 Atomicité en cas de crash | S2, S4, S5 |
| SEF-5 Lookup causal d'une action | S3, S5 |
| SEF-6 Déterminisme de transition d'état | S5, S6 |

### 6.3 Exigences considérées et écartées

Plusieurs exigences candidates ont été envisagées et écartées au cours de la rédaction de ce document. Pour mémoire :

- **Garantie FIFO sur les boîtes aux lettres.** Écartée parce que satisfaite trivialement par tous les substrats survivants — non discriminante. Sera mentionnée dans la spec interne du système (boîtes aux lettres, modèle de message), pas comme exigence sur le substrat.
- **Garantie de progression (liveness) du scheduler.** Écartée parce qu'elle entrerait en conflit avec le modèle de supervision (le superviseur asymétrique peut suspendre un acteur). La propriété adressée à la place est une *liveness conditionnelle* documentée dans `02-properties.md` section 4.3.
- **Bornes hard real-time sur la latence d'action.** Écartées par la décision documentée dans `02-properties.md` section 4.4 : les bornes du projet sont statistiques (p99 sous workload), pas hard real-time.