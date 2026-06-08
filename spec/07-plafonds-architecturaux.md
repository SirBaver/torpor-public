# 07 — Plafonds architecturaux

## 1. Nature des plafonds

Un plafond architectural est une limite structurelle découverte par l'analyse ou le prototypage, distincte des hypothèses (affirmations non encore testées) et des non-objectifs (périmètre délibérément exclu). Un plafond est une contrainte observable du domaine physique ou algorithmique qui borne la portée d'une propriété, même si toutes les hypothèses sont satisfaites.

Ces plafonds ne réfutent pas la thèse centrale — ils la précisent. Ils signalent les directions que le design doit anticiper avant d'atteindre les régimes de production.

**Identification :** Les trois plafonds décrits ici ont été identifiés lors de l'analyse post-T6 (2026-05-14), après la validation de H-densité-hébergée sur le modèle W1 révisé (état externalisé dans ContentStore). Ils résultent du fait qu'une fois les contraintes de RAM hébergée résolues, d'autres goulots d'étranglement deviennent dominants — notamment la capacité d'inférence bornée (H-densité-active).

---

## 2. C1 — Mur de l'inférence

**Identifiant :** C1

### 2.1 Description

La densité active d'agents (P1b) est bornée par les slots d'inférence GPU/CPU disponibles, non par la RAM hébergée (P1a). Une fois l'état idle géré via ContentStore (W1 révisé, H-densité-hébergée), la RAM cesse d'être le facteur limitant. Le scheduler confronte alors le **mur de l'inférence** : N agents peuvent résider en mémoire simultanément, mais un seul pool de slots d'inférence peut servir k agents à la fois, où k est déterminé par la VRAM et le parallélisme du modèle.

### 2.2 Analyse quantitative

Paramètres de référence (profil W1, modèle qwen2.5:7b sur GPU 24 GB) :

| Paramètre | Valeur |
|-----------|--------|
| Durée d'inférence par action | ~2,5 s (mesuré en lab, L9) |
| Durée totale d'un cycle W1 | ~5 s |
| Fraction active (inférant) | 50 % |
| Slots d'inférence parallèles (GPU 24 GB) | ~4–8 requêtes simultanées |
| Agents pouvant résider en RAM (16 GB) | ~3 000 (5 KB overhead WASM, W1 révisé) |

Débit d'inférence effectif avec k=8 slots :

```
agents_inférant_simultanément = k = 8
débit = k / t_inférence = 8 / 2,5 s = 3,2 actions/seconde
agents_actifs_soutenables = débit × t_cycle = 3,2 × 5 = 16
```

La densité active est donc bornée à ~16 agents simultanément actifs, indépendamment du fait que 3 000 agents résident en RAM.

### 2.3 Implications architecturales

Le scheduler de Phase 3+ doit distinguer deux ressources :

1. **RAM hébergée** — managed by ContentStore + WASM CoW. Capacité : ~10³–10⁶ agents/machine. Mesurée (T6, L27–L28).
2. **Slots d'inférence** — managed by inference scheduler. Capacité : ~4–100/machine selon GPU. Implémenté Phase 6 (ADR-0022/0023 — `InferenceQueue` avec priorité Supervisor > Foreground > Batch et garde-fou famine bornée) ; cycle evict/wake Phase 7 (ADR-0031).

Les agents en état `Active` mais en attente de réponse LLM doivent passer en `Suspended` (A4) pour libérer leur slot si le pool est saturé. Les agents `Suspended` reprennent quand un slot se libère.

**Implémenté (Phase 6, ADR-0022/0023) :** Gestionnaire de slots d'inférence — `InferenceQueue` : file d'attente avec priorité sémantique (Supervisor > Foreground > Batch), garde-fou de famine bornée (`max_wait_ms = 30 s`), dispatcher Tokio de fond. Ce composant est distinct du scheduler Tokio existant : il opère au niveau des requêtes LLM, pas des messages WASM. Cycle evict/wake des agents `Suspended` implémenté Phase 7 (ADR-0031, SchedulerCoordinator). Coordination C1↔C2 sous charge LLM réelle non encore mesurée (non-objectif Phase 7, voir §3.4).

**Ce plafond s'applique même sur bare metal.** La formulation H-densité-hébergée et H-densité-active séparent justement ces deux métriques de densité distinctes : (a) densité hébergée (agents en mémoire, P1a), (b) densité inférante/active (agents calculant simultanément, P1b). Elles sont gouvernées par des ressources différentes et doivent être messurées indépendamment.

### 2.4 État et références

| | |
|-|-|
| **État** | **Implémenté Phase 6** (`InferenceQueue`, ADR-0022/0023) ; evict/wake Phase 7 (ADR-0031). Exercé sous `SleepyBackend` (S3, S5). Coordination C1↔C2 sous backend réel non encore mesurée (Phase 10). |
| **Déclenche** | ~~Spécification du scheduler Phase 3~~ (soldé). Phase 10 : mesurer E1/E3/P1b sous backend réel (OllamaBackend). |
| **Références** | `decisions/0022-inference-queue.md`, `decisions/0023-inference-famine.md`, `decisions/0030-scheduler-unifie.md`, `decisions/0031-scheduler-coordinator.md` ; `poc/scenarios/S3-inference-cap/`, `poc/scenarios/S5-fairness-priority/` ; `spec/04-hypotheses.md §H-inférence-coût`, `§H-densité-active` |

---

## 3. C2 — Thundering Herd (saturation PCIe)

**Identifiant :** C2

### 3.1 Description

Si N agents idle se réveillent simultanément et que leur état (50 MB/agent dans ContentStore) n'est plus dans le block cache RocksDB, le bus PCIe entre SSD NVMe et RAM sature. Ce phénomène est distinct du "thundering herd" classique (contention CPU/mutex) — il est borné par la **bande passante I/O PCIe**, pas par la concurrence CPU.

### 3.2 Analyse quantitative

**Deux valeurs de débit — deux rôles distincts :**

| Paramètre | Valeur | Source | Rôle |
|-----------|--------|--------|------|
| État persisté par agent | 50 MB (ContentStore, W1 révisé) | design target W1 | base du calcul |
| BW NVMe mesuré, i3en.xlarge, fio QD=1 mono-thread (bs=1M) | 741–769 MB/s (instance B min : 741 ; instance A : 768) | T5 R1–R8, 2026-05-15 | borne basse — coût opération unitaire ; base du cap actif classe 1 |
| BW NVMe mesuré, i3en.xlarge, fio QD=32 multi-thread (bs=1M) | 678 MB/s (harness v3, instance B — R8) | T5 R8, 2026-05-15 | convergent avec QD=1 (2% d'écart) — ce NVMe ne scale pas avec la parallélisation |
| BW NVMe mesuré, AMD Ryzen 5 PRO / WD SN530, fio QD=1 (bs=1M) | 1 290–1 321 MB/s | T5 RA1–RA3, 2026-05-18 | 1,7× classe 1 ; base du cap actif classe 2 |
| BW NVMe mesuré, AMD / WD SN530, fio QD=32 (bs=1M) | 2 095–2 214 MB/s | T5 RA1–RA3, 2026-05-18 | scale avec QD (×1,7 vs QD=1) — NVMe consumer PCIe |
| IOPS rand read QD=1, AMD / WD SN530, fio (bs=4k) | 9 039–10 865 IOPS | T5 RA1–RA3, 2026-05-18 | **première mesure** — régime production RocksDB (blocs 4–16 KB) |
| IOPS rand read QD=32, AMD / WD SN530, fio (bs=4k) | 125 000–130 000 IOPS | T5 RA1–RA3, 2026-05-18 | **première mesure** — régime contention multi-agents |
| BW NVMe hypothèse hardware serveur PCIe Gen4 | ~5 GB/s (hypothèse constructeur) | non mesuré | arithmétique non poursuivie — qualification abandonnée 2026-05-27 (non transférable seL4) |
| Block cache RocksDB par machine | ~8 GB (configuration type, 64 GB RAM) | ADR-0011 | cache résidant |
| Agents dans le cache à tout instant | 160 (8 GB / 50 MB) | calculé | base admission control |

> **Note (2026-05-15, post-T5 R8) :** fio QD=1 min = 741 MB/s (instance B) ; fio QD=32 bs=1M = 678 MB/s (harness v3, R8). Convergence QD=1/QD=32 confirmée : ce NVMe sature sa bande passante dès QD=1 sans gain de parallélisation supplémentaire. Cap actif calculé sur 741 MB/s (borne basse conservatrice).
>
> **Limite méthodologique :** la bande passante séquentielle est adaptée au scénario ContentStore (chargement séquentiel de 50 MB/agent depuis NVMe). Elle n'est **pas** la bonne métrique pour prédire la latence des lookups RocksDB du log causal (P3a), qui sont des lectures aléatoires de blocs 4–16 KB. La métrique complémentaire manquante est `storage_rand_read_iops_qd1` (fio `--bs=4k --rw=randread`), absente du harness v3 — à ajouter en harness v4.

Scénario pire cas : 1 000 agents idle se réveillent simultanément, aucun état en cache.

```
données à lire = 1 000 × 50 MB = 50 GB

# Borne basse mesurée (768 MB/s QD=1, i3en.xlarge) :
temps = 50 GB / 768 MB/s ≈ 65 s

# Estimation capacité réelle i3en.xlarge multi-thread (~2,5 GB/s) :
temps = 50 GB / 2,5 GB/s ≈ 20 s  ← à confirmer par re-run fio QD=32

# Illustration — hardware serveur rapide hypothétique (~5 GB/s, non mesuré) :
# (qualification serveur abandonnée 2026-05-27 — voir §3.3 ; chiffre purement illustratif)
temps = 50 GB / 5 GB/s ≈ 10 s
```

**Dans tous les cas, violation sévère sans admission control.** La borne de l'atténuation nécessaire dépend du substrat de stockage effectif ; elle n'est pas figée tant que la stack seL4-native n'est pas prototypée.

**Atténuation naturelle (Merkle DAG) :** ContentStore est content-addressed. Des agents partageant un template de base commun partagent leurs blocs. Dans ce cas, le coût réel est inférieur à 50 MB × N. Le pire cas reste valide pour des agents à états entièrement divergents.

### 3.3 Mitigation : I/O Admission Control — cap actif (borne de référence)

Une queue dynamique bornant les lectures parallèles à `floor(BW_NVMe / état_par_agent)` simultanées. Le cap est un **paramètre de configuration** (pas une constante compilée) : il pourra être recalibré si un substrat de stockage seL4-natif est un jour mesuré, mais il n'est plus indexé sur une qualification de hardware serveur Linux — celle-ci a été abandonnée (voir ci-dessous), les latences Linux/NVMe n'étant pas transférables à la stack seL4 cible.

**Cap actif Phase 2/3** — dimensionné sur la borne basse toutes classes mesurées (QD=1 min) :

```
# BW mesurée QD=1 min, i3en.xlarge — instance B (741 MB/s) — borne basse classe 1 :
cap_actif_classe1 = floor(741 MB/s / 50 MB) = 14 agents/s

# BW mesurée QD=1, AMD Ryzen / WD SN530 — min (1 290 MB/s) — classe 2 :
cap_actif_classe2 = floor(1290 MB/s / 50 MB) = 25 agents/s

# Cap retenu (conservateur, borne basse toutes classes) :
cap_actif = 14 agents/s
```

> **IOPS aléatoires QD=1 mesurées (2026-05-18, classe 2) :** 9 039–10 865 IOPS (bs=4k, WD SN530). Cette métrique caractérise le régime production RocksDB (lookups de blocs 4–16 KB). Elle n'entre pas dans le calcul du cap C2 ContentStore (séquentiel) mais est requise pour calibrer le cap P3a sous contention multi-agents. Première mesure disponible. La reproduction sur NVMe serveur — initialement prévue pour extrapoler un régime production — a été abandonnée le 2026-05-27 (voir ci-dessous : non transférable au substrat seL4-natif).

Le cap à 14 agents/s est la **borne conservatrice de référence**. La qualification d'un NVMe serveur dédié PCIe Gen4, qui l'aurait relevé, a été **abandonnée le 2026-05-27 (décision architect)** : les latences absolues Linux/NVMe ne sont pas transférables au substrat de stockage seL4-natif cible, donc une qualification serveur ne prédirait rien sur la stack réelle. Le cap reste 14 agents/s jusqu'à mesure sur un prototype seL4-natif.

> **Régime stable effectif (2026-05-23) :** avec cap_actif = 14 agents/s et un cycle W1 moyen de 5 s, le nombre d'agents simultanément actifs en régime stable est `14 × 5 = 70`. Ce chiffre — pas plusieurs centaines — est la capacité active démontrable sur hardware consumer. Il est distinct de la densité hébergée (P1a, ~3 M agents idle sur 16 GB RAM, validée T6), qui mesure le coût mémoire au repos. Toute communication externe doit distinguer ces deux métriques. Voir `spec/01-vision.md §3.1` (note portée hardware).

> **Signature architecturale (2026-05-23) :** l'écart entre densité hébergée (~3 M agents idle) et densité opérationnelle (~70 agents actifs simultanément) n'est pas un défaut — c'est la signature d'un design où le coût d'un agent dormant est quasi nul (9.7 KB RAM, état sur NVMe) et tout le coût est dans l'activité I/O. Le réveil à la demande (ADR-0031) est la réponse correcte à cet écart : on n'a pas à réserver des ressources pour les agents dormants, seulement pour les actifs. Le ratio 3 M / 70 est la preuve que l'architecture atteint son objectif de décorréler densité hébergée et densité active.

> **Projection ~100 agents/s sur Gen4 — abandonnée (2026-05-23, clos 2026-05-27) :** le calcul `floor(5 000 MB/s / 50 MB) = 100` supposait que le goulot était la bande passante I/O séquentielle d'un serveur Gen4. Deux faits l'ont périmé. (1) T5-ter (Mode A vs Mode B, **clos 2026-05-24**, ADR-0032 §D4) a montré que la compaction RocksDB contribue 3–17 ms de variance p99 ; en régime stabilisé le cap reste valide, mais ce résultat porte sur le substrat Linux. (2) La qualification serveur qui aurait transformé ce calcul en plafond mesuré a été **abandonnée le 2026-05-27** pour non-transférabilité au substrat seL4-natif. ~100 agents/s n'est donc ni un plafond planifié ni un livrable du projet : c'est une arithmétique qui ne sera pas poursuivie sur ce substrat.

**Arithmétique serveur (hypothèse non mesurée, non poursuivie) :**

```
# Hardware serveur PCIe Gen4, ≥ 5 GB/s mesuré multi-thread (hypothèse) :
arithmétique ≈ floor(5 000 MB/s / 50 MB) = 100 agents/s
```

Ce chiffre est **hypothétique et abandonné comme objectif de qualification** (décision architect 2026-05-27). Il n'est conservé qu'à titre d'illustration de la sensibilité du cap à la bande passante. Le protocole §3.1 interdit en tout état de cause de le publier comme mesuré ; il ne doit pas non plus être présenté comme un plafond planifié.

> **Question ouverte (revue externe §6.4) :** le « 50 MB / agent » (état W1) est-il mesuré ou un objectif de design ? Si c'est un target, C2 a deux variables hypothétiques empilées. À clarifier dans `reference-workload.md §W1` (statut du paramètre « État long-terme par agent »).

Les requêtes en attente dans la queue sont ordonnées selon trois critères :

1. **Priorité sémantique** : supervisor calls > foreground user > batch background. Un agent supervisor qui répond à un `ValidationRequest` (A3) est prioritaire sur un agent batch qui traite un fichier.

2. **Affinité de cache** : préférer les agents dont des blocs ContentStore sont déjà en cache (coût de chargement réduit). Cette heuristique favorise naturellement les agents actifs récemment et les agents partageant un état de base commun.

3. **Localité temporelle** : parmi les agents à même priorité, préférer celui dont le réveil est le plus imminent (réduire la latence perçue agent-par-agent).

### 3.4 Coordination avec C1

L'I/O Admission Control (C2) doit être coordonné avec le scheduler d'inférence (C1). Précharger l'état d'un agent sans slot d'inférence disponible gaspille du cache et retarde d'autres agents. Le scheduler optimal précharge exactement k agents, où k = slots d'inférence disponibles imminents.

Cela implique un **scheduler unifié** qui gère conjointement les deux queues (I/O préchargement + inférence) plutôt que deux schedulers indépendants.

### 3.5 État et références

| | |
|-|-|
| **État** | **Implémenté Phase 7 (2026-05-22)** — `IoAdmissionQueue` dans `poc/runtime/src/io_queue.rs` (ADR-0030). Pipeline C2→C1 vérifié par scénario S10 (K=3, 3/3 pass). FutureWork : cycle evict/wake agent réel + coordination explicite C1→C2 (ADR-0030 §FutureWork). |
| **Déclenche** | Phase 3 — dès que plusieurs centaines d'agents coexistent sur une machine. |
| **Références** | `spec/04-hypotheses.md §H-densité-hébergée` et `§H-densité-active` (métriques distinctes, W1 révisé), `benchmarks/reference-workload.md §W1`, ADR-0030, `poc/scenarios/S10-unified-scheduler/` |

---

## 4. C3 — Épuisement épistémique

**Identifiant :** C3

### 4.1 Description

Un agent long-courrier (profil B — 6 mois, ~50 000 actions) accumule un log causal dont la taille dépasse la fenêtre de contexte du modèle LLM. L'agent perd progressivement la mémoire causale de ses propres décisions : il ne peut plus expliquer pourquoi une action a été prise il y a 3 mois, et potentiellement répète des erreurs déjà commises ou contredit ses propres engagements.

Ce plafond est fondamentalement différent de C1 et C2 : il est **épistémique** — relatif à ce que l'agent peut "savoir" de lui-même — non physique.

### 4.2 Analyse

Paramètres (fenêtre de contexte 2026) :

| Paramètre | Valeur |
|-----------|--------|
| Fenêtre de contexte typique (GPT-4o, Claude 3.5+) | 128K–200K tokens |
| Tokens par entrée log (action_id + description + résultat) | ~500 tokens |
| Nombre d'actions visibles dans le contexte complet | ~256–400 actions |
| Actions lifetime profil B (6 mois) | ~50 000 |
| Fraction de l'historique visible | < 1 % |

Même avec des fenêtres de contexte croissantes (1M tokens en 2026), un agent actif long terme sature sa fenêtre en quelques semaines. Le log causal répond à l'auditabilité externe (le superviseur peut reconstruire l'historique depuis la DB), mais pas à l'**auto-cohérence** de l'agent.

### 4.3 Directions architecturales

Trois approches sont en tension :

| Approche | Description | Avantages | Inconvénients |
|----------|-------------|-----------|---------------|
| **A — Mémoire sémantique noyau** | Le scheduler maintient un index RAG des actions passées (embeddings + résumés hiérarchiques). L'agent appelle une primitive `agent_recall(query)` → top-K actions pertinentes. | Interface uniforme ; qualité garantie par le noyau ; auditabilité. | Noyau doit comprendre la sémantique des actions ; couplage fort avec les modèles d'embedding ; complexité noyau augmentée. |
| **B — Mémoire sémantique userland** | L'agent gère son propre index via les primitives existantes (ContentStore + EmitType custom, log causal). Le noyau reste sémantiquement aveugle. | Noyau simple ; flexibilité totale ; déjà expressible avec les primitives actuelles. | Chaque agent implémente sa propre mémoire ; pas de garantie de qualité ni d'interopérabilité ; duplication d'effort. |
| **C — Sessions bornées avec résumé causal** | Les sessions sont limitées en durée (ex. 1 jour). À la jonction, l'agent produit un **résumé causal** (actions significatives, décisions clés, engagements actifs) injecté comme contexte au démarrage de la session suivante. | Compatible avec les primitives actuelles (checkpoint A4 + ContentStore) ; pas de changement noyau. | Dépend de la capacité de résumé du LLM ; perte d'information irréversible ; le résumé lui-même peut être erroné. |

**Observation :** ces trois approches ne sont pas mutuellement exclusives. Une architecture hybride est possible : sessions bornées (C) comme mécanisme de base, mémoire sémantique optionnelle en userland (B) pour les agents à haute criticité, primitive noyau (A) comme option avancée de Phase 4+.

### 4.4 Contrainte de décision

Cette question doit être résolue — au moins partiellement — avant de spécifier les primitives agent au-delà de A1–A4 (Phase 3). Si `agent_recall()` est une primitive noyau (approche A), son contrat doit être défini avant l'implémentation du scheduler. Si c'est une convention userland (approche B), les primitives actuelles suffisent et aucun changement noyau n'est requis.

**Recommandation provisoire :** Commencer par l'approche C (sessions bornées + checkpoint A4), qui est implémentable avec les primitives actuelles et reporte la décision A vs B à un moment où les données d'usage réel seront disponibles.

### 4.5 Lien avec A1

`agent_introspect` (A1) expose la position causale (seq, last_action_id, last_snapshot) — un outil de navigation dans le log, pas un outil de mémoire. A1 ne résout pas C3 ; elle en est un prérequis (l'agent sait où il est dans sa propre histoire avant de décider quoi charger).

### 4.6 État et références

| | |
|-|-|
| **État** | Question de design ouverte. Recommandation provisoire : approche C (sessions bornées). |
| **Déclenche** | Spécification primitives Phase 3 (au-delà de A1–A4). |
| **Références** | `spec/02c-primitives-agent.md §A1`, `spec/01-vision.md §2.4`, `spec/04-hypotheses.md §H-profil-B` |

---

## 5. Interactions et priorité

### 5.1 Interactions entre plafonds

| Interaction | Description |
|-------------|-------------|
| C1 × C2 | Le scheduler unifié (C1+C2) précharge exactement les k agents dont un slot d'inférence est imminent. Découplés, les deux schedulers gaspillent cache et VRAM. |
| C1 × C3 | Les agents suspendus en attente d'inférence (C1) pourraient utiliser ce temps de suspension pour consolider leur mémoire sémantique (C3) — résumé des actions récentes avant libération du contexte LLM. |
| C2 × C3 | Si la mémoire sémantique est gérée en noyau (approche A de C3), son index grossit l'état per-agent dans ContentStore au-delà de 50 MB. La borne C2 s'élargit proportionnellement. Cela renforce l'argument pour l'approche B (userland). |

### 5.2 Priorité de résolution

| Priorité | Plafond | Déclencheur |
|----------|---------|-------------|
| **Immédiate** | C3 | Bloque la spécification des primitives Phase 3. Décision requise avant ADR-0012+. |
| **Phase 3** | C1 | Bloque le scheduler de production avec plusieurs dizaines d'agents actifs. |
| **Phase 3** | C2 | Bloque le déploiement multi-agents dense (> 200 agents/machine). |
