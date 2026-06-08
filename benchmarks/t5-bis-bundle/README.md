# t5-bis-bundle — qualification P3b end-to-end

Bundle d'exécution one-shot du benchmark **T5-bis** (P3b, cf. `spec/02-properties.md §P3b`).

## Quel benchmark ?

T5-bis mesure le cycle complet qu'un agent en production paie avant de pouvoir relire une action qu'il vient d'émettre :

```
append_durable(entry)  → WAL fsync forcé (WriteOptions::set_sync(true))
get(action_id)          → lookup point sur l'action qu'on vient d'écrire
```

Latence chronométrée du début de `append_durable` à la fin de `get`.

**Distinction avec T5** :
- T5 (`t5-bundle/`) mesure **P3a** — lookup point seul, DB statique, borne 10 ms.
- T5-bis mesure **P3b** — cycle complet avec fsync, borne 20 ms.

## Pourquoi un harness séparé

- Bornes différentes (P3a 10 ms, P3b 20 ms).
- Profil d'écriture différent (T5 lit, T5-bis lit+écrit pendant la mesure → throttling NVMe potentiellement différent).
- Critère de pass différent dans le code de mesure (Rust : `P99_TARGET_US = 20_000`).

Le harness réutilise les probes `hardware_probe.sh` et `software_probe.sh` de `t5-bundle/` (la mesure du hardware sous-jacent est identique).

## Critère de déclenchement

T5-bis doit être exécuté **après** que les fixes harness T5 sont appliqués et P3a validé (K ≥ 5, 2 instances). Statut au 2026-05-18 : P3a **validé** (ADR-0026, K=10 conformants sur 2 classes hardware). T5-bis peut donc être lancé.

## Pré-requis hardware

- RAM ≥ 8 GB (seuil dur), 16 GB recommandé.
- NVMe persistant ≥ 1 GB/s seq read mesuré. **Pas tmpfs** — fsync sur tmpfs est un no-op, le harness refuse de tourner.
- ~15 GB libres sur `BENCH_DIR` pour la DB N=10⁸.
- Idéalement : NVMe **sans** power-loss protection embarquée (régime consumer) pour mesurer le pire cas réaliste.

## Exécution

```bash
# Exécution standard (BENCH_N=10^8, NVMe auto-détecté)
bash benchmarks/t5-bis-bundle/run.sh

# Forcer le BENCH_DIR (recommandé sur bare-metal)
T5BIS_BENCH_DIR=/mnt/nvme-bench/t5bis-bench bash benchmarks/t5-bis-bundle/run.sh

# Cycle dev rapide (N=10^6, ~1 minute)
BENCH_N=1000000 bash benchmarks/t5-bis-bundle/run.sh

# Si les paquets système sont déjà installés
SKIP_INSTALL=1 bash benchmarks/t5-bis-bundle/run.sh
```

## Sortie

`results/T5-bis/<YYYY-MM-DDTHHMMSSZ>/` contient :

- `hardware.json` — CPU, RAM, NVMe (avec débit fio mesuré)
- `software.json` — OS, kernel, rustc, rocksdb_version, git_commit, source_tree_sha256
- `workload.json` — `test: "T5-bis"`, `target_property: "P3b"`, `p99_ms_max_target: 20`
- `verdict.json` — outcome + p50/p95/p99/p99.9
- `summary.md` — résumé lisible
- `raw/bench_stdout.log` + `raw/bench_stderr.log` — sortie Cargo/bench
- `raw/iostat.txt` + `raw/cloud_io.csv` — capture iostat parsée
- une archive `t5bis-results-<TS>.tar.gz` à la racine du repo pour transfert

## Pour atteindre la classification "validé"

Protocole §5 amendé par ADR-0026 :
- **K ≥ 3 runs conformants** sur **2 classes hardware distinctes** en régime cache-mixte contraint (`drop_caches=true` ET `RAM/dataset ≤ 2×`) ou cache-miss-dominant.
- Agréger dans `results/T5-bis/SYNTHESE.md` à l'image de `results/T5/SYNTHESE.md`.

## Risque connu

Sur NVMe consumer sans power-loss protection (ex. WD SN530), fsync WAL typique 1–15 ms. Si p99 fsync mesuré > 15 ms, la borne 20 ms devient inconfortable et la spec doit être amendée plutôt que truquée. C'est précisément ce que T5-bis sert à mesurer — pas de tricherie sur la borne.
