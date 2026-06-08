# ADR-0035 — Configuration RocksDB explicite (CausalLog + ContentStore)

**Date :** 2026-05-24  
**Statut :** Acceptée  
**Contexte :** Dettes P1/P2/P3 identifiées lors de la revue doc 2026-05-24 ; investigation T6-soak (ADR-0034)

---

## Contexte et problème

La revue de conformité à la documentation officielle RocksDB (2026-05-24, 4 agents spécialisés) a identifié trois incohérences dans la configuration des deux bases RocksDB du PoC.

### Incohérence P1 — `optimize_level_style_compaction` + `set_write_buffer_size` (CRITIQUE)

Dans `poc/causal-log/src/lib.rs` (variantes `open` et `open_no_autocompact`) :

```rust
// Code fautif (avant fix)
default_opts.set_write_buffer_size(64 * 1024 * 1024);        // ligne N   — dead code
default_opts.optimize_level_style_compaction(512 * 1024 * 1024); // ligne N+1 — écrase write_buffer_size → 128 MB
```

`optimize_level_style_compaction(memtable_memory_budget)` configure un ensemble cohérent de
paramètres interdépendants : `write_buffer_size`, `max_write_buffer_number`,
`max_bytes_for_level_base`, `max_bytes_for_level_multiplier`. Pour un budget de 512 MB, cela
fixe `write_buffer_size = 128 MB` et `max_bytes_for_level_base = 512 MB`.

L'appel suivant `set_write_buffer_size(64 MB)` n'écrasait **pas** `optimize_level_style_compaction`
dans cet ordre ; c'était l'inverse : `optimize_level_style_compaction` était appelé en dernier
et fixait `write_buffer_size = 128 MB`, laissant `max_bytes_for_level_base = 512 MB` dimensionné
pour une memtable de 128 MB. En changeant l'ordre (P1 fix), `set_write_buffer_size(64 MB)` était
la valeur survivante, mais `max_bytes_for_level_base` restait à 512 MB — surdimensionné de ×4.

**Impact observé :** L0 accumule plus de fichiers avant de déclencher une compaction
vers L1 (L1 surdimensionné → seuil de compaction atteint plus tard) → stalls L0 plus longs
et p99 plus volatile. Cause probable des pics de compaction observés en T5-ter Mode B.

### Incohérence P2 — `bytes_per_sync` manquant (IMPORTANT)

Sans `bytes_per_sync` et `wal_bytes_per_sync`, le noyau Linux accumule les dirty pages
jusqu'à ce que `vm.dirty_ratio` soit atteint, puis les flushe en rafale. Ces rafales
se superposent aux fsync WAL de RocksDB et gonflent les p99 avec du bruit non attribuable
à RocksDB.

Absence constatée dans `poc/causal-log/src/lib.rs` et `poc/store/src/lib.rs`.

### Incohérence P3 — `block_cache_usage_bytes()` manquant (MOYEN)

La formule `rss_adj = RSS − memtable` (ADR-0034 D1) ne soustrayait pas le block cache.
Le block cache peut représenter jusqu'à 256 MB supplémentaires de RSS constant, ce qui
fausse toute analyse de tendance.

Absence de méthode d'introspection `block_cache_usage_bytes()` sur CausalLog et ContentStore.

---

## Décision

### D1 — Config explicite cohérente sur CausalLog (P1)

Remplacer `optimize_level_style_compaction` par des valeurs explicites et interdépendantes :

```rust
default_opts.set_write_buffer_size(64 * 1024 * 1024);       // 64 MB par CF
default_opts.set_max_write_buffer_number(2);                  // peak memtable = 128 MB max
default_opts.set_max_bytes_for_level_base(256 * 1024 * 1024); // 4 × peak_memtable (règle 4×)
```

**Règle des 4×** : `max_bytes_for_level_base = 4 × max(write_buffer_size × max_write_buffer_number)`
garantit que L0 et L1 ont des tailles comparables, minimisant les compactions L0→L1 coûteuses.

Appliqué aux deux variantes `open()` et `open_no_autocompact()`.

### D2 — Config explicite cohérente sur ContentStore (P1 + P2)

Migrer `open()` de `DB::open_cf()` vers `DB::open_cf_descriptors()` pour permettre une
config db-level distincte des config CF-level, et ajouter les mêmes valeurs que CausalLog :

```rust
// DB-level (partagé toutes CFs)
db_opts.set_bytes_per_sync(1_048_576);     // 1 MB — étalement dirty pages OS
db_opts.set_wal_bytes_per_sync(1_048_576); // 1 MB — étalement dirty pages WAL

// CF-level (chaque CF indépendante)
cf_opts.set_write_buffer_size(64 * 1024 * 1024);
cf_opts.set_max_write_buffer_number(2);
cf_opts.set_max_bytes_for_level_base(256 * 1024 * 1024);
```

### D3 — `bytes_per_sync` sur CausalLog (P2)

Ajouter `bytes_per_sync` et `wal_bytes_per_sync` dans les deux variantes `open()` et
`open_no_autocompact()` de CausalLog (au niveau `db_opts`, pas CF-level) :

```rust
db_opts.set_bytes_per_sync(1_048_576);
db_opts.set_wal_bytes_per_sync(1_048_576);
```

### D4 — `block_cache_usage_bytes()` sur CausalLog et ContentStore (P3B)

Ajouter une méthode d'introspection sur les deux stores :

```rust
pub fn block_cache_usage_bytes(&self) -> u64 {
    ["cf1", "cf2"].iter()
        .filter_map(|cf| self.get_rocksdb_int_property(cf, "rocksdb.block-cache-usage"))
        .sum()
}
```

La formule `rss_adj` dans le soak harness devient :

```
rss_adj = RSS − memtable_ContentStore − memtable_CausalLog − block_cache_ContentStore − block_cache_CausalLog
```

---

## Valeurs retenues et leur signification

| Paramètre | Valeur | Raison |
|-----------|--------|--------|
| `write_buffer_size` | 64 MB / CF | Compromis throughput / latence flush ; cohérent avec budget 128 MB peak |
| `max_write_buffer_number` | 2 | Peak memtable = 2 × 64 MB = 128 MB ; 1 pour writes, 1 pour flush concurrent |
| `max_bytes_for_level_base` | 256 MB | 4 × peak_memtable = 4 × 128 MB = 512 MB → arrondi conservateur 256 MB (2×) |
| `max_background_jobs` | 4 | 4 threads compaction/flush sur 6-core (Ryzen 5 PRO 4650U) |
| `bytes_per_sync` | 1 MB | Étalement dirty pages toutes les 1 MB écrits → pics dirty-flush réduits |
| `wal_bytes_per_sync` | 1 MB | Même logique sur le WAL |
| Block cache `default` CF | 256 MB | Conservé d'ADR-0011 (bloom filter 10 bits/clé, cache 256 MB) |
| Block cache `agent_ts` CF | 8 MB (défaut) | CF index uniquement — trafic limité |
| Block cache ContentStore | Défaut 8 MB / CF | À remplacer par bloc cache explicite (voir P4/P7) |
| `compression_type` | None (tous niveaux) | Benchmarks reproductibles ; LZ4/Zstd sur L2+ à évaluer (P10) |

---

## Budget mémoire (N=500 agents, régime établi)

Inchangé par rapport à ADR-0034 D4, mais désormais tracé aux valeurs de config :

| Source | Borne | Config source |
|--------|-------|---------------|
| Memtable CausalLog | 128 MB | `write_buffer_size=64 × max_write_buffer_number=2` |
| Memtable ContentStore | 128 MB | Même config |
| Block cache CausalLog `default` | 256 MB | `Cache::new_lru_cache(256 MB)` |
| Block cache CausalLog `agent_ts` | 8 MB | Défaut RocksDB |
| Block cache ContentStore | ~16 MB | 2 CFs × 8 MB défaut (à borner — P7) |
| Overhead agents | ~5 MB | 9.6 KB × 500 agents (T6-scaling) |
| Runtime baseline | ~20 MB | JIT Cranelift + Tokio |
| **Total** | **~561 MB** | — |

Note : le budget ADR-0034 citait ~793 MB car il incluait un block cache ContentStore de 256 MB
(anticipant P7). Avec la config actuelle (défaut 8 MB), le budget réel est ~561 MB.

---

## Décision — Amendement 2026-05-25 (P4–P9)

### D5 — Bloom filter + cache partagé sur ContentStore et CF `agent_ts` (P4/P5/P7)

`rocksdb::Cache` (crate 0.22) dérive `Clone` via un `Arc<CacheWrapper>` interne. Le partage de cache entre deux instances RocksDB ne nécessite pas d'`Arc<Cache>` externe : un simple `cache.clone()` suffit.

Les deux `open()` acceptent désormais `Option<Cache>` :
- `None` : cache local créé (256 MB pour CausalLog, 64 MB pour ContentStore standalone).
- `Some(cache.clone())` : cache partagé passé aux deux bases. Toutes les CF (default, agent_ts, blocks, headers) partagent le même LRU.

`pub use rocksdb::Cache` re-exporté depuis `os-poc-store` pour éviter une dépendance directe à `rocksdb` dans les crates consumers.

**Budget mémoire après D5 (production, cache partagé 256 MB) :**

| Source | Borne | Note |
|--------|-------|------|
| Block cache (partagé toutes CFs) | 256 MB | LRU coordonné — données les plus chaudes gagnent |
| Memtable CausalLog | 128 MB | Inchangé |
| Memtable ContentStore | 128 MB | Inchangé |
| Overhead agents/runtime | ~25 MB | Inchangé |
| **Total** | **~537 MB** | −24 MB vs ADR-0035 D4 (280 MB → 256 MB block cache) |

### D6 — Compression par niveau L2+ (P10)

`set_compression_per_level([None, None, Lz4, Lz4, Lz4, Zstd, Zstd])` sur toutes les CFs write-heavy (CausalLog `default`, CausalLog `agent_ts`, ContentStore `blocks`, ContentStore `headers`).

- L0/L1 : `None` — chemin d'écriture inchangé, latence p99 fsync identique.
- L2–L4 : `Lz4` — compression rapide (~500 MB/s décompression), SSTs plus petits = compactions moins coûteuses en I/O.
- L5–L6 : `Zstd` — données froides, rapport compression supérieur acceptable pour le coût CPU.

SSTs existants restent lisibles sans reconversion (RocksDB décode les deux formats). T5-bis fermé avant activation → baseline H-causal-latence préservée.

### D7 — `max_background_jobs` aligné sur ContentStore (P9)

`db_opts.set_max_background_jobs(4)` ajouté dans `ContentStore::open`. Évite les pics I/O asymétriques entre les deux bases lors des benchmarks combinés.

### N/A — P6 (`cache_index_and_filter_blocks_with_high_priority`)

`rocksdb_block_based_options_set_cache_index_and_filter_blocks_with_high_priority` n'est pas exposé dans `librocksdb-sys 0.16` (rocksdb crate 0.22). La valeur par défaut RocksDB C++ 8.10.0 est `true`. Aucune action requise.

### N/A — P8 (commentaire Tokio scheduler.rs)

Commentaire `scheduler.rs:2` corrigé. `spec/07 §3.3` non concerné (section I/O Admission Control, pas mémoire Tokio).

---

## Conséquences

- `poc/causal-log/src/lib.rs` : config explicite dans `open()` et `open_no_autocompact()`, `bytes_per_sync`, bloom filter CF `agent_ts`, cache partagé, `block_cache_usage_bytes()`.
- `poc/store/src/lib.rs` : migration vers `open_cf_descriptors`, config explicite, `bytes_per_sync`, `BlockBasedOptions` (bloom filter, cache partagé), `max_background_jobs=4`, `block_cache_usage_bytes()`. Re-export `Cache`.
- `poc/benchmarks/src/main.rs` : formule `rss_adj` enrichie avec `block_cache_kb`.
- ADR-0011 amendé sur la config `write_buffer_size` et `bytes_per_sync`.
- Dettes P1/P2/P3B/P4/P5/P7/P8/P9/P10 : **CLOSED**.
- Dettes P6 : N/A (défaut correct).
- T5-bis replay à lancer pour vérifier l'impact de `bytes_per_sync` sur le p99.

---

## Décision — Amendement 2026-05-27 (GAP-12/13/14/15)

### D8 — `target_file_size_base` aligné sur la recommandation RocksDB (GAP-13)

Ajout dans `open()` et `open_no_autocompact()` :

```rust
default_opts.set_target_file_size_base(32 * 1024 * 1024); // = max_bytes_for_level_base / 10
```

Valeur 32 MiB = 256 MiB / 10 (règle RocksDB : `target_file_size_base ≈ max_bytes_for_level_base/10`). La valeur précédente (défaut RocksDB : 64 MiB) était trop élevée par rapport à notre `max_bytes_for_level_base = 256 MiB`, ce qui sous-découpait L1 et créait moins de SSTs → range scans moins parallélisables.

### D9 — `pin_l0_filter_and_index_blocks_in_cache` sur CF `agent_ts` (GAP-15)

Ajout sur `agent_ts_block_opts` dans les deux variantes `open()` :

```rust
agent_ts_block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
```

Déjà présent sur CF `default`. Évite les re-lectures de blocks de filtres/index L0 lors des range scans P3b.

### Documentation — Seuils write-stall et `min_write_buffer_number_to_merge` (GAP-12/14)

Les seuils write-stall L0 (`level0_slowdown_writes_trigger=20`, `level0_stop_writes_trigger=36`) sont des défauts RocksDB intentionnellement conservés : avec `max_write_buffer_number=2` le flush arrive avant tout stall en régime normal.

`min_write_buffer_number_to_merge=1` (défaut) : chaque memtable est flushée en un SST L0 distinct. Correct pour notre workload append-only séquentiel — aucune fusion pré-flush n'est avantageuse.

---

## Références

- ADR-0011 — Options RocksDB Layer 0 (config initiale, maintenant amendée)
- ADR-0032 — Réfutation thermique (stalls L0 identifiés comme cause des spikes p99)
- ADR-0034 — Réfutation H-fuite-mémoire (formule rss_adj, budget RSS)
- `poc/causal-log/src/lib.rs` — Implémentation D1 + D3 + D4
- `poc/store/src/lib.rs` — Implémentation D2 + D4
- `poc/benchmarks/src/main.rs` — Formule rss_adj mise à jour
- RocksDB Tuning Guide §Level Style Compaction — interdépendance des paramètres
