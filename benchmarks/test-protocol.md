# Protocole de test et de mesure

**Date de création :** 2026-05-13
**Dernière mise à jour :** 2026-05-15
**Statut :** Brouillon — itération attendue avant gel
**Emplacement cible dans le repo :** `benchmarks/test-protocol.md`

---

## 0. Contexte de cette session (handoff Claude CLI)

Ce document est issu d'une session de revue (2026-05-13) qui a constaté que les mesures du projet — L19 sur T5, P4.x sur les capabilities, H-inférence-coût en phase 2 — ont été produites sans cadre formel de reproductibilité. Chaque mesure était jusqu'ici un one-shot avec ses conventions implicites. À mesure que le projet entre dans la phase de mesures quantitatives (T5 qualif, futurs T6, comparaisons baseline), ce vide devient un risque : un résultat non reproductible ne peut pas falsifier une hypothèse, et la thèse centrale du projet *est* falsifiable.

**Décisions prises pendant la session :**

1. **Séquence retenue.** Écrire d'abord le protocole, puis l'appliquer à D2 (smoke `--fresh`) comme premier cas concret. La logique : le protocole testé sur un cas vivant évite qu'il devienne théorique. D1 (REST `caused_by_list` primaire) et T1 (calibration fsync) suivent.

2. **Dépendance externe identifiée.** La qualification T5 à N=10⁸ requiert un hardware (NVMe ≥ 1 GB/s, ≥ 16 GB RAM) qui n'est pas disponible sur la machine de développement actuelle. T5 reste *partiellement validée* (L19, N=10⁶ cache chaud) jusqu'à ce qu'une qualification sur hardware adéquat soit organisée (cloud ponctuel ou prêt de labo).

3. **ADR-0011 rédigé (2026-05-14).** L'ADR formel sur les options RocksDB de Layer 0 (bloom filter, block cache, `cache_index_and_filter_blocks`, pas de compression — cf. L18) a été rédigé *avant* la qualification T5 N=10⁸, comme document de configuration de référence pour fixer les paramètres de test. La session initiale prévoyait de le rédiger après ; ce choix a été revisé pour éviter que T5-qualif tourne sans configuration documentée. (Note : le numéro "ADR-0009" dans la version 2026-05-13 de ce document était erroné — ADR-0009 est le profil acteurs LLM/séparation machine-humain ; ADR-0011 = options RocksDB.)

4. **Rôle du modèle LLM clarifié.** Les modèles LLM entrent dans le manifest des tests pour *identifier* la mesure, pas pour *comparer* les modèles entre eux. Un test qui réussit sur un modèle et pas sur un autre est une donnée sur la fragilité du modèle, pas un échec du système (cf. L4 : qwen2.5:3b vs system prompt français). Cette distinction est codifiée dans la matrice de validation (§5).

5. **Rôle du GPU clarifié.** Le GPU n'est pas un régime de validation supplémentaire. Il est un microscope sur l'overhead d'infrastructure : quand l'inférence CPU prend 130s (H-inférence-coût confirmée), un overhead système de 100ms est invisible ; sur GPU avec inférence à 2s, le même overhead devient 5% mesurable. Le GPU n'est donc requis que pour les mesures où l'overhead système est l'objet du test (typiquement P1 densité).

---

## 1. Rôle du protocole

Ce document définit les conditions sous lesquelles une mesure produite dans ce projet est *reproductible*, et donc utilisable pour confirmer ou réfuter une hypothèse de la spec.

Le protocole sert trois fonctions :

1. **Préciser ce qui doit être enregistré** à chaque mesure pour qu'un tiers puisse répliquer.
2. **Distinguer la fonctionnalité de la performance.** Un test qui répond à "le système fait-il X ?" n'a pas les mêmes exigences de reproductibilité qu'un test qui répond à "X tient-il sous tel seuil quantitatif ?".
3. **Catégoriser le statut d'une mesure** (indicatif / partiellement validé / validé) selon la diversité des conditions sous lesquelles elle a été reproduite.

Le protocole *ne définit pas* :

- Quels workloads tester — c'est `benchmarks/reference-workload.md` (W1, W2, W3).
- Quelles propriétés mesurer — c'est `spec/02-properties.md` (P1–P7).
- Les conditions d'abandon du projet — c'est `spec/04-hypotheses.md` §5.2.

---

## 2. Principes

### 2.1 Reproductibilité avant comparaison

Une mesure n'est pas comparable tant qu'elle n'est pas reproductible. La tentation naturelle est de comparer un nouveau résultat à un ancien résultat publié — par exemple le p99=11µs de L19 contre la cible P3 à 10ms. Cette comparaison est légitime seulement si les conditions sont explicites des deux côtés. Sans manifest, on ne sait pas si on compare des oranges et des oranges.

### 2.2 Les modèles LLM identifient, ne comparent pas

Pour tout test qui implique un appel LLM (smoke fonctionnel, workloads W1+ avec inférence réelle), le modèle utilisé doit être identifié au manifest : nom, taille, quantization, hash de poids, taille de contexte, langue du system prompt. Un test publié sans cette identification est *indicatif* au mieux.

Un test qui passe sur un modèle et échoue sur un autre est une donnée sur le *modèle*, pas sur le système — sauf si l'échec révèle un contrat manquant du système (par exemple : le système suppose que le modèle respecte un schéma de clés mémoire et ne valide pas — H-mémoire-schema). Dans ce cas, le système est en cause.

La règle de publication : pour qu'une mesure soit "validée" sur un aspect dépendant du modèle, elle doit être reproduite sur **au moins deux modèles de classe de capacité comparable** (par exemple : deux modèles avec tool-calling fiable, taille ≥ X paramètres). Cette classe doit être déclarée dans le manifest.

### 2.3 Conditions de cache déclarées explicitement

Toute mesure de performance sur un substrat avec cache (RocksDB, page cache OS, cache LLM) doit déclarer le régime de cache au moment de la mesure. Quatre régimes à distinguer (ADR-0026, 2026-05-18) :

- **Cache chaud** : le working set tient en cache, les accès sont quasi-mémoire. Sert à mesurer le plancher de coût.
- **Cache-mixte contraint** : `drop_caches=3` appliqué avant le run *et* RAM/dataset ≤ 2×. Régime **représentatif** pour les propriétés de latence de lookup (P3a) — voir ADR-0026 pour la justification empirique. Utilisé par T5 (toutes classes).
- **Cache miss dominant** : working set > cache × 5 (ratio RAM/dataset < 0,2×). Régime idéal — physiquement inatteignable sur hardware grand public avec RAM ≥ 8 GB pour un dataset de ~15 GB sans contrainte artificielle.
- **Cache mixte non déclaré** : valeur indicative seulement, à ne pas comparer à une cible quantitative.

L19 est un cas exemplaire : N=10⁶, dataset ~10 MB, cache 256 MB ⇒ régime cache chaud déclaré. T5 N=10⁸ opère en régime cache-mixte contraint (drop_caches appliqué, RAM/dataset ≤ 2×).

### 2.4 Statistique : convergence ou pass/fail

Pour un test binaire (smoke fonctionnel) : le critère est K runs consécutifs réussis, avec K ≥ 3 par défaut, sans aucun échec dans la fenêtre. Un seul échec dans K runs disqualifie la mesure.

Pour un test continu (latence, débit) : reporter p50, p95, p99, p99.9, et écart-type, calculés sur ≥ 1000 échantillons (ou ≥ 10 000 pour les percentiles p99.9). Reporter aussi le nombre de runs (réexécutions complètes du benchmark) et la dispersion inter-run du p99.

---

## 3. Dimensions de variation et manifests

Quatre dimensions doivent être documentées pour chaque mesure. Chacune fait l'objet d'un fragment JSON joint au rapport.

### 3.1 Hardware (`hardware.json`)

| Champ | Description | Exemple |
|-------|-------------|---------|
| `cpu_model` | Référence constructeur exacte | `"AMD Ryzen 7 5800X"` |
| `cpu_cores_physical` | Nombre de cœurs physiques | `8` |
| `cpu_cores_logical` | Nombre de threads logiques | `16` |
| `cpu_base_ghz` | Fréquence de base | `3.8` |
| `ram_gb` | RAM totale | `32` |
| `ram_type` | Type et fréquence | `"DDR4-3200"` |
| `storage_model` | Modèle du périphérique de stockage utilisé pour la mesure | `"Samsung 980 Pro 1TB"` |
| `storage_seq_read_mb_s` | Débit séquentiel mesuré (pas spec constructeur) | `6800` |
| `gpu_model` | Si présent | `"NVIDIA RTX 3060 12GB"` ou `null` |
| `gpu_vram_gb` | Si GPU | `12` ou `null` |

Le champ `storage_seq_read_mb_s` doit être *mesuré* sur le périphérique utilisé (par exemple `fio` ou `dd`) — les spec constructeur sont systématiquement optimistes et varient selon le firmware, le remplissage du SSD, et la température.

### 3.2 Substrat logiciel (`software.json`)

| Champ | Description |
|-------|-------------|
| `os` | Distribution + version + kernel (`uname -r`) |
| `docker_version` | `docker --version` |
| `docker_image_hashes` | Map nom → SHA256 des images utilisées |
| `python_version` | Si applicable |
| `rust_version` | Si applicable (`rustc --version`) |
| `rocksdb_version` | Version de la lib liée |
| `ollama_version` | Si Ollama utilisé |
| `git_commit` | SHA du commit testé du projet |
| `git_dirty` | Booléen : working tree non-propre ? |

`git_dirty: true` n'invalide pas la mesure mais la place en *indicatif*. Une mesure publiable comme "validée" doit être faite sur un working tree propre, sur un commit identifiable.

### 3.3 Modèle LLM (`model.json`) — quand applicable

| Champ | Description |
|-------|-------------|
| `name` | Nom canonique | `"qwen2.5:3b"` |
| `quantization` | Quantization utilisée | `"Q4_K_M"` |
| `weights_sha256` | Hash des poids (ollama `show --modelfile` ou équivalent) |
| `context_length` | Taille de contexte effective | `8192` |
| `system_prompt_lang` | Langue du system prompt | `"en"` |
| `system_prompt_sha256` | Hash du system prompt | |
| `capability_class` | Classe de capacité déclarée | `"tool-calling-7b-en"` |

Le champ `capability_class` est ce qui permet de dire "deux modèles de classe comparable" en §2.2. Les classes sont définies au cas par cas et documentées au moment où elles servent — par exemple, T2 du smoke a besoin d'un modèle avec tool-calling fiable en français, ce qui exclut qwen2.5:3b (L4).

### 3.4 Paramètres workload (`workload.json`)

Spécifique à chaque test. Doit inclure au minimum :

- Nom du test (`T5`, `P4.1`, `smoke-fresh`, etc.)
- Paramètres d'échelle (`BENCH_N`, `BATCH_SIZE`, nombre d'agents, durée de mesure)
- Seed des générateurs aléatoires utilisés
- Cible quantitative si applicable (par exemple `p99_ms_max: 10`)
- Régime de cache déclaré (cf. §2.3)

**Pour T5 spécifiquement**, inclure également `emit_payload_size_distribution` : histogramme des tailles de `emit_payload` en bytes (p50, p90, p95, p99, max, count). Cette métrique est requise par ADR-0017 pour calibrer le seuil BlobDB avant activation Phase 3 — le seuil `min_blob_size` sera le p90 ou p95 observé sur W2, pas une valeur arbitraire.

**Décision Q3 (2026-05-16) :** En l'absence de W2 réel, la distribution de convention documentée dans `benchmarks/reference-workload.md §emit-payload-distribution` (p50=256 B, p90=4 KB, p95=8 KB, p99=32 KB, max=64 KB) est la **référence ferme** pour le dimensionnement BlobDB jusqu'au premier W2 réel mesuré. Les harness T5 actuels (entrées 100 bytes fixes) ne couvrent pas cette distribution — un benchmark T5-payload ou W2 dédié sera produit. La valeur `min_blob_size = 4 KB` d'ADR-0017 §3bis est confirmée comme cohérente avec p90 de cette convention.

**Décision Q2 (2026-05-16) — modèle d'accès :** Le champ `workload.json.access_pattern` est requis pour tout benchmark de lookup causal. Valeurs admises : `uniform` (Modèle A worst-case, T5 actuel) ou `recency` (Modèle B, convention de référence — paramètres K=128, recouvrement inter-agent 10 %, voir `benchmarks/reference-workload.md §W1-access`). Un benchmark sans `access_pattern` déclaré est classifié *indicatif* au mieux.

---

## 4. Format de sortie standard

Chaque exécution de test produit un répertoire de résultats avec la structure suivante :

```
results/
  <test-name>/
    <YYYY-MM-DDTHHMMSSZ>/
      hardware.json
      software.json
      model.json          # si applicable
      workload.json
      raw/                # logs bruts, captures, timings
      summary.md          # résumé lisible humain
      verdict.json        # statut final
```

`verdict.json` contient :

```json
{
  "test": "T5",
  "outcome": "pass" | "fail" | "inconclusive",
  "classification": "indicatif" | "partiellement-valide" | "valide",
  "metrics": {
    "p50_us": 4,
    "p95_us": 8,
    "p99_us": 11,
    "p99_9_us": 18
  },
  "notes": "régime cache chaud — qualification N=10^8 en attente de hardware NVMe"
}
```

Le champ `classification` est celui de la matrice §5. Il est de la responsabilité de l'auteur du test de le poser honnêtement — un test publié comme "validé" sur un seul hardware est une violation du protocole.

---

## 5. Matrice de validation

| Classification | Conditions minimales | Usage légitime |
|----------------|----------------------|----------------|
| **Indicatif** | 1 hardware, 1 modèle (si applicable), 1 run réussi | Signal d'orientation précoce. Ne peut pas confirmer ni réfuter une hypothèse. Apparaît dans les notes de lab, pas dans la spec. |
| **Partiellement validé** | 1 hardware *en régime représentatif* (cache miss dominant pour mesures de perf), ou 2 hardware en régime cache chaud ; K ≥ 3 runs ; pour tests modèle-dépendants : 1 modèle de la classe attendue | Permet d'écrire "hypothèse partiellement validée" dans `spec/04-hypotheses.md` §5.1. État typique pour beaucoup d'hypothèses du projet à date. |
| **Validé** | ≥ 2 hardware de classes distinctes en régime représentatif (cache-mixte contraint ou cache-miss dominant, cf. §2.3 amendé par ADR-0026) ; K ≥ 3 runs par hardware ; pour tests modèle-dépendants : ≥ 2 modèles de classe comparable | Permet d'écrire "validée" et de fermer une dette d'hypothèse. Permet de citer la mesure dans une publication externe. |

**Classes de hardware distinctes** : on entend par là des combinaisons qui diffèrent significativement sur au moins deux dimensions parmi {CPU vendor, gamme de stockage (SATA / NVMe Gen3 / NVMe Gen4), RAM (< 16 GB / 16–64 GB / > 64 GB), présence/absence de GPU dédié}. Deux NVMe Gen4 de marques différentes sur deux Ryzen sont une seule classe.

**Note sur l'évolutivité.** Cette matrice est volontairement modérée — exiger ≥ 3 hardware pour "validé" serait correct académiquement mais bloquerait toute progression sur un projet sans budget hardware. À mesure que le projet mûrit et que des contributeurs externes apparaissent, la barre pour "validé" peut être relevée.

---

## 6. Application aux mesures existantes

Lecture des résultats actuels du projet à travers cette grille.

### 6.1 T5 (latence causale)

**Propriété mesurée (Q1, 2026-05-16) :** **P3a uniquement** — lookup point `get(action_id)` sur log statique peuplé par `populate_synthetic`, sans write concurrent ni fsync sur le chemin chaud. T5 ne mesure ni P3b (end-to-end emit→fsync→get) ni P3c (multi-agent concurrent). La borne « p99 ≤ 10 ms » référencée par T5 est la borne de P3a et **uniquement** de P3a. Voir `spec/02-properties.md §P3` pour la décomposition complète.

- **Régime mesuré** : cache-mixte contraint (drop_caches appliqué, RAM/dataset ≤ 2× — régime représentatif au sens ADR-0026).
- **Hardware** : classe 1 — AWS i3en.xlarge, NVMe instance store XFS, 741–769 MB/s QD=1, 31 GB RAM. Classe 2 — AMD Ryzen 5 PRO 4650U, WD SN530 NVMe PCIe, 1 290–1 321 MB/s QD=1, 14 GB RAM.
- **Modèle** : N/A (test pur infrastructure).
- **Runs** : K=7 conformants classe 1 (R2–R8), K=3 conformants classe 2 (RA1–RA3). p99 pire cas toutes classes : 4 855 µs (RA3). Marge ×2 sous cible 10 ms. Détail : `results/T5/SYNTHESE.md`.
- **Modèle d'accès (Q2, 2026-05-16)** : lookups uniformes — **régime Modèle A (worst-case no-locality)**. Régime Modèle B (recency-biased) mesuré par T5-bis.
- **Distribution `emit_payload` (Q3, 2026-05-16)** : entrées de taille fixe (~100 bytes) — T5 ne couvre pas la distribution Q3. Calibration BlobDB par benchmark W2 dédié.
- **Classification actuelle** : **validé** (P3a, 2 classes hardware distinctes, K≥3 par classe, régime cache-mixte contraint conforme §2.3 amendé par ADR-0026, 2026-05-18).
- **Mesure antérieure (cache chaud)** : p99=11 µs sur N=10⁶ (L19, marge ×900) — conservée comme plancher de coût.

### 6.2 P4.x (capabilities, phase 4)

- **Régime mesuré** : fonctionnel, pas de performance.
- **Hardware** : 1 machine de dev, lab.
- **Modèle** : qwen2.5:3b (pour les tests passant par tool-calling) — *un seul modèle*.
- **Dettes résolues** : D2 (`--fresh` + `POST /reset`, 2026-05-15) et D1 (`caused_by_list` primaire en REST, 2026-05-15) sont résolus. Le smoke peut désormais tourner sur DB fraîche à chaque run, ce qui lève le blocage sur la reproductibilité.
- **Runs formels** : pas encore K=3 runs indépendants sur DB fraîche.
- **Classification actuelle** : **indicatif** pour les tests fonctionnels passant par LLM. Pour passer à "partiellement validé" : K ≥ 3 runs formels avec `--fresh`, sur au moins deux modèles de classe comparable.

### 6.3 H-inférence-coût (phase 2, 130s CPU)


- **Régime mesuré** : CPU sans GPU, mesure bout-en-bout.
- **Hardware** : 1 machine de dev.
- **Modèle** : 1 modèle.
- **Classification actuelle** : **indicatif**, mais le résultat est si éloigné du seuil de réfutation (130s vs seuil 30s) qu'il est défendable de l'utiliser comme confirmation forte. Le protocole *autorise* cette lecture mais exige que la classification "indicatif" reste affichée — la robustesse vient de l'écart, pas de la reproductibilité.

### 6.4 H-rollback-latence (ContentStore, L20)

- **Régime mesuré** : cache chaud — chaîne de 1001 `SnapshotHeader` (~140 KB CF `headers`, entièrement dans le block cache RocksDB).
- **Hardware** : 1 machine de dev.
- **Modèle** : N/A (test pur infrastructure).
- **Runs** : 1 série, benchmark dev.
- **Résultats clés** :

| Workload | Depth | p50 µs | p95 µs | p99 µs |
|----------|-------|--------|--------|--------|
| W1 (50 KB) | 100 | 71 | 88 | 107 |
| **W2 (500 KB)** | **100** | **71** | **99** | **111** |
| W2 (500 KB) | 1000 | 724 | 837 | 1 052 |

- **Cible P2 (p95 ≤ 100 ms sur W2/depth=100) : conforme — marge ×1 000.**
- **Observation architecturale** : latence invariante par rapport à la taille des blocs (W1 50 KB ≈ W2 500 KB à depth équivalent). Le rollback ne lit que les `SnapshotHeader` (~140 bytes/entrée), pas les blocs. Scaling quasi-linéaire : ~0.72 µs/traversée sur cache chaud.
- **Classification actuelle** : **indicatif**. Pour passer à "partiellement validé" : K ≥ 3 runs en régime cache miss dominant (dataset >> block cache).

### 6.5 H-revoke à l'échelle (capabilities, L21)

- **Régime mesuré** : in-memory (CapabilityStore — deux HashMaps).
- **Hardware** : 1 machine de dev.
- **Modèle** : N/A.
- **Runs** : 1 série Criterion.
- **Résultats clés** :
  - `check()` : p99 = 361 ns (cible ≤ 1 µs : **conforme**).
  - `revoke()` à N=1 111 : p95 = 189 µs. À N=111 111 : p95 = 23 ms.
  - Plafond pratique ~10K caps (cache miss dominant au-delà). Critère spec §H-revoke (< 5% CPU sous W1) : conforme à ~0.03% même à N=100K.
  - Deux chemins vers O(1) documentés si le plafond est atteint : epoch-based, revocable forwarders.
- **Classification actuelle** : **indicatif**. `check()` et `revoke()` conformes au critère spec. Hors cible aspirationnelle README (< 1 ms) pour N > 10K.

### 6.6 H-densité / T6 (L27–L28)

- **Régime mesuré** : idle pur — Wasmtime 5 KB/acteur vs Docker Python 3.11-slim + langchain-core + openai + httpx + pydantic.
- **Hardware** : 1 machine de dev, N=10 containers.
- **Modèle** : N/A (infra seule, aucune inférence).
- **Runs** : 1 série.
- **Résultats clés** :
  - Overhead Docker Python (méthode hôte, delta MemAvailable) : **43 314 KB/container**.
  - Ratio Wasmtime/Docker-Python : **8 670×** (cible H-densité ≥ 5× : **conforme — marge ×1 500**).
  - Note : plancher bas (langchain-core sans extensions lourdes). Un agent LLM complet (numpy, langchain complet) donnerait un ratio encore plus élevé.
- **Classification actuelle** : **indicatif** (1 hardware, 1 run, N=10). Pour passer à "partiellement validé" : K ≥ 3 runs, N ≥ 50 containers, NVMe ≥ 1 GB/s mesuré.

### 6.7 H-cb-overhead (commit barrier, L22)

- **Régime mesuré** : N=1 000 cycles `process_one()` (snapshot ContentStore + append CausalLog), cache chaud.
- **Hardware** : 1 machine de dev.
- **Modèle** : N/A.
- **Runs** : 1 série Criterion.
- **Résultats clés** : moyenne = 11 µs/cycle, p99 = 26 µs. Cible §H-cb-overhead (< 1% W1 à 60 cycles/min) : overhead = 660 µs/min = **0.0002% — marge ×25 000**.
- **Classification actuelle** : **indicatif** (1 hardware, 1 run, cache chaud). Pas de test de dérive sous charge soutenue.

---

## 7. Premier cas d'application : D2 (smoke `--fresh`) ✓ Résolu (2026-05-15)

D2 du TODO était : *Smoke test sur DB persistante — ajouter un flag `--fresh` qui reset la DB avant de démarrer, ou utiliser un namespace unique par run horodaté.*

**Résolution (2026-05-15) :** `POST /reset` (activé via `ALLOW_RESET=1`) + flag `--fresh` dans `smoke_test.sh`. La DB est effacée en début de chaque run ; les runs successifs n'accumulent plus de données.

Ce qui reste à faire pour passer à "partiellement validé" : lancer K ≥ 3 runs formels sur DB fraîche, sur au moins deux modèles de classe `tool-calling-en`, et produire les manifests ci-dessous.

Application initiale du protocole (conservée comme référence pour les prochains runs formels) :

**Type de test** : fonctionnel binaire (pass/fail).

**Manifests requis** :
- `hardware.json` : la machine de dev courante.
- `software.json` : commit post-D2 (working tree propre).
- `model.json` : qwen2.5:3b initialement.
- `workload.json` : `{ "test": "smoke-fresh", "k_runs": 3, "fresh": true, "model_calls_expected": [...] }`.

**Critère de pass** : 3 runs consécutifs réussis, sur DB fraîche à chaque run, sans accumulation entre runs.

**Critère de pass renforcé** (pour passer à "partiellement validé") : 3 runs sur deux modèles distincts de la classe `tool-calling-en` (qwen2.5:3b et un modèle alternatif — llama3.2:3b ou autre disponible).

**Ce que D2 teste implicitement** : que le système est *capable* de tourner sur état neutre, donc que les mesures futures (T1, T5, etc.) peuvent être bâties sur cette propriété. D2 est une précondition au reste, pas un livrable autonome.

---

## 8. Extension thermique

### 8.1 Quand cette section s'applique

Tout test dont la **durée d'exécution effective dépasse 5 minutes** sous charge soutenue doit appliquer §8. Cela inclut :

- W1, W2, W3 (durée nominale 1 heure — cf. `benchmarks/reference-workload.md`).
- T5 à N=10⁸ (`cargo bench --bench causal_lookup` ; durée populate + lookups estimée 10–30 min sur NVMe).
- T6-qualif si étendu à plusieurs centaines de containers (mesure RSS soutenue).
- Tout futur test de saturation P7 (multi-jour potentiellement).

Les tests courts (T5 dev N=10⁶ à ~5 s, smoke fonctionnel D2 à < 60 s, P4.x) **ne sont pas concernés** : la dérive thermique y est négligeable comparée à la variance algorithmique.

### 8.2 Modèle de menace thermique

Trois mécanismes distincts peuvent corrompre une mesure long-running. Chacun doit être détecté séparément ; agréger les trois sous "throttling" masque la cause.

| Mécanisme | Source | Manifestation observable |
|-----------|--------|--------------------------|
| **CPU thermal throttling** | T_cœur > T_jmax (typiquement 95–105 °C selon CPU) → réduction de fréquence | `cur_freq` < `base_freq` malgré charge ; flag `PROCHOT` dans `turbostat` ; latences p99 qui dérivent à la hausse au cours du run |
| **NVMe thermal throttling** | T_contrôleur ou T_NAND > seuil firmware (typiquement 70–85 °C) → réduction de débit | `composite_temperature` proche du `warning_temp_threshold` lu en sysfs ; débit séquentiel mesuré qui chute en milieu de run ; `critical_warning` non nul dans SMART |
| **Power capping (RAPL/PL1/PL2)** | Limite de puissance configurée (BIOS ou tdp-control) atteinte avant T_jmax → fréquence plafonnée | `pkg_watts` plafonné à une valeur constante ; fréquence stable mais < `max_freq` malgré T_cœur < T_jmax. Distinct du throttling thermique : la cause est la limite de puissance, pas la chaleur. À documenter mais non disqualifiant si reproductible. |

Le throttling NVMe est le risque dominant pour T5 et W2/W3 (RocksDB compaction + log append soutenu) ; le throttling CPU est le risque dominant pour W1 actif (densité d'agents sous inférence simulée).

### 8.3 Métriques à capturer

Trois familles, à enregistrer en parallèle du run dans `raw/thermal/`.

**Capture continue (intervalle 5 s, sur toute la durée du run) :**

| Fichier de sortie | Source | Champs |
|-------------------|--------|--------|
| `raw/thermal/cpu.csv` | `sensors -j` (parsé) ou lecture directe de `/sys/class/hwmon/hwmon*/temp*_input` | `ts_unix_ms, core_id, temp_c` |
| `raw/thermal/cpu_freq.csv` | `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` | `ts_unix_ms, cpu_id, freq_khz` |
| `raw/thermal/nvme.csv` | `nvme smart-log -o json /dev/nvmeXnY` ou lecture sysfs `/sys/class/nvme/nvmeX/hwmon*/temp*_input` | `ts_unix_ms, nvme_id, composite_temp_c, sensor1_c, sensor2_c, critical_warning, percentage_used` |
| `raw/thermal/nvme_throttle.csv` | sysfs `/sys/class/nvme/nvmeX/nvmeXnY/queue/io_poll`, et compteurs `nvme get-log` page `0x02` (Smart/Health) — champ `Warning Composite Temperature Time` et `Critical Composite Temperature Time` (en minutes) | `ts_unix_ms, nvme_id, warn_temp_minutes, crit_temp_minutes` |

**Capture périodique (intervalle 30 s) :**

| Fichier | Source | Pourquoi |
|---------|--------|----------|
| `raw/thermal/turbostat.txt` | `turbostat --interval 30 --quiet --show CPU,Bzy_MHz,TSC_MHz,IRQ,SMI,POLL,POLL%,CoreTmp,PkgTmp,PkgWatt,RAMWatt,CorWatt --out raw/thermal/turbostat.txt` | Donne `Bzy_MHz` (fréquence effective), `PkgWatt` (consommation paquet), et flags PROCHOT/THERM_STATUS sur certains CPU. Source canonique pour distinguer throttling thermique vs power capping. |

**Snapshots ponctuels (avant + après run) :**

| Fichier | Contenu |
|---------|---------|
| `raw/thermal/baseline_pre.json` | Pour chaque cœur : T_idle, freq_idle. Pour chaque NVMe : T_idle, T_warning_threshold, T_critical_threshold (`nvme id-ctrl`). T_ambiante si capteur disponible (`ipmitool sdr type Temperature` sur serveur). |
| `raw/thermal/baseline_post.json` | Mêmes champs, mesurés ≥ 60 s après la fin du run pour vérifier le retour à l'idle. |
| `raw/thermal/system.json` | `cat /sys/devices/system/cpu/intel_pstate/no_turbo`, gouverneur (`scaling_governor`), `power_profile` (sysfs ou `powerprofilesctl get`), température cible RAPL si configurée. Documente l'environnement de gestion d'énergie qui conditionne tout le reste. |

### 8.4 Procédure d'exécution

1. **Préchauffe interdite, refroidissement préalable obligatoire.** Avant le run, vérifier que tous les cœurs sont à T_idle ± 3 °C de leur valeur nominale (mesurable au repos après ≥ 5 min sans charge). Capturer `baseline_pre.json`.
2. **Démarrer la capture continue** (`cpu.csv`, `cpu_freq.csv`, `nvme.csv`, `nvme_throttle.csv`, `turbostat.txt`) **avant** le démarrage du benchmark, idéalement 30 s plus tôt pour avoir une ligne de base juste avant charge.
3. **Exécuter le benchmark** selon son protocole propre (T5, W1, etc.). Enregistrer le timestamp Unix ms de début et de fin du run dans `workload.json` sous `run_started_unix_ms` et `run_ended_unix_ms`.
4. **Maintenir la capture continue ≥ 60 s après la fin** pour observer la phase de refroidissement. Capturer `baseline_post.json` à T+60 s.
5. **Calculer les indicateurs de §8.5** sur les fichiers capturés ; produire `raw/thermal/summary.json`.
6. **Évaluer les seuils d'invalidation §8.6** ; reporter le verdict thermique dans `verdict.json` sous le champ `thermal: { status: "clean" | "drifted" | "throttled" | "power_capped", details: ... }`.

Le binaire de capture est un script shell séparé (`benchmarks/thermal-capture.sh`, à créer lors de la première application de §8) lancé en arrière-plan ; il ne doit pas tourner sur les mêmes cœurs que le benchmark (utiliser `taskset -c` pour le pinner sur un cœur dédié — typiquement le dernier cœur logique). L'overhead `sensors` est ~5 ms/poll, négligeable à 5 s d'intervalle.

### 8.5 Indicateurs dérivés

À calculer sur `raw/thermal/*.csv` après le run :

| Indicateur | Définition | Source |
|------------|------------|--------|
| `delta_t_max_cpu_c` | `max(temp_c) - min(temp_c)` sur les cœurs actifs durant la fenêtre `[run_started, run_ended]` | `cpu.csv` |
| `delta_t_max_nvme_c` | `max(composite_temp_c) - min(composite_temp_c)` durant la fenêtre du run | `nvme.csv` |
| `freq_drift_pct` | `(mean(freq_khz) sur la 1re minute du run - mean(freq_khz) sur la dernière minute) / mean(freq_khz) sur la 1re minute * 100` | `cpu_freq.csv` |
| `freq_below_base_pct` | Fraction du temps de run où `cur_freq < base_freq` (`cpu_base_ghz` du `hardware.json`) | `cpu_freq.csv` |
| `nvme_warn_minutes_delta` | `warn_temp_minutes(post) - warn_temp_minutes(pre)` | `nvme_throttle.csv` |
| `nvme_crit_minutes_delta` | `crit_temp_minutes(post) - crit_temp_minutes(pre)` | `nvme_throttle.csv` |
| `pkg_watt_capped` | Vrai si `PkgWatt` reste à ±2 % d'une valeur constante pendant > 30 % du run avec charge soutenue | `turbostat.txt` |

### 8.6 Seuils d'invalidation

Un run est classé selon les règles ci-dessous, évaluées dans l'ordre. La première règle satisfaite fixe le verdict.

| Verdict | Règle | Action |
|---------|-------|--------|
| `throttled` | `nvme_crit_minutes_delta > 0` **OU** `freq_below_base_pct > 5 %` | **Run invalide.** Refaire après refroidissement et/ou retrait de la cause (cf. §8.7). Ne **pas** publier les métriques de performance ; conserver les fichiers `raw/thermal/` comme évidence. |
| `drifted` | `delta_t_max_cpu_c > 15 °C` **OU** `delta_t_max_nvme_c > 10 °C` **OU** `freq_drift_pct > 3 %` (sans avoir déclenché `throttled`) | **Run douteux.** Métriques utilisables uniquement si la dérive est expliquée (par exemple : T_ambiante a augmenté de X °C documenté dans `system.json`) ET si la dérive est monotone (pas d'oscillation). Sinon, refaire. Le `verdict.json` doit porter `thermal.status = "drifted"` même si `outcome = "pass"`. |
| `power_capped` | `pkg_watt_capped == true` **ET** `delta_t_max_cpu_c < 10 °C` (le CPU ne chauffe pas, mais la fréquence est limitée par RAPL) | **Run valide mais documenté.** La machine fonctionne sous une enveloppe de puissance volontaire ou imposée ; les chiffres sont représentatifs de cette enveloppe. À reproduire sur une seconde machine sans cap pour confirmer la classification "validé" (matrice §5). |
| `clean` | Aucune des règles précédentes | **Run valide.** |

**Justification quantitative des seuils :**

- `delta_t_max_cpu_c > 15 °C` : un delta < 10 °C est typique d'un CPU desktop sous charge stabilisée (idle ~35 °C, charge ~60–70 °C). Au-delà de 15 °C, on s'approche du régime non-stationnaire (température en montée) ou d'une saturation de refroidissement. Source : profils thermiques typiques mesurés sur Ryzen 5000 / Intel 12e gen [Intel "Thermal Solutions"][AMD "Ryzen 5000 Thermal Profile"]. Ce seuil n'est pas un standard normatif mais un compromis entre sensibilité et faux positifs sur hardware grand public correctement refroidi.
- `delta_t_max_nvme_c > 10 °C` : les NVMe modernes commencent à throttler entre 70 °C et 85 °C selon le firmware. Une dérive de 10 °C sur un run d'1 h indique un transfert thermique soutenu non dissipé, probable précurseur de throttling sur run plus long. Source : Samsung 980 Pro datasheet (T_warning à 82 °C, T_critical à 84 °C — marge réelle de 2 °C entre warning et critical).
- `freq_below_base_pct > 5 %` : une CPU moderne ne descend sous sa fréquence de base qu'en throttling thermique ou contrainte de puissance sévère. Tolérance de 5 % couvre les transients (interrupts, gouverneur powersave momentané) sans masquer un vrai throttling.
- `freq_drift_pct > 3 %` : la dérive de fréquence sur 1 h est typiquement < 1 % à charge stable. > 3 % indique soit montée thermique progressive, soit changement de régime de charge (dans ce cas la dérive thermique n'est pas la cause primaire — voir §8.8).
- `nvme_crit_minutes_delta > 0` : un seul incrément du compteur "Critical Composite Temperature Time" signifie que le contrôleur NVMe a atteint son seuil critique au moins une minute durant le run. C'est le signal SMART canonique de throttling NVMe ; aucune marge n'est tolérable. `warn_temp_minutes_delta` n'est *pas* disqualifiant mais doit être annoté.

### 8.7 Causes typiques et remédiations

| Symptôme | Cause probable | Remédiation |
|----------|----------------|-------------|
| `freq_below_base_pct > 5 %` sur CPU correctement refroidi (T_max < 80 °C) | Power capping RAPL ou gouverneur `powersave` actif | Vérifier `cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor` (devrait être `performance`) ; vérifier `intel_pstate/no_turbo` ; vérifier RAPL `cat /sys/class/powercap/intel-rapl/intel-rapl:0/constraint_0_power_limit_uw`. |
| `delta_t_max_nvme_c > 10 °C` ou throttling NVMe | Refroidissement passif insuffisant ; châssis fermé sans ventilation de la baie M.2 | Ajouter dissipateur thermique sur le NVMe ; ouvrir le châssis pour run de qualification ; ou réduire `max_background_jobs` RocksDB (cf. ADR-0011) — au prix d'une compaction plus lente. |
| `delta_t_max_cpu_c > 15 °C` sur run < 1 h | T_ambiante non stable ; pâte thermique en fin de vie ; ventilation interrompue | Documenter T_ambiante dans `system.json` ; refaire en début de matinée si la pièce chauffe en journée ; instrumenter `ipmitool sdr type Temperature` si serveur. |
| `pkg_watt_capped` inattendu | TDP configurable (cTDP) bas dans le BIOS ; profil énergétique laptop | Documenter dans `system.json` ; pour qualification "validée" §5, refaire sur machine sans cTDP. Pour T6 sur laptop : alimentation secteur obligatoire, profil "performance" obligatoire. |

### 8.8 Distinguer dérive thermique de dérive algorithmique

Une mesure long-running peut dériver pour deux raisons orthogonales : (a) le hardware ralentit (thermique), (b) le système accumule un état qui dégrade ses propres performances (compaction RocksDB, fragmentation memtable, croissance du working set au-delà du cache). La règle de discrimination :

1. **Aligner** les séries temporelles `cpu_freq.csv` + `nvme.csv` avec la série temporelle des métriques applicatives (latence p99 par fenêtre de 1 min, débit par fenêtre de 1 min — à produire par le benchmark dans `raw/perf/perf_timeseries.csv`).
2. **Calculer la corrélation de Spearman** entre `freq_khz(t)` et `inverse_latency_p99(t)`, ainsi qu'entre `composite_temp_c(t)` et `latency_p99(t)`.
3. **Interprétation :**
   - Corrélation forte (|ρ| > 0.6) entre fréquence et 1/latence → **dérive thermique dominante**. Refaire avec refroidissement adéquat.
   - Latence p99 qui dérive **sans** corrélation thermique (|ρ| < 0.3 et températures stables dans les seuils §8.6) → **dérive algorithmique**. C'est une donnée intéressante sur le système, pas une corruption de mesure. Documenter dans `summary.md` et investiguer (typiquement : pression cache RocksDB, compaction stall, fragmentation).
   - Corrélation intermédiaire ou ambiguë → mesure non concluante. Refaire dans des conditions thermiques mieux contrôlées avant d'investiguer la piste algorithmique.

Cette analyse n'est requise que si une dérive est observée. Pour un run `clean` au sens §8.6, l'analyse est facultative.

### 8.9 Interaction avec la matrice de validation §5

Un run avec verdict thermique `throttled` ne compte pas dans le décompte K runs requis pour les classifications "partiellement validé" ou "validé". Un run `drifted` documenté compte mais affaiblit la classification : pour une classification "validé", au moins K runs `clean` par hardware sont requis (les `drifted` ne suffisent pas à eux seuls). Un run `power_capped` est traité comme un hardware distinct au sens §5 (deux runs sur la même machine avec et sans cap RAPL ne comptent pas comme deux hardware).

### 8.10 Limites de cette extension

- **Pas de mesure de T_ambiante automatisée sur hardware desktop standard.** À documenter manuellement dans `system.json` au moment du run. Sur serveur avec IPMI ou DCMI, `ipmitool sdr` peut être inclus dans la capture continue.
- **Les capteurs NVMe varient par modèle et firmware.** Certains exposent `composite`, `sensor1`, `sensor2` séparément ; d'autres uniquement `composite`. Le script de capture doit dégrader gracieusement (capturer ce qui existe) et documenter dans `hardware.json` le nombre de capteurs disponibles.
- **Le throttling silencieux existe.** Certains contrôleurs NVMe réduisent le débit sans mettre à jour le compteur SMART de manière fiable (firmware bugs documentés sur certaines séries OEM). La contre-mesure est d'observer `delta_t` et le débit applicatif en cohérence — un débit qui chute sans T qui monte est suspect (firmware throttling préventif ou problème de file d'attente).
- **§8 ne couvre pas les VM cloud.** Sur AWS/GCP, les températures CPU/NVMe ne sont pas exposées au tenant. Pour T5-qualif sur cloud, §8 dégrade en : monitorer les métriques CloudWatch/Cloud Monitoring (`cpu_steal`, `disk_io_wait`), documenter l'instance type et l'AZ, et accepter une incertitude résiduelle sur le régime thermique. La classification "validé" requiert alors **deux** runs sur deux instance types distincts ou bare-metal en complément.

---

## 9. Évolution du protocole

Ce document est un brouillon. Il sera révisé chaque fois qu'une mesure réelle révèle un trou — par exemple, une dimension non couverte, une catégorisation ambiguë, ou une exigence trop laxe.

**Limites explicites de la version actuelle :**

- Pas de gestion de la concurrence multi-tenant (deux benchmarks tournant simultanément sur la même machine). À ajouter quand on aura un cluster CI partagé.
- Pas de cadre pour les tests destructifs (qui consomment la machine au point de la rendre inutilisable). À ajouter pour les tests de saturation P7.
- La classe `capability_class` du modèle est sous-spécifiée. À enrichir au fur et à mesure que les tests modèle-dépendants se multiplient.
- §8 (extension thermique) n'a pas encore été appliquée à un run réel. Sa première application (probablement T5-qualif N=10⁸) servira de validation du seuil de 15 °C / 10 °C / 5 % ; ces valeurs pourront être ajustées en amendement après mesure.

**Convention de révision** : toute modification de la matrice §5 ou des manifests §3 doit être faite par PR explicite, avec justification. Le format de sortie §4 et les seuils §8.6 peuvent évoluer plus librement à mesure que des données réelles s'accumulent.

---

## 10. Références

- `spec/02-properties.md` — propriétés P1–P7 (mesures cibles)
- `spec/04-hypotheses.md` §5 — plan d'invalidation et critères d'abandon
- `benchmarks/reference-workload.md` — définitions W1, W2, W3
- `benchmarks/equivalence-scenarios.md` — SEF-1 à SEF-6
- `lab/LESSONS.md` §L4, §L18–L22, §L27–L28 — leçons sur le tool-calling modèle-dépendant, mesure RocksDB, rollback, capabilities, densité
- `decisions/0002-choix-substrat.md` — choix RocksDB
- `decisions/0011-options-rocksdb-layer0.md` — options RocksDB Layer 0 (bloom filter, block cache, pas de compression)
- `decisions/0017-blobdb-cf-default-amendement-adr0010.md` — BlobDB sur CF `default`, invariant content-addressed, calibration Phase 3
- `decisions/0018-os-poc-reconstruct-minimal.md` — `os-poc-reconstruct` log-dump Phase 2
- `TODO.md` — D1–D8, T5-qualif, Phase 5

---

## Annexe — TODO post-protocole

À ajouter au `TODO.md` racine :

- ~~**ADR-0011**~~ : Options RocksDB critiques pour Layer 0 — rédigé (2026-05-14) comme référence de configuration avant T5-qualif. Capitalise L18. (Note : "ADR-0009" dans la version initiale de cette annexe était erroné — ADR-0011 est le bon numéro ; ADR-0009 = profils acteurs LLM.)
- **T5-qualif** : Organiser un run T5 à N=10⁸ sur hardware adéquat (cloud ponctuel ou prêt). Bloquant pour passer H-causal-latence de "indicatif" à "partiellement validé".
- **BlobDB — distribution d'`emit_payload`** : lors de T5-qualif, collecter l'histogramme des tailles de `emit_payload` (p50/p90/p95/p99/max/count, en bytes, sur l'ensemble du run). Requis par ADR-0017 pour calibrer le seuil `min_blob_size` Phase 3. À inclure dans `workload.json` et `summary.md` de T5.
- ~~**Protocole — extension thermique**~~ : ajoutée (§8, 2026-05-15). Première application opérationnelle prévue lors de T5-qualif N=10⁸.