# 06 — Glossaire

## 1. Principes d'entrée au glossaire

### 1.1 Un terme entre ici quand sa définition usuelle est insuffisante ou ambiguë dans ce projet

Ce glossaire ne redéfinit pas des termes stables de l'informatique système. Il documente les termes dont la définition usuelle est soit trop vague, soit contradictoire avec l'usage qu'on en fait dans ce projet. Chaque entrée indique pourquoi le terme est potentiellement ambigu et quelle définition on retient.

### 1.2 Format d'entrée

Chaque entrée suit le schéma : **définition opérationnelle** (ce que le terme signifie dans ce projet), **précisions** (ce qui est inclus, ce qui est exclu, les nuances), **termes liés**, **références** si le terme a une origine formelle connue.

---

## 2. Termes fondamentaux du projet

---

### Action

**Définition opérationnelle (niveau système) :** Réception et traitement complet d'un message par un acteur, ou émission d'un message par un acteur, telle qu'elle est observée par le runtime du système. C'est l'unité atomique d'observabilité, de causalité et de transaction dans ce projet.

**Précisions :**

- Une action est soit une réception (un message arrive dans la boîte aux lettres d'un acteur, qui le traite jusqu'à complétion), soit une émission (un acteur envoie un message vers un autre acteur ou l'extérieur).
- La granularité typique est de 10³ à 10⁵ actions par agent par heure, ce qui correspond à des agents qui envoient et reçoivent fréquemment des messages — pas des agents qui exécutent de longues procédures locales silencieuses.
- Les métriques système (densité, latence, traçabilité) sont mesurées au niveau action. Ce n'est pas la plus petite instruction machine, ni la requête HTTP, ni l'appel de fonction — c'est l'échange de message inter-acteur observé par le runtime.
- Une action sans message (computation locale pure, sans émission ni réception) n'est pas une action au sens de ce projet. Ce choix est délibéré : on ne cherche pas à observer chaque instruction, mais chaque échange inter-acteur.

**Termes liés :** Intention (niveau applicatif, agrège des actions), Transaction (séquence d'actions entre deux commit barriers), Agent (l'entité qui exécute des actions).

---

### Intention

**Définition opérationnelle (niveau applicatif) :** Groupement explicite d'une séquence d'actions par l'agent lui-même, déclaré via les primitives `intent_begin(id)` et `intent_end(id, status)`.

**Précisions :**

- Le système enregistre uniquement les intentions que l'agent déclare explicitement via `intent_begin` et `intent_end`. Il n'en crée jamais implicitement.
- L'intention est un concept applicatif, pas système. Le runtime ne connaît que des actions. L'intention est une annotation que l'agent pose sur un groupe d'actions pour lui donner un sens métier.
- Les métriques système (densité, latence, traçabilité) sont mesurées au niveau action. Les métriques applicatives (taux de complétion d'une tâche, coût d'un rollback métier, progression d'un objectif de haut niveau) sont mesurées au niveau intention.
- Une intention peut enjamber un commit barrier. Dans ce cas, les actions antérieures au barrier ne sont pas rollbackables, même si l'intention n'est pas encore terminée.
- Le `status` passé à `intent_end` est une valeur déclarée par l'agent (par exemple : `success`, `failure`, `aborted`) — le système ne l'infère pas.

**Termes liés :** Action (unité atomique dont les intentions sont composées), Commit barrier (peut interrompre la rollbackabilité d'une intention).

---

### Agent

**Définition opérationnelle :** Acteur logique unique identifié par un `agent_id` persistant (UUID v7 ou équivalent). Un agent est un acteur unique — il n'a pas de threads internes. La concurrence se fait par décomposition explicite en sous-agents.

**Précisions :**

- Un crash suivi d'un redémarrage ne crée pas un nouvel agent. C'est la même identité (`agent_id`) qui reprend depuis son dernier état committé. Ce point est structurant : l'identité de l'agent n'est pas son PID ou son processus courant, c'est son `agent_id`.
- Les capabilities détenues par un agent survivent à son redémarrage, car elles sont persistées dans le store content-addressed et liées à l'`agent_id`, pas au PID.
- Un agent peut spawner des sous-agents. Les sous-agents héritent d'un sous-ensemble des capabilities de leur agent parent. Cette relation de délégation est tracée par le runtime.
- Ce projet cible le profil d'agent dit "profil B" : durée de vie typique de 1h à 1 mois, maintien d'un état persistant entre actions, volume de 10⁴ à 10⁸ actions au cours de sa vie, sans superviseur humain pour chaque action (voir `Superviseur asymétrique`).

**Termes liés :** Action (ce qu'un agent exécute), Capability (ce qu'un agent détient), Superviseur asymétrique (rôle humain dans la supervision des agents), Commit barrier (point de non-retour dans la vie d'un agent).

---

### Commit barrier

**Définition opérationnelle :** Point d'irréversibilité systémique. Après un commit barrier, les actions qui le précèdent ne peuvent plus être annulées par rollback.

**Précisions :**

- Un commit barrier est inséré selon un mécanisme **hybride conservateur** : auto-trigger pour un ensemble minimal d'effets prouvablement irréversibles au niveau syscall, `commit()` explicite pour tout le reste.
- **Auto-trigger (liste fermée) :** envoi d'un paquet réseau ayant quitté la NIC (`send()` sur socket connectée avec données effectivement transmises), écriture sur un device qui n'est pas le store local content-addressed. Cette liste est conservative par construction : en cas de doute, l'opération n'est pas auto-triggée.
- **`commit()` explicite :** l'agent appelle `commit()` pour tout ce qui n'est pas dans la liste auto-trigger. C'est le cas normal pour les séquences d'actions applicatives.
- **Règle de défaillance :** si un agent ne pose pas de commit barrier avant un effet externe non-auto-triggé, le système lève une exception de type `UncommittedExternalEffect` et suspend l'action jusqu'à ce que l'agent committe ou rollback explicitement.
- Du point de vue du rollback : seules les actions survenues *après* le dernier commit barrier peuvent être annulées. Les actions *avant* le barrier sont définitivement committées.
- Une transaction est définie comme la séquence d'actions entre deux commit barriers consécutifs (voir `Transaction`).

**Termes liés :** Rollback (opération délimitée par les commit barriers), Transaction (séquence d'actions entre deux barriers), État local (ce qui peut être restauré par rollback).

---

### Rollback

**Définition opérationnelle :** Restauration de l'état local du système à l'état qu'il avait à l'instant T. Opération de complexité O(log N) où N est le nombre d'actions depuis le dernier commit barrier.

**Précisions :**

- L'état restauré par un rollback est **l'état local** au sens strict : union de l'état des acteurs locaux, du store local content-addressed, et des messages en transit interne au nœud. Les effets ayant franchi un commit barrier ne sont pas affectés par le rollback.
- Le rollback n'est pas une compensation : il n'envoie pas de messages d'annulation vers l'extérieur, ne rappelle pas des API tierces, ne tente pas d'inverser des effets réseau. Ces mécanismes relèvent de la couche applicative (voir non-objectif `N-rollback-ext` dans `05-non-goals.md`).
- La borne de performance est : durée ≤ 100ms pour un rollback sur les 100 dernières actions.
- La propriété garantie est celle d'un rollback **atomique** : soit toutes les actions de la transaction en cours sont annulées, soit aucune. Il n'existe pas d'état intermédiaire observable.

**Termes liés :** Commit barrier (délimite la portée du rollback), Transaction (unité atomique du rollback), État local (ce qui est restauré).

---

### État local

**Définition opérationnelle :** Union de l'état des acteurs locaux, du store local content-addressed, et des messages en transit interne au nœud.

**Précisions :**

- L'état local est la portée précise de ce que le système peut restaurer par rollback.
- Il exclut explicitement : les effets émis vers l'extérieur du nœud (messages réseau envoyés, appels à des API externes, écriture sur des systèmes de fichiers distants), les états des autres nœuds dans un système distribué, et toute conséquence causale qu'un effet externe a pu provoquer avant le rollback.
- Le store local est content-addressed, ce qui signifie que les versions précédentes de l'état sont accessibles par leur hash — c'est ce qui rend le rollback O(log N) possible sans copie d'état complète.

**Termes liés :** Rollback (opération qui restaure l'état local), Commit barrier (frontière entre état rollbackable et état committé), Store (sous-composant du système qui maintient le store content-addressed).

---

### Capability

**Définition opérationnelle :** Droit d'accès non-ambient, délégable, dérivable et révocable, représenté comme un token opaque détenu par un acteur.

**Précisions :**

- **Non-ambient** : un acteur ne dispose d'aucun accès par défaut. Pour accéder à une ressource, il doit détenir une capability qui l'y autorise explicitement. Il n'existe pas de droits implicites liés à l'identité ou au rôle.
- **Délégable** : un acteur peut transmettre une capability (ou une version restreinte de celle-ci) à un autre acteur. Cette délégation crée une relation parent-enfant tracée par le runtime.
- **Dérivable** : un acteur peut créer une capability dérivée, c'est-à-dire une version plus restreinte de la sienne (moins de droits, portée plus étroite). Il ne peut jamais créer une dérivée avec plus de droits que la capability dont il dispose.
- **Révocable** : la révocation d'une capability invalide récursivement toutes ses dérivées. Cette révocation est propagée via l'arbre de dérivation maintenu par le runtime.
- Pour les capabilities exportées hors du nœud, un mécanisme complémentaire de TTL court (60s–300s) est utilisé : les capabilities exportées sont re-signées périodiquement ; une révocation interne se propage en cessant de re-signer. Ce mécanisme fait l'objet de l'hypothèse `H-revoke` dans `04-hypotheses.md`.

**Références :** [Hardy 1988] "The Confused Deputy" ; [Shapiro 1999] EROS ; [Klein 2009] seL4 ; [Watson 2015] CHERI.

**Termes liés :** Agent (entité qui détient des capabilities), Superviseur asymétrique (entité avec capabilities privilégiées), H-revoke (hypothèse architecturale sur la révocation).

---

### Superviseur asymétrique

**Définition opérationnelle :** Rôle humain dans le système. Le superviseur asymétrique dispose de capabilities privilégiées d'observation, d'intervention et d'autorisation, mais n'exécute pas de tâches via l'OS au sens interactif.

**Précisions :**

- **Observer** : le superviseur peut lire le log causal complet du système — toutes les actions, leurs causes, les capabilities utilisées.
- **Intervenir** : le superviseur peut révoquer des capabilities, suspendre des agents, forcer un rollback.
- **Autoriser** : le superviseur peut signer des actions à fort impact (actions qui auraient été bloquées en attente d'autorisation humaine).
- Le superviseur n'a pas de session interactive au sens classique (pas de terminal, pas de shell). Son interface est un client externe qui communique avec le système via le même protocole que les agents, avec des capabilities différentes (privilégiées).
- L'"asymétrie" du rôle tient à ce que le superviseur n'est pas en boucle pour chaque action — il n'est convoqué qu'à des points de contrôle ou en cas d'événement déclenché par le système.

**Termes liés :** Capability (le superviseur agit via des capabilities privilégiées), Agent (entité supervisée), Non-objectif N-interface-humaine (voir `05-non-goals.md`).

---

### Transaction

**Définition opérationnelle :** Séquence d'actions entre deux commit barriers consécutifs. Atomique du point de vue du rollback : soit toutes les actions de la transaction sont visibles après rollback, soit aucune.

**Précisions :**

- Une transaction n'est pas déclarée explicitement par l'agent : elle est une conséquence de la position des commit barriers. L'agent déclare des commit barriers (ou le système les insère selon le mécanisme retenu) ; les transactions en découlent mécaniquement.
- La propriété d'atomicité s'applique uniquement à **l'état local**. Elle ne concerne pas les effets externes qui auraient franchi un commit barrier.
- Une transaction peut contenir de 1 à N actions. La complexité du rollback est O(log N) sur la taille de la transaction.
- Ce terme est emprunté au vocabulaire des bases de données (ACID), mais son sens ici est restreint : il n'y a pas de notion d'isolation multi-agent dans le sens ACID complet — chaque agent est un acteur unique sans concurrence interne (voir non-objectif `N-concurrency-intra` dans `05-non-goals.md`).

**Références :** [Gray & Reuter 1992] "Transaction Processing: Concepts and Techniques".

**Termes liés :** Commit barrier (délimite une transaction), Rollback (opération qui annule une transaction), Action (unité constituante d'une transaction).

---

---

### Profil T (Tool-calling)

**Définition opérationnelle :** Acteur LLM qui utilise les primitives mémoire, déclenche des tool calls, et opère en mode prose libre. Coût d'inférence : ~6–18 s/cycle sur CPU (qwen2.5:3b). `format=json` désactivé — la prose est fonctionnelle pour le routage outil/texte.

**Précisions :**

- La verbosité de la prose n'est pas un artefact à réduire : elle joue le rôle d'échafaudage de raisonnement pour les modèles < 7B. Contraindre la sortie JSON dégrade la fiabilité du tool call avant de réduire le coût.
- La séparation machine/humain se fait à la couche `emit()` (ADR-0010), pas au niveau de l'API LLM. Le LLM peut générer de la prose en interne ; seul ce que le module WASM publie via `emit` atterrit dans le log causal.

**Termes liés :** Profil D (autre profil), `emit()` (couche de séparation), ADR-0009 (décision).

---

### Profil D (Pure-décision)

**Définition opérationnelle :** Acteur LLM qui prend des entrées structurées et produit une décision JSON sans tool calls. Coût d'inférence : ~2–2,5 s/cycle stable. `format=json` activé. Latence ~3–8× inférieure au profil T.

**Précisions :**

- Utilisable pour les nœuds de merge, d'arbitrage, de scoring, de planification sans accès mémoire.
- Le scheduler doit connaître le profil de chaque acteur pour allouer correctement la capacité d'inférence (ressource bornée).
- Un déploiement mélange des acteurs T et D ; la densité effective dépend du mix.

**Termes liés :** Profil T (autre profil), H-densité-hébergée et H-densité-active (densité mesurée par profil), ADR-0009 (décision), P1a et P1b (propriétés associées).

---

### Content-addressed (trois sens distincts)

**Contexte :** le terme "content-addressed" est utilisé dans ce projet avec trois acceptions distinctes. Les confondre produit des discussions ambiguës.

**(a) `action_id` content-addressed (hash de LogEntry)**

L'`action_id` est le hash SHA-256 d'une `LogEntry` sérialisée (via `bincode`). C'est la clé primaire du log causal (CF `default` de RocksDB). Propriétés : identifiant déterministe, non-forgeable, content-addressed au sens cryptographique.

Utilisé dans P3 : "lookup d'une entrée causale par son `action_id` content-addressed".

**(b) ContentStore content-addressed (Merkle DAG)**

`poc/store/` implémente un store d'états agents sous forme de Merkle DAG. Chaque nœud est adressé par le hash SHA-256 de son contenu. Les snapshot successifs d'un agent forment une chaîne de hashes (analogue à git). C'est ce store qui rend le rollback O(log N) possible.

Utilisé dans ADR-0002 : "RocksDB content-addressed".

**(c) Immutabilité par hash (style NixOS)**

Parfois utilisé au sens de "une fois écrit, le contenu d'un bloc ne change jamais — il peut seulement être remplacé par un nouveau hash". C'est une propriété structurelle qui découle de (a) et (b), pas un mécanisme distinct.

**Règle d'usage :** préciser l'acception quand le contexte est ambigu. Préférer "`action_id` SHA-256" pour (a), "ContentStore" pour (b), "immutable by hash" pour (c).

**Termes liés :** `action_id` (voir P3), ContentStore (voir P2), Rollback (bénéficiaire de (b)).

---

### Modèle A / Modèle B / Modèle C (représentation du log causal)

**Définition opérationnelle :** Trois modèles de représentation des entrées du log causal, définis dans ADR-0006 et tranchés par ADR-0009.

**Modèle A — Supervision continue :** le système maintient en permanence des structures lisibles humainement (log structuré JSON, indexation synchrone). Coût continu par action. Latence de consultation minimale (O(1)).

**Modèle B — Enregistrement minimal + matérialisation fenêtrée :** la machine écrit en continu le strict nécessaire (état hashé, émissions compactes). La couche lisible est matérialisée à la demande pour la fenêtre temporelle interrogée. Coût de stockage ~10–50× plus faible qu'en modèle A. **Modèle adopté par ADR-0009 (2026-05-14).**

**Modèle C — Supervision épisodique pure :** enregistrement uniquement de ce que la machine a besoin de fonctionner. Latence de reconstruction potentiellement non bornée. Rejeté.

**Impacts sur les propriétés :**
- P3 (lookup point par `action_id`) : tient sous A et B (portée par l'index, indépendant du format).
- P3b (range query par agent/fenêtre) : sous A = borne de latence de requête directe ; sous B = garantie d'intégrité de reconstruction.

**Références :** ADR-0006 (définition + choix provisoire modèle A), ADR-0009 (adoption modèle B).

---

### Supervision asynchrone vs supervision épisodique

**Contexte :** ces deux termes sont souvent utilisés de façon interchangeable dans le projet mais recouvrent des dimensions distinctes.

**Asynchrone** (dimension temporelle de l'interaction) : la supervision ne bloque pas l'agent. L'agent peut continuer à exécuter des actions pendant qu'un humain inspecte le log ou prépare une intervention. La réponse humaine arrive comme un message dans la boîte aux lettres de l'agent.

**Épisodique** (dimension de fréquence) : la supervision est rare — heures à jours entre deux inspections. Ce terme caractérise *à quelle fréquence* un humain regarde, pas *comment* l'interaction se déroule.

**Un modèle de supervision peut être :**
- Asynchrone et fréquent (inspection non-bloquante toutes les minutes) — cas d'un développeur qui monitore en temps réel.
- Asynchrone et épisodique (inspection non-bloquante une fois par jour) — cas cible du profil B.
- Épisodique et synchrone (rare mais bloquant : l'agent attend que l'humain réponde) — cas A3 (`agent_request_validation`).

**Usage dans ce projet :**
- H-supervision utilise "asynchrone" pour signifier que le superviseur n'est *pas en boucle pour chaque action*. Ce sens combine les deux dimensions.
- ADR-0006 utilise "épisodique" pour caractériser la fréquence du profil B.
- La primitive A3 est une supervision synchrone et épisodique : l'agent bloque en `AwaitingValidation` jusqu'au verdict humain.

**Règle d'usage :** utiliser "asynchrone" pour parler du mode d'interaction (bloquant vs non-bloquant), "épisodique" pour parler de la fréquence.

**Termes liés :** H-supervision, A3 (`agent_request_validation`), ADR-0006, ADR-0014.

---

## 3. Termes empruntés à l'état de l'art (avec leur sens précis ici)

<!-- TODO: à compléter au fur et à mesure que d'autres termes techniques sont introduits dans le projet. -->

---

## 4. Termes délibérément évités et pourquoi

<!-- TODO: à compléter lorsque des choix de vocabulaire sont arbitrés — par exemple : pourquoi on dit "agent" plutôt que "process", "commit barrier" plutôt que "savepoint", etc. -->
