# T5-bis-thermal — Synthèse

**Hypothèse testée** : la dégradation p99 de P3b (append_durable + get, N=10⁸) est causalement
liée à la température NVMe, via la transition SLC→TLC du SN530.

**Verdict final : RÉFUTÉ** (2026-05-23, run `2026-05-23T095915Z`)

---

## Protocole

- **Phase A** : 3 runs consécutifs sans pause. Signal attendu : Spearman(rank(p99), rank(T_max)) > 0.7.
- **Phase B** : 3 runs avec pause thermique (NVMe ≤ T_init + 5 °C, stable 30 s). Signal attendu :
  pente OLS de p99 vs run_index non significative (|b/se_b| < 1.0).
- Capteurs : sysfs sans root — `/sys/class/hwmon/hwmon3/temp1_input` (NVMe), `/sys/class/hwmon/hwmon4/temp1_input` (CPU k10temp). Échantillonnage 1 Hz.

---

## Résultats Phase A

| Run | p50 (µs) | p95 (µs) | p99 (µs) | T_NVMe_max |
|-----|----------|----------|----------|------------|
| A1  | 988      | 1 659    | 15 941   | 50.85 °C   |
| A2  | 882      | 1 494    | 16 701   | 58.85 °C   |
| A3  | 881      | 1 428    | **2 553**| 60.85 °C   |

Spearman(rank(p99), rank(T_NVMe_max)) = **−0.50** (seuil > 0.70) → **FAIL**

A3 est le run le plus chaud mais le plus rapide en p99. Explication probable : après deux runs
identiques N=10⁸, le page cache OS a chauffé les données RocksDB fréquemment lues ; les lectures
NVMe réelles (qui génèrent la latence p99) sont en partie évitées. L'effet de cache domine l'effet
thermique.

p50/p95 stables sur A1–A3 (880–988 / 1 428–1 659 µs) : le régime médian est sain et invariant.

---

## Résultats Phase B

| Run | p50 (µs) | p95 (µs) | p99 (µs) | T_NVMe_max |
|-----|----------|----------|----------|------------|
| B1  | 1 034    | 1 642    | 3 757    | 55.85 °C   |
| B2  | 1 063    | 1 693    | 6 479    | 52.85 °C   |
| B3  | 957      | 1 709    | 16 282   | 50.85 °C   |

OLS slope p99 vs run_index : b = 6 262.5, se_b = 2 044.1, |b/se_b| = **3.06** (seuil < 1.0) → **FAIL**

Résultat paradoxal : p99 *augmente* d'un run à l'autre alors que T_NVMe *diminue* grâce aux pauses.
Corrélation thermique/p99 dans Phase B : négative. Aucune causalité thermique.

p50/p95 stables sur B1–B3 (957–1 063 / 1 642–1 709 µs) — même comportement qu'en Phase A.

---

## Interprétation

La variance du p99 n'est pas thermique. La cause la plus probable est la **fenêtre de compaction L0
RocksDB** : pendant un run de ~1 446 s, RocksDB déclenche des compactions de level 0 qui
introduisent des stalls d'écriture et des spikes de latence get. Si une compaction tombe dans le
1 % de queue du run, elle fait exploser p99 ; sinon p99 reste à ~2–4 ms. Le tirage est aléatoire
et indépendant de la température — ce qui explique la variance inter-run chaotique observée.

La même cause (compaction RocksDB) est confirmée dans T6-soak : pattern en dents-de-scie RSS,
amplitude ~230 MB, période ~30–50 min (voir `results/T6/SYNTHESE.md`).

---

## Conséquences pour l'architecture

1. **Borne P3b (20 ms p99)** : toujours tenue en médiane. La borne est trop large pour être
   sensible aux stalls RocksDB (durée typique d'un stall ≪ 20 ms). Pas de révision ADR requise.

2. **Attribution du signal thermique (T5-bis 2026-05-18)** : la progression p99 3 972→12 294→
   19 644 µs sur 3 runs consécutifs (RB1→RB3) était attribuée à la thermique SLC→TLC. Ce modèle
   est réfuté : la progression reflète plutôt l'accumulation de fichiers L0 RocksDB non compactés
   à travers les runs consécutifs (le DB était partagé entre runs à l'époque, non nettoyé).

3. **Prochain test pertinent** : isoler p99 des stalls RocksDB. Options :
   - Exposer `num-files-at-level0` via l'API ContentStore et l'inclure dans le benchmark.
   - Exécuter P3b avec `Options::set_disable_auto_compactions(true)` + compaction manuelle entre
     runs, pour éliminer les stalls de compaction du signal latence.

---

## Fichiers

| Run | Résultats bruts |
|-----|-----------------|
| `2026-05-23T092223Z` | smoke test N=10⁵ Phase A seulement — plomberie |
| `2026-05-23T092356Z` | idem |
| `2026-05-23T092420Z` | idem |
| `2026-05-23T092812Z` | idem |
| `2026-05-23T094159Z` | smoke test N=10⁵ — résultat PARTIAL (métriques nulles, bug parsing) |
| `2026-05-23T095915Z` | **run complet N=10⁸ Phase A+B — verdict RÉFUTÉ** |
