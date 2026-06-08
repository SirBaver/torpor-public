# 02 — Propriétés visées

## 1. Méthode : comment on sélectionne et formule une propriété

### 1.1 Critère d'acceptation : une propriété est une assertion vérifiable

Une propriété n'est retenue que si elle peut être vérifiée expérimentalement, soit sur un workload de référence (W1, W2 ou W3 définis dans `benchmarks/reference-workload.md`), soit sur un scénario d'équivalence fonctionnelle (SEF-1 à SEF-6 définis dans `benchmarks/equivalence-scenarios.md`). Une propriété non mesurable est une intention, pas une propriété.

La distinction entre les deux modes de vérification est utile :

- Les **workloads** mesurent *combien* le système atteint d'agents, *à quelle latence*, *avec quel débit*. Ils servent à vérifier les propriétés quantitatives — typiquement, P1, et les bornes de performance attachées à P2 et P3.
- Les **SEF** vérifient *que* le système produit le comportement correct dans des conditions précises. Ils servent à vérifier les propriétés qualitatives — typiquement, P4, P5, P6 — ainsi que la correction des comportements promis par P2 et P3 indépendamment de leurs bornes de performance.

### 1.2 Structure d'entrée : nom, définition formelle, métrique, niveau de priorité

Chaque propriété est documentée avec : nom court, définition formelle ou opérationnelle, métrique de vérification (comment on mesure), référence à l'état de l'art (qui a déjà défini ou approché cette propriété), et coût connu (ce qu'on sacrifie pour l'obtenir).

### 1.3 Propriétés visées vs. exigences sur le substrat

Les propriétés énoncées dans ce document sont des propriétés **du système** — ce que le système, vu de l'extérieur, garantit à ses utilisateurs (agents et superviseur asymétrique). Elles ne préjugent pas du substrat d'exécution (microkernel, runtime managé, OS langage, etc.) sur lequel le système sera implémenté.

Les contraintes que ces propriétés imposent à la couche d'exécution sous-jacente sont documentées séparément dans `02b-substrate-requirements.md`. Cette séparation est délibérée : elle permet d'évaluer la spec indépendamment d'une réalisation particulière, et elle rend explicite la grille d'évaluation des substrats candidats analysés au chapitre 03.

---

## 2. Les propriétés

---

### P1 — Densité d'agents

La propriété de densité se décompose en deux hypothèses et métriques distinctes, adaptées à deux régimes d'utilisation d'agents différents : l'hébergement d'agents parqués (dormants, sans inférence en cours) et l'inférence d'agents actifs simultanés.

#### P1a — Densité hébergée

**Nom court :** Densité hébergée

**Définition formelle :** Le système peut maintenir en état idle (parqué) un nombre d'agents au moins R_idle fois supérieur à la baseline Docker, sur le même hardware physique, à mémoire RAM constante.

**Métrique :** Consommation de mémoire RAM (RSS) par agent dormant, mesurée en KB/agent. Comparaison : Wasmtime/Tokio idle vs Docker baseline (container avec runtime agent LLM : Python 3.11 + dépendances, ~100–200 MB RSS à l'état idle).

**Définition opérationnelle de "agent idle":** Agent en état `LifecycleState::Suspended` ou `::Active` mais sans inférence en cours, attendant un `Message::Data` dans son inbox Tokio. Aucune écriture store en cours.

**Borne de performance :** R_idle ≥ 5× pour valider H-densité-hébergée. Sur 16 GB de RAM avec un overhead Wasmtime idle ≤ 5 KB/agent et une baseline Docker réaliste (Python runtime) ≥ 100 MB/agent, le système doit supporter un ratio minimal de 5× plus d'agents parqués que la baseline.

**Note de transparence :** L'écart mesuré provient du contraste CoW (Copy-on-Write) Wasmtime vs interpréteur Python résident. Les pages WASM d'un agent dormant demeurent virtuelles (non faulted) en mémoire physique (~5 KB overhead de page tables et métadonnées runtime). Un container Python, au contraire, conserve l'interpréteur et ses dépendances résident en RAM même entre deux actions (~100–200 MB). Ce différentiel est attendu et reflète une propriété architecturale légitime du substrat, non un artefact de mesure.

**Référence état de l'art :** Les systèmes d'acteurs légers (Erlang/OTP, Akka) obtiennent une densité d'agents parqués de 10⁶ à 10⁷ par machine en renonçant à la protection mémoire matérielle (un seul espace d'adressage, isolation par typage VM). Les conteneurs Docker sur Linux se situent typiquement dans la fourchette 10² à 10⁴ par machine selon le profil mémoire et le runtime applicatif. Project Loom (JVM) atteint 10⁶ virtual threads par machine mais sans frontière de confiance entre eux. Les unikernels (MirageOS, IncludeOS) atteignent une densité hardware extrême par instance, mais à raison d'un agent par unikernel — ce qui n'est pas commensurable avec une cible multi-tenant. La proposition de ce projet est que l'isolation forte (capabilities, store content-addressed) peut coexister avec une haute densité si les primitives sont conçues pour ce profil dès le départ. Voir le chapitre 03 pour une analyse comparative détaillée.

**Coût connu :** La haute densité hébergée suppose que les agents partagent efficacement les ressources (store mémoire, engine WASM). Cela ajoute un coût de synchronisation pour les opérations multi-agents (store content-addressed partagé, log causal partagé) mais concentre l'overhead d'infrastructure. Elle impose également des contraintes fortes sur le substrat (voir S1, S7 dans `02b-substrate-requirements.md`) — notamment l'exclusion d'un substrat où chaque agent occuperait son propre espace d'adressage MMU au sens classique d'un processus Unix.

#### P1b — Densité active

**Nom court :** Densité active

**Définition formelle :** Pour un workload W1 actif (`idle_fraction = 0.5`, inférence simulée incluse), le système soutient un débit d'actions/seconde au moins R_actif fois supérieur à la baseline Docker, à p99 latence d'action sous un seuil de performance.

**Convention de paramètre :** L'`idle_fraction = 0.5` signifie que chaque agent passe 50% du temps à traiter une action active (introspect → infer → commit_barrier → emit) et 50% du temps idle en attente du message suivant. Cette fraction est une valeur de convention neutre en l'absence de données de production ; elle sera réévaluée quand des profils d'agents LLM réels sont disponibles.

**Métrique principale :** Débit d'actions/seconde (total emit / durée d'exécution), mesurée sur W1 avec N agents simultanément actifs.

**Métrique secondaire :** p99 latence d'action (end-to-end, de la réception du message à la réponse du commit_barrier), décomposée en :
- p99 temps d'attente dans le pool d'inférence (sémaphore d'acquisition de slot)
- p99 temps d'exécution post-acquisition (introspect + infer + commit_barrier + emit)

**Définition opérationnelle de "agent actif":** Agent exécutant le cycle W1 complet : réception message → introspect → infer (simulé) → commit_barrier → emit. Inférence simulée = `SleepyBackend(delay_ms=2500)`.

**Borne de performance :** R_actif ≥ 2× pour valider H-densité-active. Justification : au-delà de k inférences concurrentes, le pool d'inférence sérialise le débit ; le ratio tend vers 1× quel que soit le runtime une fois le pool saturé. La borne 2× est le seuil défendable sur la dimension active sans hypothèse sur le capacity du pool.

**Seuil de latence :** p99 latence d'action ≤ 2 secondes, compatible avec W1 (inférence 2,5 s + overhead < 2s toléré).

**Comparaison baseline :** Docker baseline active (cycle W1 complet sur container Python, pas idle). Le ratio se mesure à capacité d'inférence équivalente : même nombre de slots disponibles pour les deux systèmes.

**Note clé sur la saturation du pool :** La métrique "nombre d'agents" ne suffit pas à prédire le débit au-delà du point de saturation du pool d'inférence. Le débit d'actions/s est la métrique qui ne se laisse pas tromper. Un système avec 1 000 agents mais k=8 slots d'inférence aura le même débit asymptotique qu'un système avec 10 agents : les deux sont limités par k. La densité active mesure donc non pas "combien d'agents", mais "combien d'actions/s avec quel latency p99".

**Coût connu :** La haute densité active dépend de deux facteurs orthogonaux :
1. La latence de commit_barrier (overhead par action) — cette composante est bornée par H-commit-barrier.
2. La disponibilité de slots d'inférence non saturés — cette composante dépend de la politique de supervision asymétrique pour l'allocation de capacité d'inférence. Elle n'est pas une contrainte du système lui-même, mais plutôt une contrainte de planning opérationnel.

**Référence état de l'art :** Aucun système de production (Erlang/OTP, Akka, Project Loom) n'expose simultanément (a) le débit d'actions sur des agents IA long-courriers, (b) une latence p99 bornée dans la formation des actions, et (c) une ressource d'inférence partagée visible au scheduler. Cette triple combinaison est spécifique au profil d'agents IA du projet. Le débit est comparable à un load balancer Web (requests/s) ; la latence est comparable aux garanties de SGBD transactionnels (p99 ≤ 2s sous contention raisonnable).

**Interaction avec H-inférence-coût :** La densité active (P1b) interagit fortement avec l'hypothèse H-inférence-coût. Chaque appel LLM d'un agent consomme une tranche de capacité d'inférence sur le nœud, et les appels d'un même agent s'enchaînent séquentiellement — ils ne peuvent pas se paralléliser entre eux. Sur CPU avec qwen2.5:3b (~7 tok/s), une chaîne de 10 appels représente 130–170 secondes de capacité d'inférence monopolisée par un seul agent (mesuré empiriquement en phase 2 du lab, voir `lab/LESSONS.md` §L9 et `04-hypotheses.md` §H-inférence-coût). Sur GPU (A100/H100), les temps tombent à < 1s/appel — la contrainte sort de la zone problématique, mais le caractère borné de la ressource reste vrai. Le scheduler doit gérer deux dimensions : (a) la densité horizontale — nombre d'agents simultanément actifs ; (b) la densité verticale — profondeur de chaîne d'appels que chaque agent peut atteindre. P1b capture la dimension horizontale sous profil W1 donné ; la dimension verticale est traitée par H-inférence-coût et doit être intégrée dans le design du scheduler dès la phase 3+.

> **Note T6-qualif (2026-05-16) :** P1 a été réécrite pour séparer formellement densité hébergée (P1a) et densité active (P1b) selon les décisions validées du 2026-05-16. Cette décomposition reflète la distinction architecturale du système : un agent peut être hébergé (état dans ContentStore, pages WASM virtuelles) sans pour autant inférer activement (sans slot d'inférence acquis). Les deux métriques doivent être validées indépendamment avec des workloads et des baselines distinctes. Le paramètre `idle_fraction = 0.5` de W1 est une convention provisoire neutre ; il sera réévalué quand des profils d'agents LLM réels sont disponibles.

---

### P2 — Rollback transactionnel

**Nom court :** Rollback

**Définition formelle :** Pour tout instant T dans la vie d'un agent, le système peut restaurer l'état local (tel que défini dans `06-glossary.md`) à l'état qu'il avait après l'action survenue à T, en un temps borné et avec une complexité **O(depth) = O(N)** où N est le nombre d'actions depuis le dernier commit barrier (traversée linéaire de la chaîne de parents content-addressed, `rollback_path`).

> **Amendement ADR-0051 (2026-05-30) :** la formulation antérieure « O(log N) » était une **sur-revendication** (gate SEF-8, L87) : l'implémentation est O(depth) et le reste par construction. La revendication sous-linéaire est **retirée**, pas conservée comme cible — elle ne porte aucune propriété visée, et un design O(log N) (skip-list de snapshots) serait du gold-plating YAGNI (overhead d'écriture par action sur le chemin chaud pour optimiser un chemin froid déjà borné). La promesse réelle de P2 est la **borne murale ci-dessous** (≤ 100 ms / 100 actions), tenue en O(depth) car N est borné par le commit barrier.

**Borne de performance :** La durée d'un rollback sur les 100 dernières actions est ≤ 100ms, mesurée sur workload W2 (qui inclut explicitement la capacité de rollback sur 100 actions).

**Métrique :** Temps de rollback mesuré en ms, de la commande de rollback à la confirmation que l'état local est restauré et cohérent (hash d'état identique à l'état cible). Mesuré via le scénario d'équivalence fonctionnelle SEF-2 : après 1 000 actions, un rollback à l'action n°500 produit un état dont le hash est identique à celui mesuré après l'action n°500.

**Référence état de l'art :** Les systèmes de checkpoint/restore (CRIU sur Linux) permettent de restaurer un processus à un état antérieur, mais avec des coûts O(N) sur la taille de l'état sérialisé et des durées typiques de plusieurs secondes. Les bases de données avec MVCC (PostgreSQL, SQLite WAL) offrent un rollback transactionnel O(log N) sur les tuples modifiés, mais ne gèrent pas l'état actoriel complet ni les messages en transit. NixOS/Unison appliquent le stockage content-addressed à du déploiement statique, pas à de l'état runtime mutant à haute fréquence. Ce projet vise à apporter le niveau de garantie des SGBD transactionnels au niveau du runtime OS d'un agent.

**Coût connu :** Le rollback O(log N) suppose que le store est content-addressed et que les snapshots d'état sont maintenus de manière incrémentale. Cela introduit un overhead continu sur chaque action (écriture dans le store, mise à jour du log causal) et consomme de l'espace disque proportionnellement au volume d'actions dans la transaction en cours. Le rollback ne s'applique qu'à l'**état local** — la portée précise de cette restriction est documentée dans le glossaire (`06-glossary.md`) et dans le non-objectif `N-rollback-ext` (`05-non-goals.md`).

---

### P3 — Traçabilité causale — famille de propriétés

**Périmètre de la famille P3 :** La traçabilité causale est décomposée en trois sous-propriétés (P3a, P3b, P3c) selon la **portée** de la garantie de latence. Cette décomposition évite de prétendre valider une portée large (end-to-end, multi-agent) avec une mesure qui n'en couvre qu'une partie étroite (lookup isolé sur DB statique). Réf : avis externe 2026-05-15 §Q1.

**Décision Q1 (2026-05-16) — portée officielle de la borne 10 ms :** La borne historique « p99 ≤ 10 ms » est désormais **liée exclusivement à P3a** (lookup point isolé sur DB statique). Elle ne couvre **pas** le cycle end-to-end emit→fsync→get (P3b, borne distincte ≤ 20 ms) ni le régime multi-agent concurrent (P3c, bornes ≥ 50 ms). Toute communication externe (README, papier, slides) référençant « la borne 10 ms de P3 » doit qualifier explicitement la portée P3a. La métrique T5 (lookup `get(action_id)` sur log peuplé statique) mesure correctement P3a et **uniquement P3a**. Cette décision est tracée dans `TODO.md §Q1`.

La consultation fenêtrée par `(agent_id, ts_ms)` (range query) est une propriété distincte, documentée séparément en **P3-range** ci-dessous — elle n'est pas une portée de P3 mais un accès orthogonal par index secondaire.

---

### P3a — Traçabilité causale — lookup point isolé

**Nom court :** Traçabilité causale (lookup)

**Périmètre :** `get(action_id)` sur DB statique, en lecture seule, sans write concurrent. C'est la portée la plus étroite — elle mesure la latence structurelle de l'index, pas le coût end-to-end d'une action en production.

**Définition formelle :** Pour toute action exécutée par un agent, le système maintient une entrée causale identifiée par un `action_id` (hash SHA-256 content-addressed de l'entrée sérialisée, 32 bytes) contenant au minimum : l'identifiant de l'agent émetteur, le timestamp, les parents causaux dans le DAG (`parent_ids`), les hashes d'état avant/après, et le payload d'émission éventuel (cf. `LogEntry` dans `poc/causal-log/src/lib.rs`, contrat fixé par ADR-0010). Étant donné un `action_id`, le système retourne l'entrée correspondante (ou son absence) en latence bornée, indépendante de la taille du log.

**Borne de performance :** Le lookup d'une entrée par `action_id` a un p99 ≤ 10 ms, mesuré sur un log de 10⁸ entrées, sur le hardware de qualification défini en `benchmarks/test-protocol.md` §6.1 (NVMe ≥ 1 GB/s, RAM ≥ 16 GB).

**Métrique :** Latence de lookup p99 en ms, sur un log synthétique de 10⁸ entrées peuplé par `CausalLog::populate_synthetic` (échantillonnage uniforme d'`action_id` parmi les 10⁸ entrées, K runs ≥ 3, régime cache froid documenté). Vérifiée via le scénario SEF-5 restreint au lookup point : pour tout `action_id` retourné précédemment par `append`, `get(action_id)` produit l'entrée correspondante en p99 ≤ 10 ms. La complétude sémantique du contenu de l'entrée (présence des parents causaux, des hashes d'état, du payload d'émission attendu) est vérifiée séparément par SEF-3 (cf. P4) et par les tests d'intégrité de `poc/causal-log/`.

**Référence état de l'art :** Le lookup par identifiant indexé est l'opération canonique des index secondaires de SGBD (B+tree, LSM) et des systèmes de tracing à haut volume (Jaeger sur Cassandra, Honeycomb sur ClickHouse). La borne p99 ≤ 10 ms à 10⁸ entrées est dans le régime atteignable d'un LSM tree correctement configuré (RocksDB avec bloom filter, voir ADR-0011 et [Dong et al. 2017, FAST]). Le différenciateur n'est pas la latence elle-même mais le couplage natif au modèle d'exécution : le `action_id` est content-addressed et produit par le runtime à chaque commit barrier, sans surcouche applicative ni transport réseau.

**Coût connu :** Le lookup O(1) suppose un index maintenu en temps réel (la clé RocksDB *est* l'`action_id`). L'overhead d'écriture par action est dominé par l'append RocksDB + bincode + SHA-256 (~10 µs mesuré en Phase 5, cf. H-commit-barrier). L'espace disque est linéaire avec le volume d'actions : pour 10⁸ entrées (~100–200 bytes chacune, cf. ADR-0011), ~10–15 GB sans compression. La tension avec P1 (densité) est documentée en section 3 et amortie par le store mutualisé entre acteurs.

**Statut de qualification :** **P3a validé sous conditions** (2026-05-18). K=10 runs conformants sur 2 classes hardware distinctes. Classe 1 : AWS i3en.xlarge, K=7 conformants, NVMe 741–769 MB/s QD=1, p99 pire cas 482 µs. Classe 2 : AMD Ryzen 5 PRO 4650U + WD SN530 NVMe PCIe, K=3 conformants, NVMe 1 290–1 321 MB/s QD=1, p99 pire cas 4 855 µs. N=10⁸, régime cache-mixte (drop_caches appliqué ; RAM/dataset ≈ 0,93× classe 2 — plus contraint que classe 1). Marge pire cas toutes classes : ×2 sous la cible de 10 ms. Voir `results/T5/SYNTHESE.md`.

**P3a validé** (ADR-0026, 2026-05-18). Conditions (a) et (b) du protocole §5 toutes deux satisfaites : (a) 2 classes hardware distinctes, K≥3 runs chacune ; (b) régime cache-mixte contraint entériné comme régime représentatif par ADR-0026 (drop_caches + RAM/dataset ≤ 2×).

---

### P3b — Traçabilité causale — end-to-end emit→fsync→get

**Nom court :** Traçabilité causale (end-to-end)

**Statut :** **À mesurer** — aucune mesure disponible. Déclencheur : T5-bis.

**Périmètre :** Le cycle complet qu'un agent en production paie avant de pouvoir relire une action qu'il vient d'émettre : `emit()` → WAL fsync → `get(action_id)`. Cette portée inclut le coût d'écriture durable (fsync du WAL RocksDB), absent de P3a.

**Définition formelle :** Pour toute action émise par un agent via `CausalLog::append` avec durabilité garantie (`WriteBatch` + WAL fsync), le système retourne l'entrée correspondante via `get(action_id)` en latence bornée depuis le moment de l'appel `append`.

**Borne de performance (cible) :** p99 ≤ 20 ms, sur le hardware de qualification défini en `benchmarks/test-protocol.md` §6.1. La borne accommode le coût WAL fsync : sur NVMe rapide 0,5–3 ms ; sur SSD SATA 5–20 ms. La borne 20 ms laisse une marge raisonnable et reste en-deçà du seuil de perception « instantané » (~100 ms).

**Métrique :** Latence end-to-end p99 en ms, mesurée de l'appel `append` à la réponse `get(action_id)`, sur un log de 10⁸ entrées avec writes et reads entrelacés. K runs ≥ 3, régime cache froid documenté (drop_caches avant chaque run). Benchmark dédié T5-bis (`poc/causal-log/benches/causal_end_to_end.rs`, harness `benchmarks/t5-bis-bundle/`).

**Méthode de mesure (T5-bis, 2026-05-18) :**

- L'écriture est faite via `CausalLog::append_durable()` (et non `append()`). `append_durable` active `WriteOptions::set_sync(true)` : la fonction ne retourne qu'après que le WAL RocksDB a été fsynced. C'est la sémantique requise par P3b (« WAL fsync »).
- Chaque cycle de mesure crée une entrée distincte (agent_id unique par cycle, ts_ms incrémental), puis appelle `get(action_id)` sur l'action qu'on vient d'écrire. Le timer chronomètre le cycle complet. Cette sémantique isole le coût d'écriture durable (fsync) du coût lookup (qui est en memtable côté lecture), évitant de mélanger P3a et P3b dans la même mesure.
- Population initiale 10⁸ entrées (régime LSM stable, plusieurs niveaux). 10 000 cycles append+get chronométrés après la population.
- Modèle d'accès Q2 = `uniform` (Modèle A) — les agent_id générés par T5-bis sont uniformément distribués.

**Critère de déclenchement de T5-bis :** après application des fixes harness T5 (drop_caches + fio QD=32) et validation de P3a « validé » (K ≥ 5, 2 instances). **Statut au 2026-05-18 :** P3a validé (ADR-0026, K=10 conformants sur 2 classes hardware). Critère de déclenchement satisfait — T5-bis prêt à être exécuté.

**Coût additionnel vs P3a :** le fsync WAL est le facteur dominant. Sur i3en.xlarge NVMe, fsync typique 0,5–2 ms. La borne 20 ms absorbe ce coût avec une marge de sécurité.

---

### P3c — Traçabilité causale — multi-agent concurrent

**Nom court :** Traçabilité causale (multi-agent)

**Statut :** **Réservé** — aucune mesure, aucune implémentation. Déclencheur : voir ci-dessous.

**Périmètre :** p99 du lookup (P3a) ou du cycle end-to-end (P3b) sous N agents concurrents écrivant et lisant simultanément. C'est la portée de production réelle : le store est soumis à une contention RocksDB réelle, le block cache est partagé entre N agents, les WAL fsyncs s'accumulent.

**Bornes de performance (cibles) :**
- p99 ≤ 50 ms à N=8 agents concurrents
- p99 ≤ 100 ms à N=32 agents concurrents

**Critère de déclenchement :** P3b passée (T5-bis) ET modèle de working set multi-agent formalisé dans `benchmarks/reference-workload.md §W1-access` (Modèles A & B déclarés, mesure sous chaque pattern implémentée). Ce benchmark multi-tenant est distinct de T5 et requiert un workload de référence multi-agent qui n'existe pas encore.

**Note :** P3c ne figure pas dans l'ordre d'arbitrage §3.2 tant qu'elle reste sans borne mesurée.

---

### P3-range — Consultation fenêtrée (provisoire, non-bornée)

**Nom court :** Fenêtre causale

**Statut :** **Propriété provisoire — pas de borne de latence chiffrée à ce stade.** Présence dans la spec à titre de placeholder pour la consultation par range query (intervalle temporel, séquence d'actions d'un agent).

**Note (2026-05-14) :** ADR-0009 a adopté le **modèle B** (enregistrement minimal + matérialisation fenêtrée) comme modèle de représentation du log causal, révisant ADR-0006. Dans ce cadre, P3-range se reformule en garantie d'**intégrité de reconstruction** : la chaîne causale complète d'un agent est reconstituable à la demande, bornée par la profondeur de la chaîne demandée — et non plus en borne de latence de range query. Cette reformulation est différée à l'implémentation de la CF `agent_ts` et à la décision sur le modèle de matérialisation fenêtrée. Tant que ces étapes ne sont pas franchies, P3-range reste un placeholder non-falsifiable.

**Couplage modèle de représentation (historique) :** La formulation initiale couplait P3-range (alors nommée P3b) au **modèle A** (supervision continue, ADR-0006) : entrées structurées requêtables par `agent_id` + `ts_ms` sans matérialisation intermédiaire. Sous modèle B désormais adopté, cette formulation est à remplacer par la garantie d'intégrité ci-dessus. Le passage au modèle B ne touche pas à P3a (lookup point par `action_id`), dont la borne O(1) est portée par l'index, indépendamment du format des entrées.

**Définition opérationnelle (provisoire) :** Étant donné un `agent_id` et un intervalle `[t_début, t_fin]` (ou alternativement un `seq_start` et un `seq_end`), le système retourne la liste de toutes les entrées causales de cet agent dont le timestamp tombe dans l'intervalle, ordonnées par séquence causale.

**Métrique :** **À définir.** Aucune borne chiffrée n'est posée tant que :
1. Un index secondaire `(agent_id, ts_ms) → action_id` n'est pas implémenté. L'implémentation actuelle (`CausalLog::entries_by_agent`, `poc/causal-log/src/lib.rs` lignes 197–211) est un **scan linéaire O(N)** sur l'ensemble du log, explicitement marqué "tests et diagnostics uniquement, pas le chemin chaud". Elle ne satisfait aucune propriété de production.
2. Un workload de référence pour la consultation fenêtrée (fréquence, taille moyenne de fenêtre, profil de concurrence avec les writes) n'est pas formalisé dans `benchmarks/reference-workload.md`.

**Structure d'index retenue (décision d'implémentation, non implémentée à 2026-05-14) :** column family RocksDB dédiée `agent_ts` à côté de la CF `default` du `CausalLog`. Clé de la CF `agent_ts` = concaténation `agent_id (16 bytes) || ts_ms (8 bytes, big-endian) || action_id (32 bytes)` = 56 bytes ; valeur = bytes vides. L'encodage big-endian de `ts_ms` aligne l'ordre lexicographique RocksDB sur l'ordre temporel, permettant un scan de préfixe `[agent_id]` de retourner toutes les entrées de l'agent dans l'ordre causal temporel ; un scan de préfixe `[agent_id || ts_start]` à `[agent_id || ts_end]` réalise une range query bornée par fenêtre. L'inclusion d'`action_id` dans la clé désambiguïse les collisions à `ts_ms` égal (deux actions d'un agent dans la même milliseconde — possible sous L9 sur GPU). L'écriture dans la CF `agent_ts` est atomique avec l'écriture dans `default` via un `WriteBatch` couvrant les deux CF — RocksDB garantit l'atomicité cross-CF d'un batch. La range query retourne les `action_id` ordonnés ; le lookup du contenu se fait par `get(action_id)` séparé dans la CF `default` (réutilise P3).

L'alternative d'un prefix-extractor sur la CF `default` est **techniquement non-applicable** : la clé de `default` est `SHA-256(bincode(LogEntry))`, donc statistiquement non-corrélée à `agent_id` et `ts_ms` (effet d'avalanche du hash). Aucun préfixe binaire de la clé `default` n'encode `agent_id`. Un prefix-extractor sur cette CF n'accélère pas une range query par `(agent_id, ts_ms)`. Changer la clé primaire pour qu'elle préfixe par `agent_id` détruirait P3 (clé content-addressed).

La structure et les options RocksDB de la CF `agent_ts` sont documentées dans `decisions/0011-options-rocksdb-layer0.md` (section « Index secondaire `agent_ts` — structure prévue »). L'implémentation et la mesure relèvent du jalon de promotion P3-range (cf. « Action requise avant promotion »).

**Référence état de l'art :** Les systèmes de tracing à grande échelle séparent systématiquement l'index `span_id → span` (lookup point) et le backend de range query (Cassandra secondary index, Elasticsearch, ClickHouse columnar). La latence de range query est typiquement deux ordres de grandeur au-dessus du lookup point et dépend fortement de la taille de fenêtre. NixOS/Unison content-addressed ne traitent pas cette dimension (logs statiques, pas runtime). Aucun système connu ne combine lookup point sub-10ms et range query bornée *à la même garantie* sur le même substrat — c'est précisément pourquoi P3-range reste non-bornée tant qu'on n'a pas tranché entre (a) index secondaire dédié sur la CF du log, (b) backend séparé pour la consultation humaine, (c) matérialisation à la demande (modèle B).

**Coût connu (anticipé) :** Un index secondaire `(agent_id, ts_ms)` introduit un coût d'écriture supplémentaire à chaque `append` (deuxième entrée RocksDB, ~5 µs additionnel estimé) et un espace disque linéaire en plus du log principal. Cette dépense est exclusivement justifiée par la fréquence anticipée de consultation fenêtrée — qui n'est pas mesurée. C'est précisément ce que l'amendement ADR-0006 pointe : la pertinence de cet overhead dépend du modèle de supervision retenu.

**Action requise avant promotion en propriété bornée :**
1. Formaliser un workload de référence pour la range query (fréquence, taille de fenêtre, ratio reads/writes).
2. Décider entre index secondaire dédié (modèle A) et matérialisation à la demande (modèle B). Cette décision relève d'un futur ADR successeur d'ADR-0006.
3. Implémenter et mesurer. Tant que ces étapes ne sont pas franchies, P3-range reste un placeholder non-falsifiable.

---

### P4 — Isolation par capabilities

**Nom court :** Isolation

**Définition formelle :** Tout accès à une ressource par un acteur du système (lecture ou écriture d'état d'autres acteurs, émission de message vers une destination, accès au store, accès à un effet externalisable) requiert que l'acteur détienne une capability explicite l'autorisant. Aucun accès ambient n'est possible. La délégation de capabilities entre acteurs respecte la propriété d'**atténuation**, définie selon deux dimensions :

- **Atténuation de permission** : une capability dérivée accorde au plus les mêmes droits d'opération sur la même ressource que la capability source. Exemple : une capability lecture-écriture sur un acteur peut être atténuée en lecture seule sur le même acteur.
- **Atténuation de portée** : une capability dérivée couvre au plus le même ensemble de ressources que la capability source. Exemple : une capability sur l'ensemble du store d'un agent peut être atténuée en capability sur un sous-arbre `store/agent-A/tâche-X/`. Une capability sur un acteur A ne peut pas être utilisée pour dériver une capability sur un acteur B distinct.

Ces deux dimensions sont indépendantes et cumulables : une dérivée peut être à la fois plus restreinte en permission et en portée. En aucun cas une dérivée ne peut excéder la capability source sur l'une ou l'autre dimension. La révocation d'une capability invalide récursivement toutes ses dérivées.

**Métrique :** Vérifiée via le scénario SEF-3. Le système doit satisfaire conjointement les trois conditions suivantes :

- **Soundness des accès autorisés** : 100% des tentatives d'accès couvertes par une capability détenue réussissent.
- **Soundness des refus** : 100% des tentatives d'accès non couvertes par une capability détenue échouent, sans contournement possible.
- **Complétude de l'audit** : 100% des tentatives d'accès non autorisées sont enregistrées dans le log causal avec l'identifiant de l'acteur fautif, la capability manquante, et le timestamp logique — **jusqu'au rate-limit anti-DoS du log causal** (100 refus/agent/1 s, `CapabilityDenied 0x14`). Au-delà du rate-limit, l'attribution est préservée pour tout ensemble **borné** de resources distinctes nouvelles (agrégation par resource, correctif ADR-0051 §D2) ; le compteur agrégé évite le DoS du log sans masquer une resource refusée non encore vue dans la fenêtre.

> **Amendement ADR-0051 (2026-05-30) :** la formulation antérieure « 100% … aucun accès non autorisé ne passe sans enregistrement » était **littéralement fausse sous flood** (SEF-9, L88) : un adversaire inondant >100 refus bénins/s pouvait masquer un refus malveillant (agrégation scalaire `cap_id+count` sans resource, puis silence). C'est un défaut d'**observabilité d'audit** (3ᵉ critère conjoint de P4), **pas** un défaut d'isolation (la cap reste correctement refusée). Le correctif #6 (agrégation par resource bornée) rehausse l'implémentation au niveau de l'énoncé amendé.

La propriété de propagation correcte des révocations est vérifiée par un scénario complémentaire défini dans le plan d'invalidation de l'hypothèse `H-revoke` (voir `04-hypotheses.md`, section 5.1) : après révocation d'une capability donnée à un acteur ayant lui-même délégué N dérivées, aucune des N+1 capabilities (originale et dérivées) ne permet plus d'accès, dans un délai borné par le mécanisme de TTL pour les capabilities exportées.

**Référence état de l'art :** Le modèle capability est formellement défini depuis [Dennis & Van Horn 1966]. seL4 [Klein 2009] a démontré la vérification formelle d'un microkernel capability-based. CHERI [Watson 2015] l'implémente au niveau hardware. EROS [Shapiro 1999] et KeyKOS l'ont appliqué au niveau OS. Pony [Clebsch 2015] applique des *reference capabilities* au niveau langage pour garantir l'absence de data races (problème conceptuellement distinct mais analogue dans le mécanisme d'atténuation). Ce projet applique le modèle au niveau du runtime d'un système d'agents IA, avec une dimension nouvelle : la délégation et la révocation dynamiques entre sous-agents spawnés à l'exécution. Cette dynamique est précisément ce qui distingue P4 des modèles statiques cités, et c'est aussi ce qui en fait l'objet de l'hypothèse bloquante `H-revoke`.

**Coût connu :** Le contrôle d'accès non-ambient impose une vérification de capability à chaque opération inter-acteur. Cet overhead est inhérent au modèle ; sa magnitude dépend du substrat (vérification au niveau langage à la compilation, au niveau IPC kernel, ou au niveau hardware). L'arbre de dérivation des capabilities a un coût en mémoire O(N) où N est le nombre de capabilities vivantes dans le système. La révocation propagée est O(profondeur de l'arbre) en temps. Ces coûts sont l'objet de l'hypothèse `H-revoke` et de son plan d'invalidation.

---

### P5 — Déterminisme de transition d'état

**Nom court :** Déterminisme transition

**Définition formelle :** Pour tout agent dont l'exécution ne dépend que des messages reçus et des primitives explicites du système, deux instances de l'agent initialisées avec un état identique et soumises à la même séquence de messages dans le même ordre produisent la même séquence de messages émis et le même état final (vérifié par hash).

Le périmètre de la propriété est restreint comme suit :

- Le déterminisme garanti porte sur la **transition d'état**, pas sur l'**exécution** (timings, ordonnancement, latences). Cette restriction est documentée dans `05-non-goals.md` section 3.3.
- Les sources de non-déterminisme externes (horloge wall-clock, génération d'aléas, résultats d'inférence stochastique, données provenant de l'extérieur du nœud) sont accessibles aux agents uniquement via des primitives explicites du système, qui peuvent être substituées dans un contexte de replay (voir exigence S6 dans `02b-substrate-requirements.md`).
- **P5 est une garantie conditionnelle** : elle tient si et seulement si les exigences S1 et S6 du substrat sont satisfaites conjointement. S1 garantit qu'un acteur ne peut pas accéder à de la mémoire partagée non médiée (ce qui serait une source de non-déterminisme non observable). S6 garantit que toutes les sources de non-déterminisme passent par des primitives substituables. Si le substrat retenu ne satisfait pas S1 ou S6, P5 devient non-vérifiable — et SEF-6 est classé hors-périmètre pour ce substrat dans le tableau de discrimination de `02b-substrate-requirements.md`.
- Les agents qui contournent les primitives S6 (par exemple via un trou dans S1) sortent du périmètre garanti par P5. Le système ne peut pas détecter ce contournement si S1 n'est pas satisfait — c'est la raison pour laquelle S1 est une condition nécessaire à P5.

**Métrique :** Vérifiée via le scénario SEF-6. Deux instances du système, initialisées avec un état d'agent identique (hash vérifié) et alimentées avec une séquence enregistrée de 1 000 messages dans le même ordre, doivent produire des séquences de messages émis identiques et des hash d'état finaux identiques.

**Référence état de l'art :** Le déterminisme de transition est garanti par construction dans les modèles fonctionnels purs (Haskell sans `IO`) et dans les modèles d'acteur où la communication est l'unique source d'observabilité (modèle d'acteur original [Hewitt 1973], variantes formellement vérifiées). Sur Linux, le déterminisme d'exécution complet peut être obtenu via record-and-replay externe (`rr` de Mozilla [O'Callahan 2017]), mais avec un overhead substantiel et sans intégration au modèle d'observabilité du système. Foundation DB et Antithesis utilisent la simulation déterministe pour tester des systèmes distribués ; leur approche est une référence d'architecture, pas une garantie systémique. Ce projet intègre le déterminisme de transition comme propriété native du modèle, permettant le replay d'une exécution d'agent sans outil externe.

**Coût connu :** Le déterminisme de transition impose que toute source de non-déterminisme passe par une primitive du substrat. Cela contraint la conception du runtime : pas d'accès direct à `clock_gettime()`, pas de `/dev/urandom` non médié, pas d'appel à un service externe sans interposition. Cet overhead est principalement structurel (interface plus stricte) plutôt que coûteux en performance.

---

### P6 — Atomicité de transaction face aux pannes

**Nom court :** Atomicité crash

**Définition formelle :** Si un agent est en cours de traitement d'une transaction (séquence d'actions entre deux commit barriers, voir `06-glossary.md`) et qu'un crash brutal survient (kill du processus, panique du runtime), l'état local du système après recovery est exactement l'un des deux suivants :

- l'état immédiatement antérieur au début de la transaction interrompue, **ou**
- l'état immédiatement postérieur au dernier commit barrier atteint avant le crash.

Aucun état intermédiaire n'est observable après recovery. La propriété est vérifiable par hash de l'état local. **L'« état local »** au sens de cette propriété est défini par le hash du **ContentStore** (Merkle DAG des `SnapshotHeader` de l'agent), qui est l'**état autoritaire** ; le **log causal** est l'**observable de complétude transactionnelle** (ce qui est visiblement committé), pas l'état autoritaire (cf. ADR-0027 §D3). L'oracle SEF-4 observe via le log (dernier `LogEntry`) ; la cohérence des deux objets repose sur l'**asymétrie** suivante.

> **Asymétrie orphelin / référence pendante (amendement ADR-0051 §D1, 2026-05-30).** Sous le régime no-force, deux états cross-store sont possibles après recovery :
> - **Orphelin toléré** : un snapshot ContentStore **non référencé** par le log (store en avance). C'est du garbage GC-able, **admis** par le no-force et invisible à l'observable causal.
> - **Référence pendante** : un `LogEntry` référençant un snapshot **absent** du ContentStore (log en avance). C'est un **état déchiré**, **non admis** par le no-force — distinct de la simple perte de queue.
>
> **Trou de P6 non couvert (SEF-10, L89) :** ContentStore et CausalLog étant **deux instances RocksDB séparées** sans fsync ni atomicité cross-DB, il existe sous cache-loss une fenêtre de **référence pendante cross-store**. Ce trou est distinct du trou power-loss déjà documenté ci-dessous. Atténuation actuelle : fail-safe au restore (#7a, ADR-0051 §D3) — détecte la référence pendante, ne ferme pas la fenêtre. Fermeture (commit cross-store atomique, #7b) différée au chantier GC / re-séparation CAS/index (ADR-0049 §D3a).

**Modèle de menace couvert (Phase 6) :** Crash brutal **du processus utilisateur** — `SIGKILL`, panique Rust, OOM-killer, `std::process::exit`. Sous ces conditions, le page cache du noyau Linux survit ; RocksDB rejoue son WAL au redémarrage et restaure intégralement les écritures faites par le processus défunt. P6 tient sans fsync forcé sur le chemin chaud (ADR-0027 D1).

**Modèle de menace non couvert (Phase 6) :** **Power-loss / kernel panic / hardware fault.** Sous ces conditions, le page cache OS est perdu ; les écritures depuis le dernier fsync sont perdues. La couverture exigerait `WriteOptions::set_sync(true)` sur le ContentStore (`put_block` + `put_snapshot`) et sur le `append` du log qui le suit. Le coût (P3b ≤ 20 ms ciblé, non encore mesuré sur hardware qualifié — T5-bis pending) tendrait à dominer P1b. Reporté Phase 7+ après mesure T5-bis (ADR-0027 §D4).

**Métrique :** Vérifiée via le scénario SEF-4. Un agent exécute une séquence d'actions ; à un instant arbitraire pendant une transaction, le système est tué par `SIGKILL` (ou équivalent au niveau substrat — `std::process::exit` accepté). Après recovery, le hash de l'état local (ContentStore) est calculé et comparé aux deux hashes de référence (état avant transaction, état après dernier commit). L'égalité avec l'un des deux est requise. Le scénario SEF-4 actuel **ne teste pas** le régime power-loss (cf. modèle de menace ci-dessus).

**Référence état de l'art :** L'atomicité face aux pannes est un acquis classique des systèmes transactionnels (ACID, [Gray & Reuter 1992]) appliqué aux bases de données. Sur Linux, le journaling de système de fichiers (ext4, btrfs) garantit l'atomicité au niveau fichier ; les transactions de SGBD garantissent l'atomicité au niveau tuple ; aucun mécanisme système ne garantit l'atomicité d'un état actoriel complet (mémoire de l'acteur + boîte aux lettres + messages en transit interne). Erlang/OTP traite le problème par convention de design ("let it crash" avec supervision et redémarrage à un état stable connu) plutôt que par garantie d'atomicité transactionnelle. La distinction « durable » (force-at-commit) vs « no-force » (group commit + recovery) — [Gray & Reuter 1992, ch. 9–10] — éclaire la portée de notre garantie : le système est « no-force » sous menace SIGKILL et bénéficie du WAL OS-buffered.

**Coût connu :** L'atomicité face aux pannes suppose que le store soit en mesure de distinguer un état committé d'un état en cours de transaction, et que la procédure de recovery puisse identifier sans ambiguïté l'un ou l'autre. Cela impose une discipline d'écriture stricte (pas d'écriture intermédiaire dans le store sans encadrement transactionnel). C'est un corollaire direct de l'exigence S2 dans `02b-substrate-requirements.md` (capture cohérente d'état d'agent à coût borné) et des propriétés du store content-addressed sous-jacent à P2. Le coût additionnel d'une éventuelle extension power-loss (Phase 7+) est borné par P3b (≤ 20 ms p99 pour `emit → fsync → get`) ; sa promotion impacterait P1b (densité active) — arbitrage chiffré possible après T5-bis.

---

## 3. Tensions et tradeoffs entre propriétés

### 3.1 Tableau des conflits potentiels

Les propriétés du système ne sont pas indépendantes. Certaines partagent une infrastructure commune (synergie potentielle ou contention) ; d'autres consomment des ressources qui se font directement concurrence. Le tableau ci-dessous recense les tensions identifiées.

| Propriété A | Propriété B | Nature de la tension |
|-------------|-------------|----------------------|
| P1a Densité hébergée | Aucune tension directe | P1a mesure le coût d'hébergement d'agents parqués, orthogonal aux opérations : aucune action en cours, pas d'overhead par action. Infrastructure partagée avec d'autres propriétés mais pas de contention. |
| P1b Densité active | P2 Rollback | Le maintien des snapshots incrémentaux pour P2 consomme du débit d'écriture par agent, ce qui réduit le débit d'actions/s et donc la densité active atteignable. |
| P1b Densité active | P3a Traçabilité (lookup) | L'indexation synchrone du log causal pour P3a introduit un overhead par action (~10 µs), ce qui réduit le débit d'actions/s. La tension se mesure en p99 latence d'action sous charge. |
| P1b Densité active | P3b Traçabilité (end-to-end) | Le fsync WAL bloquant de P3b ajoute 0,5–3 ms par `append` sous NVMe, ce qui contraint directement le débit d'actions par agent et la densité active atteignable. |
| P1b Densité active | P3-range Fenêtre causale | Un index secondaire `(agent_id, ts_ms)` (si retenu pour borner P3-range) doublerait l'overhead d'écriture par action (~20 µs), réduisant le débit. Cette tension n'est activée que si P3-range est promue en propriété bornée — actuellement non. |
| P1b Densité active | P4 Isolation | La vérification de capability à chaque opération inter-acteur ajoute un coût constant par action. Sa magnitude dépend du substrat retenu. |
| P2 Rollback | P3a Traçabilité (lookup) | Les deux propriétés s'appuient sur un store content-addressed et un log structuré — synergie potentielle (infrastructure partagée) ou contention (concurrence sur le même stockage). |
| P4 Isolation | P5 Déterminisme | Synergie : une capability tracée et un log causal complet aident à la reproductibilité du replay (P5). Aucune tension identifiée. |
| P5 Déterminisme | P1b Densité active | Le passage obligé par des primitives substituables peut forcer un point de synchronisation dans le runtime, contraignant le parallélisme inter-agent et le débit d'actions/s. P1a (hébergée) n'est pas affectée puisqu'elle mesure des agents inactifs. |
| P6 Atomicité | P2 Rollback | Synergie forte : P6 est en grande partie un corollaire de P2 (un crash est traité comme un rollback implicite à la dernière transaction committée). L'infrastructure est commune. |

### 3.2 Ordre de priorité arbitré

Si une décision de conception force un arbitrage entre deux propriétés, l'ordre de priorité retenu est :

> **P4 ≻ P2 ≻ P3a ≻ P6 ≻ P5 ≻ P1**

**Justification :**

- **P4 (Isolation) en premier** parce que sans elle, le profil B n'est pas adressable. Un système qui héberge des agents stochastiques sans contrôle d'accès non-ambient est dangereux, indépendamment de ses autres propriétés.
- **P2 (Rollback) ensuite** parce que c'est le différenciateur fonctionnel principal vs. la baseline. Sans rollback transactionnel, le système n'apporte pas de capacité nouvelle — il est juste plus dense.
- **P3a (Traçabilité par lookup point)** est le second différenciateur. Sa borne de performance peut être assouplie (p99 ≤ 100ms au lieu de 10ms) avant de remettre en cause le projet ; sa correction et sa complétude ne le peuvent pas. **P3b (end-to-end)** et **P3c (multi-agent)** ne figurent pas dans l'ordre d'arbitrage chiffré tant qu'elles restent sans borne mesurée ; si elles sont promues, leur rang sera situé entre P3a et P6. **P3-range (consultation fenêtrée)** suit le même principe.
- **P6 (Atomicité crash)** est en grande partie corollaire de P2 ; son ordre reflète le fait qu'elle est rarement en tension directe avec autre chose.
- **P5 (Déterminisme transition)** est précieux pour le débogage et la reproductibilité, mais peut être dégradé en mode "best effort" sans détruire le système. Les SEF qui en dépendent (SEF-6) deviennent alors non-applicables aux agents concernés, ce qui est un coût documenté mais non bloquant.
- **P1a et P1b (Densité) en dernier** parce qu'elles sont des cibles quantitatives. Atteindre 4× au lieu de 5× la densité Docker (P1a) ou 1,5× au lieu de 2× le débit actif (P1b) n'invalide pas la thèse, à condition que les autres propriétés soient solides — la quantification est une promesse forte mais le projet conserve sa valeur si elle est partiellement tenue. P1a et P1b ne sont pas en tension mutuelle : une haute densité hébergée n'interfère pas avec le débit actif d'un sous-ensemble d'agents en traitement.

Cet ordre n'est pas un classement par importance dans l'absolu — c'est un ordre de **priorité d'arbitrage**, c'est-à-dire la séquence dans laquelle on accepte de céder sur une propriété si une décision de conception ne peut pas toutes les satisfaire simultanément. Il fait l'objet de `decisions/0001-priorite-proprietes.md`.

### 3.3 Synergies revendiquées

Trois synergies entre propriétés justifient une infrastructure commune dans la conception du système :

1. **P2 et P6 partagent le store content-addressed et le log structuré.** L'atomicité face aux pannes est implémentée comme un cas particulier du mécanisme de rollback : le crash recovery est un rollback à la dernière transaction committée.
2. **P3a et P4 partagent le log causal.** L'enregistrement des accès couverts par capabilities (P4) et l'enregistrement des éléments causaux d'une action (P3a) sont la même opération, vue sous deux angles. La borne de latence de P3a (lookup par `action_id`) ne dépend pas de cette synergie — elle est portée par l'index. La synergie joue au niveau du *contenu* enregistré, pas de la latence de requête.
3. **P4 et P5 partagent les primitives d'interposition du substrat.** La capability requise pour accéder à une horloge ou à une source d'aléas est la primitive même qui permet la substitution en mode replay.

Ces synergies sont la raison pour laquelle l'addition de P4, P5, P6 à P1/P2/P3 ne triple pas le coût d'implémentation : une grande partie de l'infrastructure est mutualisable.

---

## 4. Ce qu'on a délibérément exclu et pourquoi

### 4.1 Déterminisme d'exécution complet

**Énoncé exclu :** Garantir que deux instances du système exposées aux mêmes inputs produisent les mêmes timings, les mêmes décisions de scheduling, et les mêmes ordonnancements de tâches.

**Motivation du rejet :** Le déterminisme d'exécution complet suppose des contraintes de scheduling déterministe quasi-impossibles à satisfaire sans hyperviseur dédié ou modèle d'exécution monothread strict. Il est partiellement obtenu par `rr` (Mozilla) au prix d'un overhead substantiel et sans intégration au modèle d'observabilité du système. Le déterminisme de **transition d'état** (P5) capture l'essentiel de la valeur (reproductibilité d'un bug, replay d'une exécution) à un coût bien moindre. Voir aussi `05-non-goals.md` section 3.3.

### 4.2 Compensation des effets externes

**Énoncé exclu :** Annuler les effets ayant quitté le nœud (messages réseau, appels API tiers) lors d'un rollback.

**Motivation du rejet :** La compensation transactionnelle d'effets distribués (modèle "saga") est un problème non résolu en général. L'inclure dans l'OS reviendrait à donner une illusion de résolution dangereuse. Reste une responsabilité applicative. Voir le non-objectif `N-rollback-ext` dans `05-non-goals.md`.

### 4.3 Garantie de progression (liveness) absolue

**Énoncé exclu :** Garantir que tout agent finit par traiter tout message présent dans sa boîte aux lettres, indépendamment de la charge système, du comportement des autres agents, et des décisions du superviseur asymétrique.

**Motivation du rejet :** Le superviseur asymétrique peut suspendre un agent ou révoquer ses capabilities, ce qui interrompt légitimement sa progression. Garantir une liveness inconditionnelle entrerait en conflit direct avec le modèle de supervision. La propriété adressée à la place — non encore formalisée dans cette phase — est une *liveness conditionnelle* : un agent non suspendu et disposant des capabilities nécessaires finit par traiter tout message non bloqué par une attente d'effet externe.

### 4.4 Latence absolue temps-réel

**Énoncé exclu :** Garantir des bornes de latence dures (hard real-time) pour le traitement d'une action.

**Motivation du rejet :** Les bornes données dans P2 (≤ 100ms pour rollback) et P3 (p99 ≤ 10ms pour lookup) sont des bornes statistiques sous workload de référence, pas des garanties hard real-time. Le profil B (agents long-running) ne nécessite pas de garanties hard real-time, et l'introduction de telles garanties contraindrait massivement l'architecture (scheduler temps-réel, inversion de priorité contrôlée, allocation mémoire bornée). C'est un domaine adjacent (RTOS) avec ses propres systèmes spécialisés.

---

## 5. Renvoi : exigences sur le substrat

Les propriétés P1 à P6 imposent à la couche d'exécution sous-jacente (substrat) un ensemble d'exigences précises. Ces exigences sont documentées dans `02b-substrate-requirements.md`. Elles servent de grille d'évaluation des substrats candidats analysés au chapitre 03 et déterminent les contraintes architecturales que toute implémentation devra satisfaire — indépendamment du choix de l'artefact cible (microkernel, runtime managé, OS langage, etc.) qui restera ouvert jusqu'à un ADR dédié.