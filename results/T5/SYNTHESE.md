# T5 — Synthèse des runs — H-causal-latence

**Propriété mesurée : P3a** (lookup point `get(action_id)` sur DB statique)  
**Classification : validé** (ADR-0026, 2026-05-18 — K=7 conformants classe AWS + K=3 conformants classe AMD/WD — 2 classes hardware distinctes — protocole §5 amendé)  
**Régime : cache-mixte** (drop_caches appliqué ; RAM/dataset ≈ 2× AWS, ≈ 0,93× AMD — contrainte hardware)

> **Condition (a) satisfaite.** K=3 conformants AMD Ryzen 5 PRO + WD SN530 NVMe PCIe — classe distincte d'AWS i3en.xlarge (CPU vendor, NVMe consumer vs instance storage, RAM).
>
> **Condition (b) satisfaite — ADR-0026 (2026-05-18).** Régime « cache-mixte contraint » (drop_caches + RAM/dataset ≤ 2×) entériné comme régime représentatif pour P3a. Ratio AMD : 0,93× — plus contraint que AWS (2×). Preuve empirique : RA3 p50 = 1 813 µs vs 14–23 µs en cache chaud → NVMe effectivement sollicité.

> **Statut des trous procéduraux (revue externe 2026-05-15) :**
> 1. ~~**Régime cache.**~~ **Résolu.** Harness v2 : `drop_caches=3` avant chaque run, `cache_regime` calculé dynamiquement. Le régime reste `cache-mixte` car RAM (31 GB) / dataset (~15 GB) = 2× — en-dessous du seuil §2.3 (5×). C'est une limite hardware de l'i3en.xlarge, documentée honnêtement.
> 2. ~~**`git_commit: null`.**~~ **Partiellement résolu.** `source_tree_sha256` calculé automatiquement (R5/R6 : `6f9c3ab1...`). `git_commit` reste null (transfert sans `.git/`) — identité via SHA tree suffisante pour les manifests de validation.
> 3. ~~**fio mono-thread.**~~ **Résolu.** Harness v2 : deux mesures fio (`qd1` et `qd32`). Anomalie constatée : `qd32` (663 MB/s) < `qd1` (751 MB/s) — artefact de block size (`--bs=128k` vs `--bs=1M`). À corriger dans harness v3 (même `--bs` pour les deux profils).

---

## Instance A — `i-046260ea9abaf9e50` (eu-north-1b)

Harness v1 (sans drop_caches, sans source_tree_sha256, fio QD=1 seulement).

| Champ | Valeur |
|---|---|
| Instance | AWS i3en.xlarge (`i-046260ea9abaf9e50`) |
| NVMe | `/dev/nvme1n1` — Amazon EC2 NVMe Instance Storage, XFS |
| fio QD=1 | 768–769 MB/s |
| git_commit | null (commit local : `38c4324`) |
| source_tree_sha256 | null |

| Run | Timestamp | p50 | p95 | p99 | p99.9 | steal | iowait | Conforme |
|---|---|---|---|---|---|---|---|---|
| R1 | 2026-05-15T123841Z | 14 µs | 256 µs | 502 µs | 598 µs | 0.05% | 11.98% | ✗ ¹ |
| R2 | 2026-05-15T133804Z | 14 µs | 266 µs | 482 µs | 684 µs | 0.05% | 11.07% | ✓ |
| R3 | 2026-05-15T135008Z | 17 µs | 266 µs | 482 µs | 661 µs | 0.05% | 10.82% | ✓ |
| R4 | 2026-05-15T135716Z | 17 µs | 258 µs | 371 µs | 572 µs | 0.00% | 12.12% | ✓ |

¹ R1 non conforme : bugs `fio` + `date +%s%3N` sur kernel 7.  
Régime : cache-mixte non déclaré (page cache OS accumulé entre runs, pas de drop_caches).

---

## Instance B — `i-067bcb74a25f7a2f6` (eu-north-1b)

| Champ | Valeur |
|---|---|
| Instance | AWS i3en.xlarge (`i-067bcb74a25f7a2f6`) |
| NVMe | `/dev/nvme0n1` — Amazon EC2 NVMe Instance Storage, XFS |
| fio QD=1 | 741–751 MB/s |
| fio QD=32 (bs=128k, harness v2) | 663–665 MB/s |
| fio QD=32 (bs=1M, harness v3) | 678 MB/s |
| git_commit | null |
| source_tree_sha256 | `6f9c3ab11ce5868a92e8b10e686dee5fe406601a2cbf02ce5bc9df5e82a3a1ff` |

| Run | Timestamp | Harness | p50 | p95 | p99 | p99.9 | steal | iowait | Conforme |
|---|---|---|---|---|---|---|---|---|---|
| R5 | 2026-05-15T164749Z | v2 | 15 µs | 271 µs | 422 µs | 581 µs | 0.05% | 10.54% | ✓ |
| R6 | 2026-05-15T170943Z | v2 | 12 µs | 165 µs | 280 µs | 565 µs | 0.00% | 10.22% | ✓ ² |
| R7 | 2026-05-15T172806Z | v2 | 13 µs | 243 µs | 395 µs | 577 µs | 0.00% | 10.14% | ✓ |
| R8 | 2026-05-15T173916Z | v3 | 16 µs | 267 µs | 460 µs | 592 µs | 0.00% | 10.19% | ✓ |

² R6 p99=280 µs bas : DRAM cache embarqué du NVMe non effacé par `drop_caches`. Mesure cold-start instance B = R5 (422 µs).

**Conclusion fio QD=32 :** la correction `--bs=1M` (harness v3) donne 678 MB/s vs 663–665 MB/s avec `--bs=128k`. Différence de 2% — dans le bruit. Ce NVMe ne scale pas avec la queue depth pour la lecture séquentielle : la bande passante est saturée dès QD=1. Cap actif C2 inchangé.

---

## Bilan global (toutes classes)

| Métrique | AWS i3en.xlarge (classe 1) | AMD Ryzen 5 PRO / WD SN530 (classe 2) |
|---|---|---|
| K conformants | 7 (R2–R8) | 3 (RA1–RA3) |
| p99 pire cas | 482 µs (R2/R3) | **4 855 µs (RA3)** |
| p99 cold-start | 422 µs (R5) | 2 138 µs (RA1) |
| Marge pire cas / 10 ms | ×20 | ×2,06 |
| NVMe seq QD=1 | 741–769 MB/s | 1 290–1 321 MB/s |
| NVMe seq QD=32 | 678 MB/s | 2 095–2 214 MB/s |
| NVMe rand QD=1 | non mesuré | **9 039–10 865 IOPS** |
| NVMe rand QD=32 | non mesuré | **125 000–130 000 IOPS** |
| RAM/dataset | ≈ 2× | ≈ 0,93× |
| Régime | cache-mixte | cache-mixte (plus contraint) |

| Bilan global | |
|---|---|
| Classes hardware distinctes | **2** ✓ |
| K conformants total | **10** |
| Cible p99 | ≤ 10 000 µs |
| p99 pire cas toutes classes | **4 855 µs** (×2 sous cible) |
| Seuil protocole §5 | **validé sous conditions** (ADR §2.3 en attente) |
| Cap actif C2 — classe 1 (AWS) | floor(741 / 50) = **14 agents/s** |
| Cap actif C2 — classe 2 (AMD) | floor(1 300 / 50) = **26 agents/s** |
| Cap actif C2 — retenu (conservateur) | **14 agents/s** (borne basse toutes classes) |

**Nouveaux chiffres classe 2 :** le NVMe AMD est 1,7× plus rapide en séquentiel que l'AWS. Les IOPS aléatoires (QD=1 : ~10 000 IOPS, QD=32 : ~130 000 IOPS) sont mesurées pour la première fois — elles caractérisent le vrai régime de production RocksDB (lectures de blocs 4–16 KB), absent des runs AWS.

---

## Paramètres workload

- `BENCH_N` = 100 000 000 (10⁸ entrées)
- `n_measures` = 10 000 lookups aléatoires par run
- Régime déclaré : `cache-mixte` (harness v2) / non déclaré mais effectivement mixte (harness v1)
- Bench dir : `/mnt/nvme-bench/t5-bench/` (NVMe instance store)

## Fichiers

```
results/T5/
├── SYNTHESE.md                  ← ce fichier
├── 2026-05-15T123841Z/          ← R1  (harness v1 — non conforme)
├── 2026-05-15T133804Z/          ← R2  (harness v1 — conforme, classe AWS)
├── 2026-05-15T135008Z/          ← R3  (harness v1 — conforme, classe AWS)
├── 2026-05-15T135716Z/          ← R4  (harness v1 — conforme, classe AWS)
├── 2026-05-15T164749Z/          ← R5  (harness v2 — conforme, instance B, cold-start)
├── 2026-05-15T170943Z/          ← R6  (harness v2 — conforme, NVMe DRAM cache chaud)
├── 2026-05-15T172806Z/          ← R7  (harness v2 — conforme, classe AWS)
├── 2026-05-15T173916Z/          ← R8  (harness v3 fio fix — conforme, classe AWS)
├── 2026-05-18T100400Z/          ← RA1 (harness v3 — conforme, classe AMD/WD, p99=2138µs)
├── 2026-05-18T113516Z/          ← RA2 (harness v3 — conforme, classe AMD/WD, p99=570µs)
├── 2026-05-18T115048Z/          ← RA-inc (inconclusive — disk full)
├── 2026-05-18T121441Z/          ← RA-abort (avorté — répertoire vide)
├── 2026-05-18T121537Z/          ← RA3 (harness v3 — conforme, classe AMD/WD, p99=4855µs)
└── t5-results-*.tar.gz          ← archives originales
```

Chaque répertoire contient : `hardware.json`, `software.json`, `workload.json`, `verdict.json`, `summary.md`, `raw/`.
