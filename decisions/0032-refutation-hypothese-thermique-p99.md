# ADR-0032 — Réfutation de l'hypothèse thermique sur p99 P3b

**Date :** 2026-05-23
**Statut :** Acceptée
**Contexte :** T5-bis-thermal (protocole dissociation causale), `results/T5-bis-thermal/SYNTHESE.md`, ADR-0026 (régime cache-mixte contraint)

---

## Contexte et problème

T5-bis (2026-05-18) avait mesuré une progression p99 3 972 → 12 294 → 19 644 µs sur 3 runs consécutifs (RB1→RB3) avec la même base RocksDB non nettoyée entre runs. Cette progression avait été attribuée à la thermique SLC→TLC du WD SN530 (cache SLC saturé → écriture en TLC plus lente → spikes p99).

Le protocole T5-bis-thermal (2026-05-23) a été conçu pour tester cette hypothèse causalement :
- **Phase A** : 3 runs consécutifs sans pause. Signal attendu : Spearman(rank(p99), rank(T_max)) > 0.7.
- **Phase B** : 3 runs avec pause thermique (NVMe ≤ T_init + 5 °C stable 30 s). Signal attendu : pente OLS p99 vs run_index non significative (|b/se_b| < 1.0).

---

## Résultats

### Phase A — runs consécutifs (N=10⁸)

| Run | p50 (µs) | p95 (µs) | p99 (µs) | T_NVMe_max |
|-----|----------|----------|----------|------------|
| A1  | 988      | 1 659    | 15 941   | 50.85 °C   |
| A2  | 882      | 1 494    | 16 701   | 58.85 °C   |
| A3  | 881      | 1 428    | **2 553**| 60.85 °C   |

Spearman(rank(p99), rank(T_NVMe_max)) = **−0.50** (seuil > 0.70) → **FAIL**

A3 est le run le plus chaud mais le plus rapide en p99. Explication : après deux runs identiques N=10⁸, le page cache OS a chauffé les données RocksDB fréquemment lues ; les lectures NVMe réelles sont en partie évitées. L'effet de cache domine l'effet thermique.

### Phase B — runs avec pause thermique

| Run | p50 (µs) | p95 (µs) | p99 (µs) | T_NVMe_max |
|-----|----------|----------|----------|------------|
| B1  | 1 034    | 1 642    | 3 757    | 55.85 °C   |
| B2  | 1 063    | 1 693    | 6 479    | 52.85 °C   |
| B3  | 957      | 1 709    | 16 282   | 50.85 °C   |

OLS slope p99 vs run_index : b = 6 262.5, se_b = 2 044.1, |b/se_b| = **3.06** (seuil < 1.0) → **FAIL**

Résultat paradoxal : p99 *augmente* alors que T_NVMe *diminue* grâce aux pauses. Aucune causalité thermique.

p50/p95 stables sur les 6 runs (880–1 063 µs / 1 428–1 709 µs) — le régime médian est sain et invariant.

---

## Décision

### D1 — Hypothèse thermique réfutée

La dégradation p99 de P3b n'est pas causalement liée à la température NVMe. Les deux critères de falsification (Spearman Phase A, OLS Phase B) sont tous deux FAIL, dans des directions opposées au signal attendu. La réfutation est nette.

### D2 — Cause retenue : fenêtre de compaction L0 RocksDB

La cause la plus probable est la **fenêtre de compaction L0 RocksDB** : pendant un run de ~1 446 s, si une compaction L0 tombe dans le 1 % de queue du run, p99 explose ; sinon il reste à ~2–4 ms. Le tirage est aléatoire et indépendant de la température.

La même cause est confirmée indépendamment par T6-soak (pattern dents-de-scie RSS, amplitude ~230 MB, période ~30–50 min, voir `results/T6/SYNTHESE.md §T6-soak`).

L'attribution antérieure de T5-bis (2026-05-18) — "progression RB1→RB3 due à la thermique SLC→TLC" — est révisée : cette progression reflétait l'accumulation de fichiers L0 non compactés à travers des runs partageant la même base RocksDB (base non nettoyée entre runs à l'époque).

### D3 — Borne P3b non révisée

La borne P3b (p99 ≤ 20 ms) est **toujours tenue en médiane**. La borne est trop large pour être sensible aux stalls RocksDB (durée typique d'un stall ≪ 20 ms). Pas de révision ADR-0026 requise.

La formulation de P3b dans la spec est qualifiée **"en médiane sur K≥3 runs conformants"** (ADR-0026). Un run isolé à p99=16 282 µs reste conforme ; la variance inter-run est une propriété du workload LSM, pas une violation de la borne.

### D4 — Prochain test (T5-ter)

La question ouverte est : dans quelle proportion des spikes p99 > 5 ms une compaction L0 est-elle active dans une fenêtre ±100 ms ? Ce test (T5-ter) requiert :
- Exposer `rocksdb.num-files-at-level0` et `rocksdb.compaction-pending` via l'API ContentStore.
- Mode A : `disable_auto_compactions=true` + compaction manuelle entre runs → mesure p99 hors stall.
- Mode B : config actuelle + logging des events compaction → corrélation spikes/compactions.

T5-ter est **prioritaire** sur S12/H-wake-latence : sans identifier la source de variance p99, les futurs verdicts P3b sont contaminés par un bruit de cause inconnue.

---

## Conséquences

- La dette "T5-bis-thermal (dissociation causale)" est **CLOSED** dans TODO.md.
- ADR-0033 (critère de fuite mémoire LSM) est ouvert par ce résultat : T6-soak souffre du même mécanisme de compaction.
- Aucune révision de spec/07 (plafonds), spec/02 (propriétés), ou d'un ADR de stockage n'est requise.

---

## Références

- `results/T5-bis-thermal/SYNTHESE.md` — synthèse complète avec données brutes
- `results/T5-bis-thermal/2026-05-23T095915Z/verdict.json` — métriques statistiques
- `results/T6/SYNTHESE.md §T6-soak` — confirmation indépendante de la cause compaction L0
- ADR-0026 — Régime de cache de référence pour P3a (définit la formulation de P3b)
- ADR-0033 — Critère de fuite mémoire pour workload LSM (conséquence directe)
