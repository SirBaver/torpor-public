
# 04 — Hypothèses

## 1. Méthode : distinguer hypothèse et assertion

### 1.1 Définition : une hypothèse est une affirmation non encore vérifiée dont on dépend

Une hypothèse architecturale est une affirmation qu'on tient pour vraie dans les décisions de conception, sans l'avoir encore démontrée expérimentalement dans ce contexte. Si l'hypothèse s'avère fausse, elle invalide les décisions qui en dépendent. Ce document les rend explicites pour permettre un plan d'invalidation.

### 1.2 Classification : criticité (bloquante / importante / secondaire), testabilité, domaine

- **Bloquante** : si cette hypothèse tombe, l'architecture centrale doit être révisée.
- **Importante** : si cette hypothèse tombe, des composants significatifs doivent être modifiés.
- **Secondaire** : si cette hypothèse tombe, des ajustements ponctuels suffisent.

---

## 2. Hypothèses sur les agents IA comme utilisateurs

### H-profil-B — Le profil d'usage dominant est long-courrier avec supervision ponctuelle

**Identifiant :** H-profil-B

**Énoncé :** Les agents déployés sur ce système ont un profil d'exécution correspondant au profil B défini dans `01-vision.md` : durée de vie 1h–1 mois, volume d'actions 10⁴–10⁸/lifetime, supervision humaine périodique (heures à jours), spawn de sous-agents comme pattern courant. Ce profil est le dimensionnant pour toutes les décisions d'architecture.

**Criticité :** Bloquante — si les agents déployés sont massivement des agents courts (< 1min, < 100 actions), les coûts fixes du rollback et du log causal ne sont plus amortis, et P1 devient impossible à satisfaire. Le design est optimisé pour le long-courrier ; adapter rétroactivement à du court-courrier demanderait une révision de P2 et P3.

**Domaine :** Comportement des utilisateurs / adéquation du modèle.

**Condition de réfutation :** Sur un échantillon représentatif de déploiements en conditions réelles, plus de 80% des agents ont une durée de vie inférieure à 10 minutes ou un volume d'actions inférieur à 10³. Dans ce cas le profil C (agent éphémère, style serverless) devient dominant et le design doit être révisé.

**Plan B :** Si le profil court-courrier domine, reconsidérer l'architecture vers un modèle à spawn ultra-rapide (< 100µs) sans log causal persistant, et positionner P2 comme opt-in plutôt que par défaut.

**Références :**
- `spec/01-vision.md` section définissant le profil B
- LangGraph, Temporal — patterns observés en production (voir `spec/03-state-of-the-art.md`)

---

### H-action-grain — La granularité d'action est de l'ordre de 10³–10⁵ par agent par heure

**Identifiant :** H-action-grain

**Énoncé :** Le flux d'actions d'un agent en production est de l'ordre de 10³ à 10⁵ messages traités par heure et par agent. Cette borne inférieure est le plancher pour que le log causal soit utile (assez d'événements pour reconstruire une intention). Cette borne supérieure est le plafond au-delà duquel le débit du log causal et de l'index d'action_id devient un goulot d'étranglement sur le budget CPU de P7.

**Criticité :** Importante — si la granularité réelle dépasse 10⁶/heure, les garanties p99 ≤ 10ms de P3 deviennent difficiles à tenir sans un index plus sophistiqué. Si elle est inférieure à 10²/heure, la granularité d'action est trop grossière pour capturer la causalité utile.

**Domaine :** Comportement des agents / dimensionnement du log causal.

**Condition de réfutation :** Mesure sur workloads représentatifs (W1, W2, W3) d'un débit médian sortant du corridor 10³–10⁵ actions/heure sur la majorité des agents.

**Plan B :** Si le débit dépasse 10⁵, passer à un log causal à échantillonnage stratifié (toutes les actions sauf les messages inter-agents courants ; seulement les actions ayant des effets externes). Si le débit est inférieur à 10², aggréger les actions en intentions et indexer au niveau de l'intention.

---

### H-supervision — La supervision humaine est asymétrique et ne nécessite pas d'interaction en temps réel

**Identifiant :** H-supervision

**Énoncé :** Les humains qui supervisent les agents de profil B n'ont pas besoin d'interagir en temps réel avec l'agent en cours d'exécution. Leur rôle se résume à trois primitives asynchrones : observer (via le log causal), intervenir (suspendre/rollback/révoquer une capability), autoriser (signer une action à fort impact). Aucune interface de type terminal interactif n'est nécessaire.

**Criticité :** Importante — si la supervision requiert une interaction temps réel (latence < 1s entre la question de l'agent et la réponse humaine), le modèle asynchrone est insuffisant et il faut ajouter un canal de communication synchrone avec l'agent, ce qui modifie le modèle acteur et le commit barrier.

**Domaine :** Modèle d'interaction humain-agent.

**Condition de réfutation :** Dans plus de 20% des cas d'usage collectés sur des déploiements réels, l'agent bloque en attente d'une réponse humaine pendant plus de 1 minute dans le cours normal de son exécution (hors phase d'autorisation explicite de P4).

**Plan B :** Ajouter un canal `human_input(question, timeout)` dans l'API d'agent, avec le commit barrier déclenché automatiquement avant la question. La réponse humaine est traitée comme un message entrant ordinaire.

---

## 3. Hypothèses sur les contraintes système

### H-revoke — Modèle de capabilities à dérivation tracée et TTL pour l'export

**Identifiant :** H-revoke

**Énoncé :** Le système adopte un modèle de capabilities à dérivation tracée. Toute délégation de capability d'un acteur A vers un acteur B crée une relation parent-enfant tracée par le runtime. La révocation d'une capability invalide récursivement toutes ses dérivées. Pour les capabilities exportées hors du nœud, un mécanisme complémentaire de TTL court est utilisé.

**Criticité :** Bloquante — ce choix conditionne la faisabilité du modèle de confiance. Si ce mécanisme ne tient pas aux fréquences de révocation et de re-signature rencontrées en production, l'hypothèse fondamentale de sécurité du système tombe.

**Domaine :** Faisabilité technique / modèle de sécurité.

**Détail du mécanisme :**

- En interne au nœud : l'arbre de dérivation des capabilities est maintenu par le runtime. La révocation d'une capability parcourt l'arbre et invalide toutes les dérivées. Coût en stockage : O(N) où N est le nombre de capabilities vivantes.
- Pour les capabilities exportées hors du nœud : un TTL court de 60s à 300s est appliqué. Les capabilities exportées sont re-signées périodiquement par le nœud émetteur. Une révocation interne se propage en cessant de re-signer : les nœuds distants voient la capability expirer à la fin de son TTL courant.

**Condition de réfutation :** L'hypothèse est réfutée si l'une des conditions suivantes est observée en production :
- La fréquence de révocation dépasse la fréquence de re-signature (le TTL ne joue plus son rôle de tampon).
- Le coût d'entretien de l'arbre de dérivation dépasse 5% du budget CPU du nœud sous charge représentative.

**Plan B :** Si H-revoke est réfutée, basculer vers des *revocable forwarders* (approche B, [Stiegler 2010] E language) : chaque capability exportée est un objet forwarder dont la révocation coupe le forward, sans nécessiter un arbre de dérivation global.

**Références :**
- [Hardy 1988] "The Confused Deputy (or why capabilities might have been invented)"
- [Shapiro 1999] "EROS: A Fast Capability System"
- [Klein 2009] "seL4: Formal verification of an OS kernel"
- [Watson 2015] "CHERI: A Hybrid Capability-System Architecture"
- [Stiegler 2010] "An Introduction to E and CapTP" (revocable forwarders)

**Termes liés :** Capability (voir `06-glossary.md`), Superviseur asymétrique (révocation comme primitive de supervision).

**Résultat empirique (lab phase 4, 2026-05-13) :** **Partiellement validée.** Le mécanisme fonctionnel intra-nœud est confirmé sur cinq scénarios :
- Accès autorisé avec capability active (P4.1) : HTTP 200 ✓
- Révocation directe → accès refusé (P4.2) : HTTP 403 capability_denied ✓
- Révocation du parent → dérivée invalide sans la toucher (P4.3) : chain lazy propagée ✓
- Dérivée depuis parent révoqué rejetée au grant (P4.4) : HTTP 400 ✓
- `memory_list` filtré par scope de capability (P4.5) : 0 clé hors scope exposée ✓

**Nuance de périmètre :** la validation porte sur le mécanisme intra-nœud (SQLite, lazy chain). Ce qui reste non validé : (2) TTL pour les capabilities exportées hors du nœud — hors scope du lab (intra-nœud uniquement). L'audit logging des refus est implémenté et validé (P4.6, `capabilities.py::log_denied`) — ce gap est résolu.

**Résultat empirique — performance à l'échelle (lab Phase 5, 2026-05-13, `poc/capabilities/`) :**

| Opération | N caps | Médiane | Cible | Statut |
|-----------|--------|---------|-------|--------|
| `check()` hot path | 11 111 | p99 = 361 ns | ≤ 1 µs | ✓ |
| `revoke()` arbre entier | 1 111 | 49 µs | < 1 ms | ✓ |
| `revoke()` arbre entier | 11 111 | 736 µs | < 1 ms | ✓ |
| `revoke()` arbre entier | 111 111 | **20 ms** | < 1 ms | ✗ |

**`check()` est O(1) et conforme.** Un HashMap lookup — stable quelle que soit la taille du store.

**`revoke()` est O(N) HashMap : plafond architectural à ~10K caps pour tenir < 1ms.** À N=100K, le dataset excède le L2 cache et les cache misses dominent (~0.2 µs/nœud). Toutefois, le critère spec est *< 5% CPU sous W1*, pas < 1ms absolu : une révocation de 100K caps une fois par minute représente 0.03% CPU — conforme. Le plafond ne se matérialise que si des arbres de capabilities croissent au-delà de ~10K nœuds, ce qui suppose un spawn très profond ou très large (depth=5, branching=10).

Si ce seuil est atteint en production, deux options documentées dans `lab/LESSONS.md` §L21 : (a) epoch-based revocation O(1) ; (b) revocable forwarders (Plan B existant).

Décisions déclenchées : `decisions/0005-design-capabilities-revoke.md` (design validé) ; `lab/LESSONS.md` §L11–L13 (observations pré-implémentation) ; `daemon/capabilities.py` (implémentation de référence).

---

## 4. Hypothèses sur la faisabilité technique

### H-inférence-coût — La latence d'inférence est une ressource dimensionnante au niveau du scheduler

**Identifiant :** H-inférence-coût

**Énoncé :** La latence d'un appel LLM (de la requête à la fin de génération) est suffisamment grande pour que le scheduler d'agents doive la modéliser comme une ressource, au même titre que le CPU ou la mémoire. À 7 tok/s sur CPU (mesuré en lab sur qwen2.5:3b, Alpine, sans GPU), une chaîne de 10 appels LLM séquentiels représente 1 à 5 minutes d'attente. Même avec GPU (quelques centaines de tok/s), la latence de premier token (~100–500ms) reste significative pour des boucles agentiques serrées.

Cette hypothèse se décline en deux sous-assertions :
- **H-inférence-coût/séquentiel** : les chaînes d'appels LLM séquentiels (orchestrateur → sous-agent → sous-sous-agent) créent des goulots linéaires non-compressibles. La profondeur de spawn est un multiplicateur de latence.
- **H-inférence-coût/parallèle** : le parallélisme d'appels LLM (N sous-agents inférents simultanément) est limité par la VRAM disponible et le débit du modèle, pas seulement par le CPU ou le réseau.

**Criticité :** Importante — si cette hypothèse est confirmée, P1 (densité d'agents) doit être qualifiée : la densité maximale est bornée non seulement par la mémoire et le CPU, mais par la capacité d'inférence disponible. Un agent "actif" en attente d'une réponse LLM est suspendu mais consomme une unité de capacité d'inférence.

**Domaine :** Faisabilité technique / modèle de performance.

**Origine empirique :** Mesure directe en lab phase 1 — qwen2.5:3b sur CPU, ~7 tok/s, réponses courtes (~50 tokens) en 6–8s, réponses longues (~200 tokens) en 25–30s. Voir `lab/LESSONS.md` §L3.

**Condition de réfutation :** La latence d'inférence devient négligeable (< 50ms bout-en-bout) sur le hardware de production cible, de sorte qu'une chaîne de 10 appels représente < 500ms au total — latence comparable à un appel réseau, absorbable par le scheduler sans traitement spécifique.

**Plan B :** Si H-inférence-coût est confirmée (cas probable), le scheduler doit distinguer les états "en calcul" et "en attente d'inférence". La densité d'agents actifs (P1) doit être bornée par la capacité d'inférence disponible, pas seulement par la mémoire. Les boucles agentiques profondes doivent être découragées architecturalement (coût explicite par niveau de spawn).

**Résultat empirique (lab phase 2, 2026-05-12) :** **Confirmée.** Sur CPU (qwen2.5:3b, Alpine, sans GPU) : orchestrateur 13 747ms, agent-a 15 475ms, agent-b 17 532ms, merge 5 263ms. Total 4 appels = 52s. Extrapolation 10 appels = ~130s >> seuil de 30s. Décision déclenchée : intégrer la capacité d'inférence comme ressource bornée dans P1 et le scheduler phase 3+. Voir `lab/LESSONS.md` §L9.

**Résultat empirique (Phase 10, 2026-05-30 — OllamaBackend réel, llama3.2:3b, CPU) :** Confirmée avec backend réel. P10-S3 (6 workers, cap=2) : t_infer médiane 12,5 s, p99 18 s. P10-S5 (3 fg + 1 sv, cap=1) : t_infer similaire. Ces valeurs CPU (13–18 s) sont non transférables au hardware cible GPU 24 GB (spec/07 §2, ADR-0052 §D2). La confirmation reste qualitative sur CPU ; la borne sur hardware GPU de production reste à mesurer (ADR-0050 §D6 C2/hardware représentatif, bloqué infra). Voir `poc/scenarios/P10-S3/VERDICT.md`, `poc/scenarios/P10-S5/VERDICT.md`.

**Références :**
- `lab/LESSONS.md` §L3 — mesure empirique 7 tok/s
- `lab/LESSONS.md` §L9 — validation empirique phase 2
- `spec/02-properties.md` §P1 — densité d'agents
- `spec/04-hypotheses.md` §H-action-grain — granularité d'action et débit

---

### H-mémoire-schema — Un store clé-valeur non contraint est insuffisant pour la cohérence mémoire multi-agents

**Identifiant :** H-mémoire-schema

**Énoncé :** Quand plusieurs agents accèdent à un store mémoire partagé sans convention de nommage ou de typage, la cohérence sémantique se dégrade : des agents différents utilisent des clés différentes pour le même concept (`name`, `user_name`, `first_name`, `prenom`), les valeurs s'écrasent sans détection de conflit, et aucune traversée du store ne permet de reconstruire une vue cohérente de l'état partagé.

Cette incoherence n'est pas corrigeable par des instructions dans le system prompt — le modèle fait des choix de nommage autonomes que l'instruction-following ne contrôle pas fiablement à < 7B paramètres.

**Criticité :** Importante — si confirmée, le design de l'API mémoire doit imposer un niveau de structuration supplémentaire : namespaces par agent ou par domaine, clés canoniques définies dans un schéma partagé, ou typage des valeurs.

**Domaine :** Design des primitives / cohérence multi-agents.

**Origine empirique :** Observé en lab phase 1.5 — le modèle a écrit `first_name=Joey` (T4a) puis lu `name=Charlie` (T4b), ne trouvant pas son propre enregistrement. Même problème en T2 (`name=Charlie` alors que le test vérifiait `user_name`). Voir `lab/LESSONS.md` §L6.

**Condition de réfutation :** Sur un workload représentatif avec un modèle ≥ 7B, la cohérence de nommage est maintenue spontanément dans > 95% des cas, rendant un schéma contraint non nécessaire. Ou : un schéma de nommage imposé uniquement dans le system prompt est suffisant pour garantir la cohérence.

**Plan B :** Si H-mémoire-schema est confirmée (cas probable), l'API mémoire de production doit inclure : (a) des namespaces par agent (`agent_id/key`), (b) un registre de clés canoniques partagées avec validation de type à l'écriture, (c) un mécanisme de résolution de conflits explicite. Le store clé-valeur nu devient une couche interne non exposée directement aux agents.

**Résultat empirique (lab phase 2, 2026-05-12) :** **Confirmée.** Deux agents avec exactement le même system prompt ont mémorisé "Dupont" sous deux clés distinctes : `user.family.last_name` (agent-a) et `user.family.name` (agent-b). Aucune instruction dans le prompt ne suffit à garantir la convergence. Décision déclenchée : ADR-0004 (schéma mémoire structuré). Voir `lab/LESSONS.md` §L8.

**Références :**
- `lab/LESSONS.md` §L6 — observation empirique de l'incohérence de nommage
- `lab/LESSONS.md` §L8 — validation empirique phase 2
- `lab/tests/smoke_test.sh` §T4, P2.3 — tests de mémorisation et cohérence inter-agents
- `spec/04-hypotheses.md` §H-supervision — le superviseur doit pouvoir inspecter l'état mémoire partagé
- `decisions/0004-schema-memoire.md` — ADR déclenché par cette confirmation


---

### H-causal-latence — Le lookup causal O(1) sur 10⁸ entrées est faisable avec p99 ≤ 10ms sur un LSM tree

**Identifiant :** H-causal-latence

**Portée (Q1, 2026-05-16) :** Cette hypothèse porte exclusivement sur **P3a** (lookup point `get(action_id)` sur log statique, sans write concurrent, sans fsync sur le chemin chaud). Elle ne couvre ni P3b (end-to-end emit→fsync→get, borne 20 ms) ni P3c (multi-agent concurrent, bornes ≥ 50 ms). Voir `spec/02-properties.md §P3` pour la décomposition.

**Énoncé :** Sur un substrate LSM (RocksDB) correctement configuré (bloom filter 10 bits/clé, block cache LRU 256 MB, pas de compression), un lookup par `action_id` (clé opaque SHA-256 32 bytes) satisfait p99 ≤ 10ms sur un dataset de 10⁸ entrées (~10–15 GB sur disque). Cette borne est la condition d'habitabilité de P3a : un superviseur ou un agent qui interroge le log causal reçoit une réponse en temps humainement invisible.

**Criticité :** Bloquante — si p99 dépasse 10ms à N=10⁸, P3 tombe et Layer 0 doit soit changer de substrate, soit adopter un index secondaire, soit sacrifier la complétude du log (échantillonnage). Aucune de ces alternatives n'est anodine.

**Domaine :** Faisabilité technique / performance du log causal.

**Condition de réfutation :** Sur hardware de qualification (NVMe ≥ 1 GB/s, 16 GB RAM), `cargo bench --bench causal_lookup` avec `BENCH_N=100000000` produit un p99 > 10ms. Les cache misses à N=10⁸ sont le vrai régime de test — le cache de 256 MB ne couvre que ~2% du dataset.

**Plan B :** Si H-causal-latence est réfutée sur RocksDB standard :
- Option 1 : augmenter le block cache (512 MB–2 GB) pour réduire la fréquence des cache misses — efficace si le working set est concentré sur une fraction des entrées.
- Option 2 : activer la compression LZ4 (–40% taille disque, +5–20µs/lookup) et mesurer le trade-off sur hardware NVMe rapide.
- Option 3 : log causal à échantillonnage stratifié — toutes les actions ayant des effets externes, sous-ensemble des actions internes — pour rester sous N=10⁷ dans le cas courant.

**Résultat empirique (lab Phase 5, 2026-05-13) :** **Indicatif** (selon `benchmarks/test-protocol.md` §6.1 — 1 hardware, régime cache chaud, 1 run). Sur machine de développement (N=10⁶, dataset ~10 MB, entièrement dans le cache) :

| Percentile | Latence |
|------------|---------|
| p50        | 4 µs    |
| p95        | 8 µs    |
| p99        | 11 µs   |
| p99.9      | 18 µs   |

P3 (p99 ≤ 10ms) satisfait avec une marge de ~900×. Ce résultat est sur régime cache chaud — la qualification officielle T5 (N=10⁸, hardware NVMe, cache misses réels) reste à faire. Voir `lab/LESSONS.md` §L19.

**Références :**
- `poc/causal-log/src/lib.rs` — implémentation Layer 0
- `poc/causal-log/benches/causal_lookup.rs` — benchmark T5
- `spec/02-properties.md` §P3 — propriété de latence causale
- `lab/LESSONS.md` §L17 — pourquoi pas SQLite ; §L18 — options RocksDB critiques ; §L19 — résultats T5 dev
- ADR-0002 — choix du substrate RocksDB

---

### H-rollback-latence — Le rollback à profondeur ≤ 100 est faisable avec p95 ≤ 100ms sur W2

**Identifiant :** H-rollback-latence

**Criticité :** Bloquante — si le rollback est trop lent, P2 (réversibilité) n'est pas opérationnelle. Sans rollback utilisable, la supervision humaine perd sa primitive d'intervention principale.

**Énoncé :** Le rollback d'état agent jusqu'à un snapshot passé à profondeur ≤ 100 actions est faisable avec p95 ≤ 100ms sur le workload W2 (blocs de 500 KB). Le rollback n'est pas un chemin chaud (il n'intervient qu'en intervention superviseur) mais doit rester suffisamment rapide pour ne pas bloquer une intervention de sécurité.

**Condition de réfutation :** p95 > 100ms sur hardware représentatif (profondeur 100, blocs 500 KB, DB non entièrement en cache).

**Plan B :** Si H-rollback-latence est réfutée :
- Option 1 : limiter la profondeur de rollback en garantissant des snapshots intermédiaires tous les N actions (pagination), réduisant le chemin effectif à O(N) borné.
- Option 2 : lazy rollback — retourner immédiatement un token de rollback et matérialiser l'état restauré en arrière-plan (P2 assouplie : "rollback initié" != "état restauré").
- Option 3 : vérifier si le bottleneck est I/O (cache miss headers CF) → augmenter le block cache dédié à la CF headers.

**Résultat empirique (lab Phase 5, 2026-05-13) :** **Indicatif** (1 hardware, régime cache chaud, 1 run — voir `benchmarks/test-protocol.md` §5). Sur machine de développement (chaîne de 1001 snapshots, headers CF ~140 KB entièrement en cache) :

| Workload | Depth | p50 | p95 | p99 |
|----------|-------|-----|-----|-----|
| W1 (50 KB) | 100 | 71 µs | 88 µs | 107 µs |
| W2 (500 KB) | 100 | 71 µs | **99 µs** | 111 µs |
| W2 (500 KB) | 1000 | 724 µs | 837 µs | 1052 µs |

P2 (p95 ≤ 100ms sur W2/depth=100) satisfait avec une marge de ~1000×. **Observation clé :** la taille des blocs (W1 vs W2) n'a aucun effet sur la latence de rollback — `rollback_path` ne lit que les headers (CF dédiée), pas les données. Le rollback est O(depth) en traversées de la chaîne de parents. Voir `lab/LESSONS.md` §L20.

Pour passer à "Partiellement validé" : qualification sur état de DB froid (dataset >> block cache) avec K ≥ 3 runs.

**Références :**
- `poc/store/src/lib.rs` — implémentation ContentStore
- `poc/store/benches/rollback_latency.rs` — benchmark H-rollback-latence
- `spec/02-properties.md` §P2 — propriété de réversibilité
- `lab/LESSONS.md` §L20 — résultats benchmark rollback dev
- ADR-0002 — choix du substrate RocksDB

---

### H-commit-barrier — Intercepter tout effet externe avant commit est faisable avec un overhead borné

**Identifiant :** H-commit-barrier

**Criticité :** Bloquante — P2 (rollback) et P6 (atomicité crash) reposent tous deux sur l'existence d'un point de commit durable avant tout effet externe. Si ce mécanisme ne peut pas être implémenté correctement (interception complète) ou introduit un overhead qui détruit P1 (densité), les deux propriétés tombent simultanément.

**Domaine :** Faisabilité technique / substrat runtime.

**Énoncé :** Le commit barrier — écriture durable d'un snapshot d'état + entrée de log causal, déclenchée de manière synchrone avant tout effet externalisable (appel WASI, émission de message inter-acteur) — est :
1. **Complet** : aucun effet externe ne peut se produire sans qu'un commit barrier ait été enregistré. L'interposition WASI Preview 2 permet cette garantie sans modification du code agent.
2. **À overhead borné** : sur W1 (1 action toutes les 5 secondes par agent), l'overhead cumulatif du commit barrier sur N agents simultanés ne dépasse pas 5% du budget CPU du nœud.

**Condition de réfutation :** L'hypothèse est réfutée si l'une des deux conditions suivantes est observée :
- Un effet externe est produit (message livré, socket write) sans qu'une entrée de commit barrier correspondante soit trouvée dans le log causal → violation de complétude.
- Avec N agents W1 simultanément actifs sur le hardware de référence, le coût mesuré du commit barrier (write store + write log causal, avec durabilité WAL) dépasse 5% du CPU total → overhead prohibitif pour P1.

**Décomposition en deux sous-hypothèses :**

*H-cb-correct* : L'interposition WASI via Wasmtime intercepte 100% des effets externalisables — aucun appel de syscall externe n'échappe à l'interception. Vérifiable par SEF-4 (crash pendant une transaction) et SEF-6 (replay déterministe).

*H-cb-overhead* : Sur NVMe ≥ 1 GB/s, un commit barrier (1 block write + 1 snapshot header write + 1 causal log append en batch RocksDB) est exécuté en ≤ 500µs (p99), soit < 0.01% d'un cycle W1 de 5 secondes par agent. Le scaling à N agents concurrents est sous-linéaire grâce aux batches WAL RocksDB.

**Plan B :** Si H-cb-overhead est réfutée sur hardware cible :
- Option 1 : commit barrier asynchrone — le write est lancé avant que l'effet soit autorisé, mais le syscall de durabilité (fsync) est différé. Réduit la latence au prix d'une fenêtre de vulnérabilité en cas de crash.
- Option 2 : granularité de commit configurable — le barrier se déclenche seulement sur les effets qualifiés de "durables" par le superviseur asymétrique (écriture fichier oui, envoi de message oui, lecture seule non).

Si H-cb-correct est réfutée (trou dans l'interception WASI) :
- Restreindre les agents au périmètre WASI Preview 2 strict ; interdire les modules qui utilisent les syscalls non interposables via Wasmtime.

**Références :**
- `poc/runtime/src/lib.rs` — scaffolding du runtime avec Wasmtime
- `spec/02-properties.md` §P2 — rollback transactionnel (bénéficiaire principal)
- `spec/02-properties.md` §P6 — atomicité crash (corollaire du commit barrier)
- `spec/02b-substrate-requirements.md` §S4 — interception des effets externes
- `spec/02b-substrate-requirements.md` §S6 — non-déterminisme via primitives substituables
- ADR-0002 — choix Wasmtime comme substrat

**Résultat empirique (Phase 5, 2026-05-13, `poc/runtime/`) :** **Indicatif** (1 hardware, régime cache chaud, 1 run).

*H-cb-correct* : **✓ Structurellement validée.** Le module WAT impose `commit_barrier` avant `emit` dans le call graph — aucun chemin d'exécution ne peut produire un effet externe sans barrière préalable. Le `debug_assert!(barrier_fired)` dans la host function `emit` confirme l'invariant à l'exécution. La propriété est architecturalement garantie par la topologie du module, pas par validation défensive.

*H-cb-overhead* : **✓ Conforme sur machine de développement.** 1 000 cycles `process_one` (commit_barrier + snapshot ContentStore + append CausalLog + emit) sur TempDir :

| Métrique | Valeur |
|----------|--------|
| p50      | 10 µs  |
| p95      | 16 µs  |
| p99      | 26 µs  |
| moyenne  | 11 µs  |
| Criterion mean | 18.7 µs |

Overhead W1 : 11 µs / 5 000 000 µs = **0.0002%** (cible ≤ 5% — marge de 25 000×).

Régime : cache chaud, store et log dans TempDir, dataset << RAM. Le chiffre représente le coût RocksDB en cache warm — pas le régime longue durée avec dataset >> cache. Voir `lab/LESSONS.md` §L22 pour l'analyse de P6 (atomicité crash) et des limites de cette mesure.

---

### H-densité-hébergée — Densité d'agents parqués sur Wasmtime/Tokio

**Identifiant :** H-densité-hébergée

**Criticité :** Bloquante — c'est le premier critère de réussite de la densité (P1a). Si la densité d'agents idle n'atteint pas ≥ 5× la baseline Docker, la proposition de haute densité par isolation légère est réfutée.

**Domaine :** Faisabilité technique / substrat runtime.

**Énoncé :** Sur le hardware de référence (8 cœurs, 16 GB RAM, NVMe), un agent Wasmtime idle (en état `LifecycleState::Suspended` ou `::Active` sans inférence, attendant un message dans son inbox Tokio) consomme ≤ 5 KB de RAM (pages WASM CoW non faulted). Comparé à la baseline Docker réaliste (container avec runtime agent LLM : Python 3.11 + dépendances ≈ 100–200 MB RSS à l'état idle), cela permet d'héberger au moins R_idle ≥ 5× plus d'agents sur la même RAM totale.

**Mécanisme sous-jacent :** Le gain provient de deux propriétés architecturales mesurées :
1. **Overhead minimal d'un agent idle** : un acteur Wasmtime inactif réserve une mémoire WASM (pages virtelles) mais n'en faulte aucune en physique — seulement les métadonnées runtime (~5 KB) sont résident. Un container Python, même idle entre deux actions, garde l'interpréteur Python en RAM (~100–200 MB).
2. **Store mutualisé** : le ContentStore RocksDB est partagé entre N acteurs (cache 256 MB total, amortisé, pas × N par agent). Un volume Docker est isolé par container.

**Condition de réfutation :** Le ratio Wasmtime idle / Docker (baseline réaliste avec runtime Python LLM) est < 5× sur hardware de référence ; c'est-à-dire que la RAM idle par agent Wasmtime est ≥ 80% de celle d'un container Docker réaliste idle. Mesure : overhead `MemAvailable` host sur 16 GB total, N containers / N agents, régime stable.

**Baseline spécification :** Docker baseline = `python:3.11-slim` avec dépendances agent LLM minimales (langchain-core, openai, httpx, pydantic) en état idle (agent en attente de message, pas en traitement actif). Overhead idle mesuré par delta `MemAvailable` hôte = ~37–43 MB par container (RSS userspace + slab kernel). Cette baseline est plus pertinente qu'Alpine `sleep infinity` (~4,4 MB) car elle reflète le vrai coût d'un agent applicatif.

**Note de transparence :** L'avantage mesurable de Wasmtime provient du contraste CoW (Copy-on-Write) entre pages WASM virtuelles et interpréteur Python résident. Cette différence est attendue, architecturale, et refléterait toute hypothèse substrat-différent (JVM, Node.js, etc.) : aucun code applicatif complet ne peut s'allouer "0 MB". Le différentiel avantage de ~10 000× entre 5 KB Wasmtime et 100 MB Python est honnête et capturé de manière transparente.

**Plan B :** Si H-densité-hébergée est réfutée sur Wasmtime :
- Investiguer d'autres substrats : eBPF programs (overhead sub-MB par "agent" mais périmètre fonctionnel limité), processus légers avec namespaces partagés, ou unikernels multi-tenant. Chaque option implique des compromis sur P4 (isolation) ou P5 (déterminisme).
- Réévaluer le ratio cible : un gain de 3× au lieu de 5× est insuffisant comme preuve de thèse architecturale mais peut suffire pour un contexte de déploiement spécifique.

**Références :**
- `spec/02-properties.md` §P1a — densité hébergée (propriété cible)
- `benchmarks/reference-workload.md` §W1 — définition du workload de mesure
- `spec/01-vision.md` §2.4 — portée épistémique LLM vs agents généraux
- ADR-0002 — choix Wasmtime

**Résultat empirique (lab Phase 5, 2026-05-14, `benchmarks/t6-docker-python-baseline.sh` + `poc/benchmarks/ -- t6`) :** **Indicatif** (1 hardware, 1 run — voir `benchmarks/test-protocol.md` §5).

**Mesures Wasmtime idle (T6 dev) :**

| Mode | Overhead/acteur | Critère RAM ≤ 10 MB |
|------|-----------------|---------------------|
| Minimal (AGENT_WAT) | **5 KB** | ✓ (×2 000) |
| W1 (infrastructure partagée incluse) | ~5 KB/agent idle | ✓ |

Infrastructure partagée (Engine + ContentStore + Log) : +3 324 KB (coût unique amortisé sur N agents).

**Mesures Docker baseline Python LLM (python:3.11-slim + langchain-core + openai + httpx + pydantic, idle) :**

| Méthode | Overhead/container | Agents sur 16 GB |
|---------|-------------------|----|
| (A) Delta MemAvailable hôte | **43 314 KB (42,3 MB)** | ~387 |
| (B) RSS userspace | **37 683 KB (36,8 MB)** | ~447 |

**Comparaison densité 16 GB :**

| Substrat | Overhead idle | Agents sur 16 GB | Ratio vs Docker |
|----------|--------------|------------------|-----------------|
| Wasmtime | 5 KB | ~3 355 443 | **8 670×** (vs A) |
| Docker Python LLM (A) | 43 314 KB | ~387 | baseline |

**H-densité-hébergée ≥ 5× atteinte** sur le modèle W1 révisé. Le ratio est de **8 670×** contre la baseline Docker réaliste (Python 3.11 + deps agent LLM).

**Interprétation :** la cible ≥ 5× est satisfaite avec une marge de ×1 500. La mesure de 37–42 MB par container est le plancher bas (langchain-core seul, sans numpy ni extensions lourdes). Un agent LLM complet (langchain + numpy + embeddings) serait 80–150 MB idle, renforçant encore le ratio.

Voir `lab/LESSONS.md` §L27 (analyse T6 Wasmtime dev) et §L28 (T6-docker-réaliste Python LLM).

---

### H-densité-active — Débit d'actions sur Wasmtime/Tokio sous workload W1

**Identifiant :** H-densité-active

**Criticité :** Bloquante — c'est le second critère de réussite de la densité (P1b). Si le débit d'actions/s ne soutient pas un ratio ≥ 2× vs Docker sous charge active, la proposition de haute densité applicative s'effondre.

**Domaine :** Faisabilité technique / substrat runtime / scheduler.

**Énoncé :** Pour un workload W1 actif (`idle_fraction = 0.5`, 50% du temps en traitement actif, 50% idle), le système Wasmtime/Tokio soutient un débit d'actions/seconde au moins R_actif ≥ 2× supérieur à la baseline Docker, à p99 latence d'action ≤ 2 secondes, sur le même hardware de référence.

**Convention de paramètre :** `idle_fraction = 0.5` signifie que chaque agent passe 50% du temps à exécuter le cycle W1 complet (réception message → introspect → infer simulé 2,5s → commit_barrier → emit) et 50% du temps idle en attente du message suivant. Cette fraction est une valeur de convention neutre en l'absence de données de production ; elle sera réévaluée quand des profils d'agents LLM réels sont disponibles.

**Définition opérationnelle :** Agent "actif" = agent exécutant le cycle W1 complet avec inférence simulée `SleepyBackend(delay_ms=2500)` + commit_barrier + emit. L'inférence est bornée en durée (2,5s par action) pour reproduire le profile d'agents IA.

**Métrique :** Débit d'actions/seconde = `total_emits / wall_clock_duration`, mesuré sur N agents Wasmtime simultanés vs N containers Docker exécutant le même cycle W1.

**Métrique secondaire :** p99 latence d'action (end-to-end, de la réception du message au retour de `emit()`), décomposée en deux termes indépendants :
- p99 temps d'attente dans le pool d'inférence (acquisition du sémaphore, queue depth)
- p99 temps d'exécution post-acquisition (introspect + infer + commit_barrier + emit, sans attente pool)

**Borne de latence :** p99 ≤ 2 secondes (compatible W1 : inférence 2,5s + overhead < 2s toléré).

**Borne de débit :** R_actif ≥ 2× pour valider H-densité-active.

**Justification du ratio 2× et non 5× :** Au-delà de k slots d'inférence simultanés, le pool d'inférence sérialise le débit. Une fois saturé, le débit est borné par la capacité du pool (tokens/s du backend d'inférence), indépendamment du substrat runtime (Wasmtime, Docker, ou autre). Le ratio atteignable entre Wasmtime et Docker sans saturer le pool dépend de :
- La latence de commit_barrier : Wasmtime avantage par design (H-commit-barrier, overhead ≤ 500µs) vs Docker (syscall, FS I/O, overhead ~ms).
- La latence de scheduling : Wasmtime avantage par structure (Tokio tasks ~400 bytes stack vs container processes ~MB).
- Une fois saturé, les deux convergent vers le même débit limité par le backend d'inférence seul.

Atteindre 2× sur la dimension active sans saturation du pool est défendable ; atteindre 5× suppose une surdimen sion du pool ou un workload non saturant, ce qui n'est pas réaliste en production.

**Baseline spécification :** Docker baseline active = container Python 3.11 exécutant le même cycle W1 (message reception → introspect → sleep 2,5s → commit → emit) en mode actif (pas idle). La baseline doit avoir accès à la même capacité d'inférence : même nombre de slots disponibles.

**Condition de réfutation :** Le ratio Wasmtime/Docker (débit actif) est < 2× sur hardware de référence, ou p99 latence d'action dépasse 2 secondes.

**Plan B :** Si H-densité-active est réfutée :
- Mesurer le bottleneck : commit_barrier overhead vs pool contention vs scheduler latency. Chacun implique un plan d'action différent.
- Réévaluer le ratio cible : un gain de 1,5× au lieu de 2× peut suffire si le bottleneck identifié (ex: pool d'inférence, pas substrat) est indépendant du runtime.
- Considérer une architecture avec pool d'inférence à latence bornée par design (ex: GPU avec QoS, CPU affinity) pour démontrer la limite théorique.

**Références :**
- `spec/02-properties.md` §P1b — densité active (propriété cible)
- `spec/04-hypotheses.md` §H-inférence-coût — interaction avec la capacité d'inférence
- `benchmarks/reference-workload.md` §W1 — définition du workload de mesure (idle_fraction)
- ADR-0002 — choix Wasmtime
- ADR-0006 ou successeur — modèle de supervision et allocation de capacité d'inférence

**Résultat empirique (lab Phase 5, 2026-05-16, en attente de T6-actif):** **Non encore mesurée** (comparaison Docker). Cette hypothèse requiert un benchmark `t6-active` qui reproduise N agents W1 en charge active (500 KB/s débit d'émission, cycle complet, pool d'inférence saturé ou non selon N). Le benchmark n'existe pas à cette date ; il est déclencheur d'une qualification de phase ultérieure (T6.5 ou Phase 6). Voir `benchmarks/test-protocol.md` §6.2.

**Note Phase 10 (2026-05-30) :** P10-S3 et P10-S5 ont validé la mécanique du scheduler (équité E1, priorité E3, coordination C1↔C2) sous OllamaBackend réel avec 6 workers et cap=2, overhead scheduler ≈ 0. Ce n'est pas la comparaison Docker de H-densité-active (pas de baseline Docker en charge active) mais confirme que le scheduler n'introduit pas de surcoût mesurable. La qualification T6-actif reste à produire sur hardware représentatif. Voir `poc/scenarios/P10-S3/VERDICT.md`.

> **Note T6-qualif (2026-05-16) :** H-densité a été séparée en deux hypothèses distinctes (H-densité-hébergée et H-densité-active) selon les décisions validées du 2026-05-16. Chacune repose sur une métrique d'isolation et une baseline propres. H-densité-hébergée (P1a) est partiellement validée sur machine de développement ; H-densité-active (P1b) est en attente de qualification sur W1 en charge active. Le paramètre `idle_fraction = 0.5` est une convention provisoire ; il sera réévalué quand des profils d'agents LLM réels sont disponibles.

---

### H-wake-latence — La latence de livraison à un agent dormant est bornée sous charge nominale

**Identifiant :** H-wake-latence

**Énoncé :** La latence de bout en bout d'une livraison de message à un agent dormant via `Scheduler::deliver` — chemin complet C2 (IoAdmissionQueue) → `wake_agent` (ContentStore restore) → `send` — est bornée par un budget T_wake à définir par mesure, en régime de charge nominale (cap_io actif, fraction d'agents dormants > 20 %, charge I/O representative de W1). Cette borne est le critère de déclenchement de l'option A (admission prédictive, ADR-0031 §FutureWork) : tant que p99 deliver ≤ T_wake, l'option B (lazy wakeup, implémentée) est suffisante ; au-delà, l'option A devient pertinente.

**Criticité :** Secondaire — l'option B (ADR-0031) est fonctionnelle et correcte ; H-wake-latence détermine seulement à quel moment l'optimisation prédictive vaut son coût de complexité additionnelle.

**Domaine :** Performance du scheduler / cycle evict-wake.

**Condition de réfutation :** La latence p99 de deliver à un agent dormant, mesurée sous charge réelle (N agents, cap_io actif, fraction dormants > 20 %), dépasse T_wake = 10 ms de manière reproductible sur K ≥ 3 runs.

**Plan B :** Si H-wake-latence est réfutée (p99 deliver > T_wake sous charge), implémenter l'option A (ADR-0031 §FutureWork) : le `SchedulerCoordinator` pré-charge les agents dormants les plus probablement sollicités avant que leur premier message arrive, en utilisant le `cache_score` et la distribution historique des inter-arrival times. Coût : complexité additionnelle dans le scheduler + risque de pré-charge inutile sur des agents silencieux.

**État :** **Première mesure effectuée — T7 (2026-05-25).** T_wake = **311 µs (p99)**, p50 = 204 µs. N=50 agents, N_dormant=20, CAP_IO=3, K=3 runs, 60 samples, 0 erreur. État AGENT_WAT (64 KiB, cache chaud). T_wake fixé à **10 ms** (×32 de marge sur le mesuré, pour couvrir état W1 réel + cache froid). L'option B (lazy wakeup) est suffisante : 311 µs représente 0.006 % d'un cycle de 5 s — l'option A n'est pas pertinente dans ce régime. Limite : état minimal + cache chaud ; validation W1 + charge réelle reste à produire.

**Références :**
- `decisions/0031-scheduler-coordinator-reveil-a-la-demande.md` §FutureWork — option A différée
- `poc/scenarios/S12-scheduler-coordinator/` — scénario de base (N=8, charge légère)
- `poc/results/T7/wake/SYNTHESE.md` — résultats T7 (première mesure T_wake)
- `spec/02-properties.md §P1b` — densité active (bénéficiaire principal de l'option A)

---

## 5. Plan d'invalidation

### 5.1 Pour chaque hypothèse bloquante : quel prototype minimal la teste

| Hypothèse | Prototype minimal | Critère de validation | Résultat |
|-----------|------------------|----------------------|----------|
| H-profil-B | Collecte de métriques sur un déploiement réel (durée de vie, volume d'actions par agent) | > 80% des agents avec durée < 10 min ou < 10³ actions → hypothèse réfutée | **Non testée** — aucun déploiement réel disponible à ce stade |
| H-revoke | Benchmark de révocation sur arbre synthétique de N capabilities, N = 10³ à 10⁶ | Coût CPU < 5% sous charge W1 ; TTL 60s–300s suffisant pour propagation | **Partiellement validée** (lab phase 4–5, 2026-05-13) — mécanisme fonctionnel confirmé (P4.1–P4.5) ; `check()` p99=361ns ✓ ; `revoke()` < 1ms jusqu'à N≈10K, 20ms à N=100K (critère 5% CPU probablement tenu, plafond O(N) documenté L21) ; TTL inter-nœuds non testé |
| H-causal-latence | `cargo bench --bench causal_lookup` avec `BENCH_N=100000000` sur NVMe ≥ 1 GB/s | p99 > 10ms → hypothèse réfutée | **Indicatif** (lab phase 5, 2026-05-13) — N=10⁶ régime cache chaud : p99 = 11µs (×900 sous la cible) ; N=10⁸ régime cache miss réel en attente (voir `benchmarks/test-protocol.md` §6.1) |
| H-commit-barrier | SEF-4 (crash pendant transaction) + mesure overhead N agents W1 | Trou dans l'interception WASI ou overhead > 5% CPU | **Indicatif** (Phase 5, 2026-05-13) — H-cb-correct ✓ structurel (WAT call graph) ; H-cb-overhead ✓ 11 µs/cycle = 0.0002% W1 (marge 25 000×) ; régime cache chaud, 1 run (voir L22) |
| H-densité-hébergée | T6 : overhead par agent Wasmtime idle vs Docker réaliste (Python runtime) | Overhead Wasmtime idle ≥ 5× inférieur à Docker Python LLM idle (R_idle ≥ 5×) | **Partiellement validée** (lab Phase 5, 2026-05-14) — W1 révisé (état dans ContentStore) : Wasmtime 5 KB vs Docker Python LLM 43 314 KB → ratio **8 670×** (cible ≥ 5× : ×1 500 au-delà). 1 hardware, 1 run, N=10 containers. Voir §L27 et §L28. |
| H-densité-active | T6-actif (non implémentée) : débit d'actions W1 sous charge active (idle_fraction=0.5) | Débit Wasmtime/Tokio ≥ 2× Docker Python LLM (R_actif ≥ 2×) ; p99 latence ≤ 2s | **Non encore mesurée.** Requiert benchmark t6-active : N agents W1 en charge active, cycle complet avec pool d'inférence. Déclencheur : après T5 (causal-log + rollback qualifiés) et T6-hébergée confirmée. Voir `benchmarks/test-protocol.md` §6.2. |
| H-rollback-latence | `cargo bench --bench rollback_latency -p os-poc-store` sur DB à froid (dataset >> block cache) | p95 > 100ms sur W2/depth=100 → hypothèse réfutée | **Indicatif** (lab phase 5, 2026-05-13) — régime cache chaud : p95=99µs sur W2/depth=100 (×1000 sous la cible) ; rollback independant de la taille des blocs confirmé ; qualification à froid en attente |
| H-inférence-coût | Mesure du temps bout-en-bout pour des chaînes de 1, 5, 10, 20 appels LLM séquentiels sur hardware de prod cible | Chaîne de 10 appels < 500ms → hypothèse réfutée ; > 30s → hypothèse confirmée | **Confirmée** (lab phase 2, 2026-05-12) — 130s sur CPU |
| H-mémoire-schema | Lab phase 2 : 2 agents avec system prompts identiques accèdent au même store et écrivent le "nom de famille de l'utilisateur" — compter les clés distinctes | > 1 clé distincte utilisée pour le même concept dans > 50% des runs → hypothèse confirmée | **Confirmée** (lab phase 2, 2026-05-12) — `user.family.last_name` vs `user.family.name` |
| H-wake-latence | T7 (2026-05-25) : N=50, N_dormant=20, CAP_IO=3, K=3 | p99 deliver > 10 ms → option A (admission prédictive) justifiée | **Indicatif.** T_wake mesuré = 311 µs (p99), p50 = 204 µs — très en dessous du budget 10 ms. Option A non pertinente. Limite : état AGENT_WAT + cache chaud. |

### 5.2 Critères d'abandon du projet si une hypothèse centrale tombe

La thèse centrale est : *un OS peut rendre les agents IA longs-courriers auditables, réversibles et confinés, sans coût d'infrastructure prohibitif sur leurs opérations courantes.*

La plupart des réfutations d'hypothèses appellent un Plan B, pas un abandon. L'abandon est justifié quand le Plan B lui-même repose sur une hypothèse qui tombe.

**Révision majeure requise (pas abandon) :**

| Hypothèse réfutée | Conséquence | Plan de repli |
|-------------------|-------------|---------------|
| H-profil-B seule | Le design est optimisé pour le mauvais profil | Pivoter vers le profil C (agents éphémères) : spawn ultra-rapide sans log causal persistant, P2 opt-in |
| H-causal-latence seule | P3 ne tient pas sur RocksDB | Changer de substrate ou adopter un log échantillonné |
| H-revoke à l'échelle | P4 trop coûteuse en CPU | Basculer vers revocable forwarders (Plan B documenté) |

**Abandon justifié :**

Deux conditions cumulatives invalident la thèse sans repli architectural connu :

1. **H-profil-B réfutée ET H-supervision réfutée simultanément.** Si les agents sont massivement courts (< 10 min) *et* que la supervision requiert une interaction temps réel, alors le modèle de valeur du système disparaît : le log causal n'a pas le temps d'accumuler de l'information utile, et la supervision asynchrone ne répond pas au besoin réel. Le système devient une infrastructure de surveillance coûteuse pour des agents qui n'en bénéficient pas.

2. **P7 (overhead OS) impossible à satisfaire par aucun substrate connu.** Si la mesure sur workloads représentatifs (W1, W2, W3) montre que le coût du log causal + capabilities + rollback dépasse systématiquement le budget alloué, et qu'aucune combinaison substrate / architecture (RocksDB, WAL court-circuit, log échantillonné) ne passe en dessous du seuil — alors le problème est fondamental, pas une question d'implémentation.

**Ce qui n'est pas un critère d'abandon :**

- H-causal-latence réfutée seule : RocksDB n'est pas le seul LSM tree. Le substrate est substituable.
- H-mémoire-schema ou H-inférence-coût confirmées : ce sont des contraintes qui informent le design, pas des réfutations de la thèse.
- Des résultats de performance décevants sur hardware de développement : la qualification se fait sur hardware de référence (NVMe, RAM suffisante).
