# ADR-0033 — Critère de fuite mémoire pour workload LSM

**Date :** 2026-05-23
**Statut :** Acceptée
**Contexte :** T6-soak FAIL OLS (2026-05-23), ADR-0032 §D2 (cause compaction L0), `results/T6/SYNTHESE.md §T6-soak`

---

## Contexte et problème

T6-soak (run N=500 agents × 4h, 7 147 500 messages) a produit un verdict **FAIL** sur critère OLS :
- Pente RSS brute : 1 067.8 KB/min vs seuil 80.6 KB/min (×13).

Mais l'analyse du signal montre que la croissance RSS est entièrement expliquée par le cycle flush/compaction RocksDB : write buffers 500 agents × 1 write/s → ~26 MB/min de memtable → flush SST toutes les 30–50 min → chute RSS 150–230 MB. Les baselines post-compaction sont stables sur 4h. L'overhead/agent mesuré après chaque compaction est stable à 9.7 KB (cohérent avec T6-scaling).

**Problème fondamental :** le critère OLS sur RSS brut mesure le rythme de remplissage des memtables RocksDB (comportement attendu et borné d'un LSM tree), pas une fuite applicative. Ce critère est structurellement inadapté à tout workload write-intensif sur LSM.

L'accepter comme "PASS conditionnel sur la base des baselines stables" installe un précédent : un FAIL avec interprétation post-hoc devient un PASS. Ce précédent est inacceptable ; chaque futur FAIL aurait une belle histoire.

---

## Options considérées

### Option (a) — OLS sur baselines post-compaction
Filtrer le signal RSS aux points temporels post-flush (après chaque chute RSS ≥ 50 MB), puis appliquer OLS sur ce sous-échantillon.

- Avantage : simple à implémenter avec le signal RSS déjà collecté.
- Inconvénient : le filtrage est post-hoc et dépend d'un seuil arbitraire (50 MB). En cas de compaction fréquente ou irrégulière, le sous-échantillon peut être trop petit ou biaisé.

### Option (b) — OLS sur RSS ajusté (RSS − memtable_usage)
Borner explicitement `write_buffer_size × max_write_buffer_number` dans la config RocksDB du PoC, et mesurer OLS sur `RSS − rocksdb.cur-size-all-mem-tables` (exposé via l'API `GetIntProperty`).

- Avantage : la borne est connue et constante ; la soustraction est triviale et déterministe ; on mesure exactement ce qu'on cherche (croissance hors-cache LSM).
- Inconvénient : nécessite d'exposer `cur-size-all-mem-tables` depuis le ContentStore au harness de benchmark.

---

## Décision

### D1 — Adopter l'Option (b)

Le critère de fuite mémoire pour workload LSM est :

```
rss_adj(t) = rss(t) − cur_size_all_mem_tables(t)
OLS(rss_adj, t) : pente b ≤ seuil_b
```

Avec :
- `cur_size_all_mem_tables` = valeur de la propriété RocksDB `rocksdb.cur-size-all-mem-tables` (entier, octets), exposée via `ContentStore::get_int_property("rocksdb.cur-size-all-mem-tables")`.
- `seuil_b` : recalibré sur le run de référence T6-scaling (9.7 KB/agent, N=500) — borne = overhead/agent × N × tolérance (ex. 1 % de drift/h → ~48.5 KB/h ≈ 0.8 KB/min pour N=500).

### D2 — Borner write_buffer_size dans la config PoC

Ajouter dans la config RocksDB du PoC (`poc/runtime/src/content_store.rs` ou équivalent) :

```rust
opts.set_write_buffer_size(64 * 1024 * 1024);       // 64 MB par memtable
opts.set_max_write_buffer_number(2);                  // 2 memtables max
// borne mémoire memtables = 128 MB (connue, constante)
```

Ces valeurs sont déjà raisonnables pour le workload T6-soak ; les documenter explicitement rend la borne calculable et reproductible.

### D3 — Exposer cur-size-all-mem-tables depuis ContentStore

Ajouter une méthode `get_rocksdb_int_property(name: &str) -> Option<u64>` à `ContentStore`, wrappant `DB::property_int_value`. Le harness T6-soak l'appelle à la même fréquence que le sampling RSS (1 Hz) et log la paire `(rss, memtable_usage)` dans le fichier de mesures.

### D4 — Re-run T6-soak avec le nouveau critère

Un re-run de T6-soak (N=500, 4h) est requis avec le critère D1 pour produire un verdict binaire propre. Si PASS → hypothèse H-fuite-mémoire validée. Si FAIL → investigation de vraie fuite (profiler Valgrind/heaptrack sur le runtime Wasmtime + RocksDB).

### D5 — La même instrumentation couvre T5-ter

L'exposition de `num-files-at-level0` requise par T5-ter (ADR-0032 §D4) utilise le même mécanisme `get_rocksdb_int_property`. Implémenter D3 une fois, l'utiliser dans les deux harnesses.

---

## Conséquences

- `ContentStore` : nouvelle méthode `get_rocksdb_int_property`.
- Harness T6-soak : sampling de `memtable_usage` à 1 Hz, calcul `rss_adj`, OLS sur `rss_adj`.
- Config RocksDB : `write_buffer_size` et `max_write_buffer_number` explicitement bornés et documentés.
- Verdict T6-soak courant : **invalide** (critère inadapté). Remplacé par le verdict du re-run.
- La dette T6-soak reste **ouverte** jusqu'au verdict du re-run.

---

## Ordre d'implémentation

1. `ContentStore::get_rocksdb_int_property` (couvre T6-soak + T5-ter).
2. Mise à jour harness T6-soak (sampling `memtable_usage`, calcul `rss_adj`, seuil calibré).
3. Re-run T6-soak 4h → verdict.
4. T5-ter Mode A + Mode B (utilise le même mécanisme pour `num-files-at-level0`).

---

## Références

- `results/T6/SYNTHESE.md §T6-soak` — run original, analyse du signal RSS
- `lab/LESSONS.md §L55` — leçon OLS inadapté sur workload LSM
- ADR-0032 §D2 — cause compaction L0 (contexte commun)
- ADR-0002 — Choix substrat (RocksDB comme LSM de référence)
- `poc/runtime/src/content_store.rs` — site d'implémentation de `get_rocksdb_int_property`
