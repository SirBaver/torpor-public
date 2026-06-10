# Workloads de référence

## Rôle de ce document

Les workloads de référence servent à deux fins distinctes :

1. **Évaluation des propriétés cibles.** Les propriétés P1 (densité), P2 (rollback), et P3 (traçabilité causale) définies dans `spec/02-properties.md` sont toutes formulées relativement à un workload de référence. Un workload de référence sans la description précise de W1 est une propriété non mesurable.

2. **Baseline de comparaison.** La thèse centrale du projet est falsifiable : le système doit démontrer un avantage quantifié sur Linux+containers sur ces workloads spécifiques. Sans workload précis, la comparaison est arbitraire.

Les workloads sont définis de manière à être **reproductibles** : toute équipe disposant du matériel spécifié doit pouvoir les reproduire indépendamment et obtenir des résultats comparables.

---

## Définitions des workloads

### W1 — Micro (agent LLM à état externalisé)

**Description :** Agent qui exécute en boucle le cycle suivant, simulant le profil d'un agent LLM asynchrone dont l'état canonique est dans le ContentStore :

1. **Réveil & lecture** : lecture d'une instruction depuis sa boîte aux lettres (attente bloquante si la boîte est vide).
2. **Inférence simulée** : `sleep` calibré sur le hardware cible + allocation d'une mémoire de travail volatile de **2 MB** (buffers de tokens, état JIT Wasmtime). L'inférence n'est pas exécutée nativement — elle est simulée pour isoler le coût d'infrastructure.
3. **Persistance d'état** : lecture/écriture d'un bloc de **50 MB** représentant le contexte long-terme de l'agent (historique, documents chargés, mémoire épisodique).
   - **Système OS-IA** : ce bloc est un objet dans le ContentStore (RocksDB). L'accès passe par le block cache global partagé entre N acteurs (256 MB). L'agent n'alloue pas ces 50 MB en WASM linear memory — il détient un pointeur de contenu (hash).
   - **Baseline Linux+Docker** : ce bloc est maintenu dans le RSS du container (mémoire résidente), ou chargé depuis un volume Docker sans mécanisme de cache partagé entre containers.
4. **Émission** : appel `emit()` → `commit_barrier` → écriture durable dans le log causal. Le WASM linear memory (tampon de travail) peut être désalloué après cette étape.
5. **Sommeil** : l'agent retourne en état idle en attendant le prochain trigger.

**Paramètres :**

| Paramètre | Valeur | Note |
|-----------|--------|------|
| Mémoire de travail volatile (WASM linear memory) | 2 MB | non persistée entre actions |
| État long-terme par agent (dans ContentStore) | 50 MB | partagé via block cache RocksDB |
| Fréquence de traitement | 1 instruction toutes les 5 secondes | |
| Durée de l'inférence simulée | à calibrer sur le hardware cible | profil 3B ou 7B |
| Durée de la mesure | 1 heure | |
| **`idle_fraction`** | **0,50** (défaut) | fraction du cycle où l'acteur est en état idle (entre deux inférences) ; paramètre variable pour explorer le continuum actif↔dormant |

**`idle_fraction` : paramètre clé de H-densité.** L'avantage Wasmtime se manifeste intégralement à `idle_fraction = 1,0` (agents dormants, 5 KB vs 37–42 MB Docker Python) et s'atténue à `idle_fraction = 0` (agents constamment actifs, coûts qui convergent). Le rapport de densité Wasmtime/Docker varie selon ce paramètre. Pour une comparaison honnête, `idle_fraction` doit être justifié par des mesures d'usage réel (profil d'un agent LLM en production : attente LLM ~2–18s, traitement ~0,1s → `idle_fraction` ≈ 0,90–0,99).

**Statut du paramètre « État long-terme par agent » (50 MB) — clarification revue externe §6.4 (2026-06-10) :** c'est un **objectif de design** (hypothèse de modélisation du profil B), pas une mesure. Aucun corpus d'agents réels n'existe pour le mesurer ; aucun scénario ne l'a exercé en bout-en-bout (S10 lit une entrée ContentStore, pas 50 MB ; le calibrage evict/wake est FutureWork, ADR-0030).

**Conséquence sur le cap actif C2.** Dans `cap_actif = floor(BW_NVMe / 50 MB)`, la bande passante est **mesurée** (T5, 741 MB/s) et seul le numérateur d'état est hypothétique : le cap actif ne porte donc **qu'une** variable non mesurée, pas deux. (La crainte « deux hypothèses empilées » de la revue visait l'arithmétique serveur PCIe Gen4, abandonnée le 2026-05-27 — cf. `spec/07 §3`.) Le « 14 agents/s » est **conditionnel à l'hypothèse de 50 MB** et décroît en `1/état` : un état réel plus grand abaisse le cap, un état plus petit le relève. À titre purement illustratif et non mesuré, un état de 100 MB ramènerait le calcul à ~7 agents/s.

**Atténuations.** (i) `cap_actif` est un **paramètre de configuration recalibrable** (ADR-0030), non une constante compilée : une mesure future ajuste le paramètre sans toucher au design. (ii) Le coût agrégé réel est **borné supérieurement** par `50 MB × N` — le partage de templates via le Merkle DAG le réduit quand les agents partagent un état de base.

**Communication.** Toute publication du cap doit porter la réserve « conditionnel à l'hypothèse 50 MB/agent ».

**Régimes de mesure pour P1 (densité) :**

| Régime | Définition | Ce qui est mesuré |
|--------|------------|-------------------|
| **Actif** | 10 % des agents exécutent l'étape 2 simultanément | Overhead isolation (Wasmtime vs Docker) sous charge CPU/RAM |
| **Idle** | 90 % des agents attendent un trigger | Efficacité de la gestion d'état : WASM pages désallouées (5 KB/acteur) vs runtime Python résident (~150 MB/container) |

**Baseline Docker réaliste :** container Python 3.11 + dépendances agent LLM (LangChain ou équivalent), pas Alpine. L'overhead mesuré doit inclure le RSS du runtime applicatif en état idle (~100–200 MB), car c'est ce qu'un déploiement agent réel consomme.

**Ce que W1 mesure :** Le coût d'infrastructure pur de N agents simultanés — scheduling, isolation, messagerie inter-acteur, commit barrier, cache de l'état externalisé. W1 ne mesure pas les performances d'inférence LLM.

**Justification de la séparation état volatile / état persistant :** Dans l'architecture de ce projet, l'état canonique de l'agent est écrit dans le ContentStore à chaque `commit_barrier`, avant tout `emit`. Le WASM linear memory est un tampon temporaire, pas l'état persistant. Cette propriété permet à un OS qui connaît le cycle de vie des acteurs de désallouer agressivement les pages WASM après `emit()` et de les re-faulter au prochain message — l'overhead RAM d'un acteur inactif tend vers 0. Docker ne peut pas offrir l'équivalent sans couche applicative supplémentaire : le runtime Python garde ses objets en mémoire indépendamment de l'activité de l'agent.

---

### W1-access — Modèles d'accès au log causal (Q2)

**Contexte :** sans modèle d'accès déclaré, on ne peut pas dimensionner le block cache (C2), écrire T5-multi-tenant (P3c), ni calibrer BlobDB (Q3). Aucune donnée de production n'étant disponible, deux modèles sont déclarés explicitement.

**Décision Q2 (2026-05-16) — convention de référence :**

- **Modèle B (recency-biased)** est adopté comme **convention de référence ferme** pour le dimensionnement du block cache (C2), des bornes P3b/P3c, du seuil de saturation scheduler Phase 6, et de tout benchmark visant à représenter un régime de production réaliste. C'est la convention par défaut quand un calcul de dimensionnement nécessite un modèle d'accès.
- **Modèle A (no-locality)** est conservé comme **borne supérieure de coût** (worst-case adversaire). Tout cap conservateur (cap_actif C2 jusqu'au passage Phase 6, dimensionnement d'admission control en absence de profil applicatif) reste calculé sur Modèle A.
- La vraie p99 production se situe entre les deux ; aucun des deux ne ment seul. Réévaluation conditionnelle : si un W2 réel mesuré (Phase 6+) révèle une distribution d'accès écartée de Modèle B (par exemple K observé > 500 ou recouvrement < 2 %), la convention sera amendée par un ADR.

**Statut :** convention ferme (2026-05-16) — Q2 résolue dans `TODO.md`. Réévaluation conditionnelle au premier W2 réel.

#### Modèle A — borne supérieure (« no-locality »)

Chaque agent peut lire n'importe quelle entrée de son historique causal avec probabilité uniforme. Hot set par agent = historique causal complet. Recouvrement inter-agent = 0 % (isolation forte). C'est ce que mesure `populate_synthetic` aujourd'hui avec ses lookups uniformes — c'est la borne supérieure du coût.

| Paramètre | Valeur |
|---|---|
| Distribution des accès | uniforme sur les N entrées de l'agent |
| Recouvrement inter-agent | 0 % |
| Hit ratio block cache attendu | faible (dataset >> cache) |
| Usage | borne supérieure du coût — T5 actuel mesure ce régime |
| `workload.json.access_pattern` | `uniform` |

Conséquence pour C2 : `cap_actif = floor(BW_NVMe / 50 MB)` dimensionné sur worst-case cache-miss. Avec 741 MB/s mesuré (borne basse instance B) : **14 agents/s** (valeur en vigueur dans `spec/07 §C2`).

#### Modèle B — convention de référence (« recency-biased »)

Distribution exponentielle sur l'âge causal : la majorité des lookups portent sur les K dernières actions, avec une queue exponentielle décroissante sur l'historique profond. Recouvrement inter-agent via références causales parent→enfant et merge.

| Paramètre | Valeur de convention | Justification |
|---|---|---|
| Distribution des accès | exponentielle, `P(action_i) ∝ exp(-age_i / K)` avec **K = 128 actions** | mémoire de travail LLM courte (LangChain, CAMEL, AutoGPT) ; chaînage parent→enfant dominant |
| Hot set par agent | **128 actions récentes** (K) couvrent ≥ 90 % des lookups | borné par ADR-0012 (sessions 10K actions max) |
| Recouvrement inter-agent | **10 %** des `action_id` référencés par ≥ 2 agents | références causales communes (spawn parent, validation supervisor, merge) |
| Hit ratio block cache attendu | élevé (accès récents + recouvrement = cache-friendly) | |
| Usage | dimensionnement par défaut C2, P3b, P3c, scheduler Phase 6 |  |
| `workload.json.access_pattern` | `recency` (paramètre dérivé : `recency_k=128`, `cross_agent_overlap=0.10`) | |

**Conséquences chiffrées (à confirmer empiriquement par T5-bis) :**
- Block cache RocksDB : 256 MB (ADR-0011) couvre ~2,5 M entrées chaudes (~100 B/entrée). À 128 actions × 1000 agents × 100 B = 12,8 MB, le hot set tient dans 5 % du cache → hit ratio attendu > 95 %.
- `cap_actif` C2 sous Modèle B : dimensionné sur le hit ratio. Estimation provisoire 50–100 agents/s, à mesurer par T5-bis cache-friendly. Tant que cette mesure n'est pas faite, **le cap publié reste celui de Modèle A** (14 agents/s, conservateur).

**Justification des valeurs de convention :**
1. **K = 128** : compromis entre (a) la fenêtre de contexte LLM 128K–200K tokens (~256–400 actions visibles, cf. C3) et (b) la borne de session 10K actions (ADR-0012). K = 128 capture la mémoire de travail effective sans prétendre couvrir l'historique de session. Valeur ronde, alignée sur des puissances de 2.
2. **Recouvrement 10 %** : ordre de grandeur observable dans des systèmes multi-agents avec délégation hiérarchique (spawn parent + N validations supervisor + quelques merges). 0 % est irréaliste sous ADR-0008 (spawn_child) et ADR-0013 (supervision). 20–30 % serait optimiste sans données.

#### Mise en œuvre dans le benchmark

`populate_synthetic` doit exposer `--access-pattern=uniform|recency` avec paramètres `--recency-k=N` (défaut 128) et `--cross-agent-overlap=X` (défaut 0.10). Reporter p50/p95/p99/p99.9 sous chaque pattern dans `verdict.json.metrics`. La différence A vs B mesure la sensibilité du système à la locality d'accès et calibre l'admission control C2.

---

### emit-payload-distribution — Distribution de convention des tailles d'`emit_payload` (Q3)

**Contexte :** `populate_synthetic` génère des entrées de taille fixe (~100 bytes par L18). T5 actuel est structurellement incapable de produire un histogramme `emit_payload_size_distribution` informatif — la distribution est dégénérée par construction. Une distribution de convention est adoptée pour débloquer ADR-0017 (calibration `min_blob_size`) sans attendre le premier workload W2 réel.

**Décision Q3 (2026-05-16) — convention ferme :** La distribution composite ci-dessous (p50=256 B, p90=4 KB, p95=8 KB, p99=32 KB, max=64 KB) est adoptée comme **convention de référence ferme** pour tout dimensionnement Phase 6 dépendant de la taille de payload : seuil BlobDB `min_blob_size = 4 KB` (ADR-0017 §3bis confirmé), provisioning SST par CF `default`, dimensionnement du block cache pour les entrées inline. Réévaluation conditionnelle : si un W2 réel mesuré (Phase 6+) révèle un p90 écarté de plus d'un facteur 2× de 4 KB, le seuil BlobDB sera ajusté par un amendement à ADR-0017 (le changement est transparent — pas de migration de schéma, cf. ADR-0017 §3bis). En l'absence d'écart majeur, la convention tient.

**Statut :** convention ferme (2026-05-16) — Q3 résolue dans `TODO.md`. Réévaluation conditionnelle au premier W2 réel avec critère explicite (facteur 2× sur p90).

#### Distribution de convention

Basée sur les types d'`EmitType` (ADR-0010) et les payloads typiques d'agents LLM :

| Type d'action | Taille typique | Source de l'estimation |
|---|---|---|
| `Spawned`, `Active`, `Suspended`, `Terminated` | 64–128 B | métadonnées d'état, formats fixes |
| `Introspect` | ~73 B | format binaire fixe documenté (L29) |
| `SelfRollback`, `SchedulerRollback` | 16–32 B | distance + target_seq + flags |
| `ValidationRequest` / `ValidationResponse` | 32–64 B | risk_level + verdict |
| `SessionBoundary` | 256–1 024 B | résumé causal court |
| Tool call args (custom) | 256–2 048 B | typique LLM function-call JSON |
| Memory write payload | 512–4 096 B | observation, fait extrait |
| Web/file observation | 4 KB–64 KB | contenu indexé partiel |

Distribution composite résultante (mix de types attendu) :

```
p50  ≈    256 B
p90  ≈  4 096 B  ← seuil BlobDB provisoire (ADR-0017 §3bis)
p95  ≈  8 192 B
p99  ≈ 32 768 B
max  ≈ 65 536 B
```

#### Conséquences

- **ADR-0017 `min_blob_size` provisoire :** 4 KB (p90 de cette distribution). 10 % des entrées sortent via BlobDB, 90 % restent inline en SST.
- **T5 et calibration BlobDB :** séparés délibérément. T5 mesure la latence du lookup causal à payload constant ; la calibration BlobDB se fera avec un benchmark dédié W2 utilisant cette distribution de convention, ou avec des données de production réelles en Phase 3.

---

### W2 — Medium (agent avec mémoire et rollback)

**Description :** Agent W1 augmenté de trois capacités supplémentaires :

1. **Index de connaissance vectoriel** : l'agent maintient un index de 10 000 embeddings (dimension à préciser selon le modèle de référence) et effectue des recherches kNN sur cet index à chaque instruction traitée.
2. **Journal d'actions structuré** : l'agent persiste un journal structuré de chaque action (timestamp, type, payload sommaire, résultat).
3. **Capacité de rollback** : l'agent peut être soumis à un rollback sur les 100 dernières actions. La propriété P2 est évaluée sur W2.

**Paramètres :**

| Paramètre | Valeur |
|-----------|--------|
| État résident par agent | 500 MB |
| Taille de l'index vectoriel | 10 000 embeddings |
| Profondeur de rollback garantie | 100 actions |
| Fréquence de traitement | 1 instruction toutes les 5 secondes |
| Durée de la mesure | 1 heure |

**Ce que W2 mesure :** Le coût marginal de l'index vectoriel et du journal sur la densité ; la latence et la correction du rollback sur 100 actions ; la cohérence du store sous charge soutenue.

---

### W3 — Macro (agent avec sous-agents et outils externes)

**Description :** Agent W2 augmenté de trois capacités supplémentaires :

1. **Intégration avec un système de fichiers projet** : l'agent lit et écrit dans un corpus d'environ 1 000 fichiers texte (taille totale à préciser), représentant un projet de code ou de documentation.
2. **Invocation d'outils externes simulés** : 5 outils distincts avec des latences calibrées (à préciser selon les cas d'usage réels visés — appels réseau, compilateurs, indexeurs).
3. **Spawning de sous-agents** : l'agent spawne typiquement 10 sous-agents simultanés avec des capabilities restreintes (sous-ensemble de ses propres capabilities). Ces sous-agents exécutent des tâches déléguées et renvoient des résultats.

**Paramètres :**

| Paramètre | Valeur |
|-----------|--------|
| État résident par agent (parent + sous-agents) | 2 GB |
| Nombre de fichiers projet | ~1 000 |
| Nombre d'outils externes simulés | 5 |
| Nombre de sous-agents simultanés | 10 |
| Fréquence de traitement | à préciser (workload asymétrique) |
| Durée de la mesure | 1 heure |

**Ce que W3 mesure :** Le coût du spawning d'agents ; la correction de la délégation de capabilities et de leur révocation ; la latence d'accès au système de fichiers projet ; l'overhead de l'orchestration de sous-agents.

---

## Métriques rapportées

Pour chaque workload (W1, W2, W3), les métriques suivantes sont rapportées :

| Métrique | Unité | Description |
|----------|-------|-------------|
| RAM moyenne par agent | MB | Mémoire résidente moyenne pendant la mesure |
| Latence d'action p50 | ms | Médiane du temps de traitement d'une action |
| Latence d'action p99 | ms | 99e percentile du temps de traitement d'une action |
| Débit total | agents·actions/s/machine | Nombre total d'actions traitées par seconde par toutes les instances actives, rapporté à la machine |
| Coût d'un rollback de 100 actions | ms | Temps de rollback de la dernière action à la confirmation de restauration de l'état (SEF-2) |
| Coût de spawn d'un agent supplémentaire | ms | Temps entre la commande de spawn et la première action traitée par le sous-agent |

---

## Spécification de la baseline

**Composition de la baseline :** Linux 6.x + Docker 24.x + orchestrateur (Kubernetes ou Nomad) sur le même hardware physique que le système sous test.

**Principes de configuration :**

- La baseline est **raisonnablement tunée** — ce n'est pas un strawman. Les limites de ressources (cgroups, memory limits), les options de scheduling, et les paramètres réseau sont configurés selon les recommandations des documentations officielles pour une charge de type "agent IA".
- La configuration de la baseline est **documentée et reproductible** (fichiers de configuration versionnés dans ce dépôt).
- Si la baseline requiert une couche applicative supplémentaire pour passer un scénario d'équivalence fonctionnelle (par exemple CRIU pour SEF-2, AppArmor/seccomp pour SEF-3), cette couche est incluse et son overhead est comptabilisé dans les métriques.

**Référence hardware :** La classe de machine de référence est spécifiée de manière cloud-agnostic pour garantir la reproductibilité sans hardware dédié :

| Composant | Spécification minimale | Équivalent cloud indicatif |
|-----------|----------------------|---------------------------|
| CPU | x86-64, ≥ 8 cœurs physiques, ≥ 3,5 GHz base clock | AWS c6i.2xlarge, GCP c2-standard-8 |
| RAM | 16 GB DDR4, ≥ 3 200 MHz | (inclus dans les instances ci-dessus) |
| Stockage | NVMe SSD local, ≥ 1 GB/s écriture séquentielle | Instance store NVMe ou SSD attaché dédié |
| Réseau | ≥ 10 Gbps (pour les tests multi-nœuds, optionnel pour W1/W2) | (inclus dans les instances ci-dessus) |

Le choix d'une instance compute-optimized 8-cœurs / 16 GB est motivé par : suffisamment de cœurs pour mesurer la densité d'agents sous parallélisme réel, sans être assez grand pour rendre la thèse de densité 5x triviale à atteindre (une machine avec 512 GB de RAM rendrait P1 évident).

La configuration logicielle de la baseline (fichiers cgroups, limites Docker, paramètres Kubernetes) est versionnée dans `benchmarks/baseline-config/` (à créer lors de l'implémentation).
