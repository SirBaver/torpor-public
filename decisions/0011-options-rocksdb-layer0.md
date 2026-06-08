# ADR-0011 — Options RocksDB critiques pour le Layer 0 (log causal)

**Date :** 2026-05-14
**Statut :** Acceptée (provisoirement) — réévaluation obligatoire après qualification T5 à N=10⁸ sur hardware NVMe

---

## Contexte

Le Layer 0 du système (`poc/causal-log/`) est un log causal append-only stocké dans RocksDB. La propriété P3 (spec/02-properties.md §P3) exige un lookup point par `action_id` (SHA-256, 32 bytes) avec p99 ≤ 10 ms sur 10⁸ entrées. L'hypothèse H-causal-latence (spec/04-hypotheses.md §H-causal-latence) précise que cette borne est tenable sur un LSM tree correctement configuré.

Le bench T5 dev (`poc/causal-log/benches/causal_lookup.rs`) a été exécuté à N=10⁶ et donne p99 = 11 µs (lab/LESSONS.md §L19) — soit ~900× sous la cible. Cependant, ce résultat est obtenu en **régime cache chaud** : dataset ~10 MB << block cache 256 MB. Le régime cible (N=10⁸, ~10–15 GB sur disque, cache misses dominants) reste **non mesuré** faute de hardware NVMe adéquat.

Cet ADR fige les options RocksDB que le code utilise déjà (`CausalLog::open`), documente la justification de chaque option à la lumière de la littérature RocksDB et du résultat T5 dev, et énonce explicitement le régime d'incertitude qui reste avant qualification T5 officielle.

**Ce que cet ADR ne fait pas.** Il ne valide pas H-causal-latence — seule la mesure à N=10⁸ peut le faire. Il ne fige pas non plus la column family `emit` introduite par ADR-0010 (BlobDB pour payloads > 4 KiB) ; celle-ci sera couverte par un ADR distinct si nécessaire. Il ne fige pas non plus la borne de latence de P3b ; il fige la *structure* de l'index secondaire qui permettra de la mesurer (cf. section « Index secondaire `agent_ts` — structure prévue » plus bas).

---

## Décision

### Options retenues pour la column family `default` du `CausalLog`

| Option | Valeur | Effet attendu |
|--------|--------|---------------|
| `set_bloom_filter(10.0, false)` | 10 bits/clé, full filter (per-SST) | ~1 % faux positifs ; élimine la quasi-totalité des reads SST inutiles sur lookups négatifs et identifie l'SST cible sur lookups positifs |
| `Cache::new_lru_cache(256 * 1024 * 1024)` | 256 MB LRU | Sert les entrées chaudes depuis la RAM ; ratio cache/dataset à N=10⁸ ≈ 1,7 % |
| `set_cache_index_and_filter_blocks(true)` | activé | Index et filtres comptés dans le budget cache, évite double-read sur cache miss data |
| `set_pin_l0_filter_and_index_blocks_in_cache(true)` | activé | Les blocs L0 (les plus récents et consultés) ne sont jamais évictés |
| `set_compression_type(DBCompressionType::None)` | aucune | Supprime 5–20 µs de décompression par cache miss ; coût accepté : +5–10 GB disque à N=10⁸ |
| `set_write_buffer_size(64 * 1024 * 1024)` | 64 MB | Compromis entre fréquence de flushes (~16 000 à N=10⁸, ~100 B/entrée) et amplification de write |
| `optimize_level_style_compaction(512 * 1024 * 1024)` | 512 MB | Calibre L1/L2 pour minimiser write amplification ; profil leveled standard |
| `set_max_background_jobs(4)` | 4 | Parallélisme compaction/flush, valeur conservatrice pour SSD/NVMe ≥ 4 cœurs |

Ces valeurs sont **identiques** à celles déjà présentes dans `CausalLog::open` (poc/causal-log/src/lib.rs lignes 157–175) à 2026-05-14. L'ADR fige et justifie ; il ne change pas le code.

### Index secondaire `agent_ts` — structure prévue (non implémentée)

Pour permettre la consultation fenêtrée `(agent_id, ts_ms)` requise par P3b sans dégrader P3, une column family additionnelle `agent_ts` sera créée dans le même `DB` RocksDB que la CF `default`. **Cette CF n'est pas implémentée à 2026-05-14** ; sa structure est figée ici pour éviter une dérive de conception au moment de l'implémentation et pour autoriser la rédaction d'un benchmark de référence avant le code.

**Schéma de clé (56 bytes) :**

```
[ agent_id : 16 bytes ] || [ ts_ms : 8 bytes big-endian ] || [ action_id : 32 bytes ]
```

**Schéma de valeur :** bytes vides (`&[]`). L'index est une structure de navigation pure ; le contenu reste exclusivement dans la CF `default` et est récupéré par `get(action_id)` (réutilise P3, p99 ≤ 10 ms).

**Justifications par composant :**

| Composant | Choix | Justification |
|-----------|-------|---------------|
| `agent_id` en tête | 16 bytes opaques | Préfixe naturel pour scan par agent (`Iterator::seek(agent_id)` retourne toutes les entrées de l'agent en ordre temporel) |
| `ts_ms` big-endian | 8 bytes | L'ordre lexicographique RocksDB coïncide avec l'ordre temporel ; petit-endian inverserait l'ordre, forçant un scan inverse ou un tri applicatif |
| `action_id` en queue | 32 bytes | Désambiguïse les collisions à `ts_ms` égal — deux actions d'un agent dans la même milliseconde sont possibles (GPU à < 1 ms/inférence, cf. L9). Sans `action_id` en clé, la deuxième écriture écraserait la première et l'index divergerait du log principal |
| Valeur vide | 0 byte | L'`action_id` étant déjà dans la clé, il n'y a rien à stocker dans la valeur. Économie de ~32 bytes/entrée ; latence d'écriture dominée par le coût RocksDB de l'insertion, pas le payload |

**Atomicité log ↔ index :** l'écriture dans `agent_ts` doit être groupée avec l'écriture dans `default` dans un même `WriteBatch`. RocksDB garantit l'atomicité d'un batch même cross-CF [RocksDB Wiki, "Atomic flush"] : soit les deux entrées sont visibles après crash recovery, soit aucune. Sans cette garantie, un crash partiel produirait soit (a) une entrée `default` orpheline (range query incomplète, mais lookup point OK — dégradation acceptable de P3b, P3 préservée), soit (b) une entrée `agent_ts` orpheline pointant vers un `action_id` introuvable (range query retourne un identifiant inexistant — corruption observable). Le `WriteBatch` cross-CF élimine les deux cas.

**Options RocksDB de la CF `agent_ts` (à fixer au moment de l'implémentation) :**

| Option | Valeur envisagée | Note |
|--------|------------------|------|
| `set_bloom_filter` | À évaluer | Le bloom filter sert le lookup point. Pour un index dont l'usage primaire est le scan de préfixe (range), son utilité est marginale. À mesurer ; possiblement désactivé. |
| `block_cache` | Partagé avec `default` (256 MB) ou séparé | Partage simple côté code ; le scan de range bénéficie peu du cache (parcours séquentiel, pages froides). À mesurer. |
| `set_prefix_extractor(SliceTransform::create_fixed_prefix(16))` | activé | Préfixe = `agent_id` (16 bytes). Active le `prefix_bloom` (filtre de Bloom sur le préfixe agent), évite des SST reads sur scans d'agents inexistants. C'est **précisément le cas où le prefix-extractor s'applique** — par contraste avec la CF `default` où la clé SHA-256 le rend inopérant. |
| `set_compression_type` | À décider | Les clés sont fortement redondantes par préfixe (16 bytes `agent_id` partagés sur N entrées consécutives) ; la compression Zstd bottommost donnerait un gain significatif sur disque. Trade-off latence vs disque à mesurer. |

Ces options sont **indicatives** et ne sont pas figées par le présent ADR — elles seront tranchées au moment de l'implémentation, sur la base de mesures, et documentées en amendement à cet ADR (ou en ADR séparé si la décision est non-triviale, par exemple si le prefix-extractor n'est finalement pas adopté).

**Coût attendu :**

- **Espace disque :** ~56 bytes/entrée + overhead RocksDB (SST blocks, index intra-SST, compaction). Estimation conservatrice à N=10⁸ : 6–10 GB additionnels (vs 10–15 GB pour la CF `default`). Avec compression Zstd sur les niveaux profonds, possiblement 3–5 GB.
- **Latence d'écriture par `append` :** ~5 µs additionnel estimé (1 put RocksDB supplémentaire dans le même batch). À mesurer ; doit rester compatible avec la borne implicite de P1 (densité non dégradée par l'overhead par-action).
- **Mémoire :** négligeable si block cache partagé ; +overhead memtable de la CF (write_buffer_size à dimensionner — probablement 16 MB plutôt que 64 MB, le débit d'écriture est identique mais la pression mémoire séparée).

**Statut d'implémentation :** **non implémenté à 2026-05-14**. Prérequis pour la promotion de P3b en propriété bornée mesurable (cf. `spec/02-properties.md` §P3b « Action requise avant promotion »). L'implémentation déclenchera :
1. Migration de `CausalLog::open` pour ouvrir le `DB` avec deux CF.
2. Migration de `CausalLog::append` pour écrire dans les deux CF via `WriteBatch`.
3. Remplacement de `CausalLog::entries_by_agent` (scan O(N), test-only) par une méthode utilisant le scan de préfixe sur `agent_ts` (O(K) où K = entrées de l'agent dans la fenêtre).
4. Bench dédié à la range query, à intégrer dans `benchmarks/test-protocol.md` (analogue de T5 mais sur l'index).
5. Workload de référence pour P3b (fréquence, taille de fenêtre, ratio reads/writes) — `benchmarks/reference-workload.md`.

**Migration depuis l'état actuel (CF unique) :** la première ouverture d'une DB préexistante après cette migration devra peupler `agent_ts` rétroactivement par scan de la CF `default`. Coût ponctuel O(N) à la première migration ; à scripter comme outil one-shot. Tant que le PoC n'a pas de DB à préserver, ce point est trivial.

### Régime de validité

Cette décision est **provisoirement acceptée** sur la base de :
- Le résultat T5 dev (N=10⁶, p99 = 11 µs, lab/LESSONS.md §L19).
- L'analyse de littérature ci-dessous.

Elle doit être **réévaluée** dès que la qualification T5 à N=10⁸ sur NVMe ≥ 1 GB/s a produit un résultat. Critère de réévaluation déclenché : p99 mesuré > 1 ms à N=10⁸, ou ratio cache hit < 50 %.

---

## Alternatives considérées

Trois variantes (A, B, C) ont été évaluées pour la même cible (p99 ≤ 10 ms à N=10⁸). Une quatrième (D) est la configuration retenue qui combine les choix gagnants des trois précédentes.

### (A) Bloom filter + block cache géré, **sans** `cache_index_and_filter_blocks`

**Profil :** bloom 10 bits/clé, block cache LRU 256 MB. Les index et filtres sont conservés *en dehors* du budget cache (comportement RocksDB par défaut historique).

**Avantage :** index et filtres jamais évictés tant que le process vit — pas de pression de cache sur eux.

**Inconvénient :** la mémoire totale du process devient `block_cache_size + Σ(index_size + filter_size par SST ouvert)`. À N=10⁸ avec ~16 000 SSTs potentiels après compaction (estimation conservatrice : ~6 GB de niveau leaf en SSTs de ~64 MB → ~100 SSTs ; en pratique le leveled compaction réduit ce nombre), la mémoire hors-cache n'explose pas mais n'est pas bornée par configuration. Le risque est latent, pas immédiat.

**Rejet :** non rejetée fonctionnellement, mais l'absence de borne mémoire explicite est un défaut sur un système qui partage la machine avec d'autres composants (runtime WASM, scheduler). [RocksDB tuning guide, "Memory usage in RocksDB"] recommande explicitement `cache_index_and_filter_blocks=true` pour les déploiements multi-process.

### (B) Bloom filter + `cache_index_and_filter_blocks=true` + `pin_l0_filter_and_index_blocks_in_cache=true`

**Profil :** identique à (A) plus inclusion des index/filtres dans le budget cache et épinglage des blocs L0.

**Avantage :** mémoire totale bornée par `block_cache_size` (256 MB). Permet de dimensionner et de mesurer. Les blocs L0 (writes récents, les plus susceptibles d'être interrogés) sont garantis chauds.

**Inconvénient :** sous très forte pression cache, des blocs de filtre/index L1+ peuvent être évictés au profit de blocs data — un cache miss peut alors devenir « cold miss » sur filtre (1 read disque pour filtre + 1 read pour data). À 256 MB de cache pour 10⁸ entrées, la pression est élevée.

**Référence :** [Facebook RocksDB Wiki, "Block-based Table Format"], [Dong et al. 2017 FAST, "Optimizing Space Amplification in RocksDB"] pour le coût d'amplification dans le leveled compaction.

**Statut :** option dominante en termes de bornes mémoire. **Retenue.**

### (C) Compression Snappy/LZ4 activée

**Profil :** identique à (B) plus `DBCompressionType::Snappy` ou `Lz4`.

**Avantage :** réduit la taille disque de 40–60 % (entrées synthétiques sont compressibles ; entrées réelles avec hashes et payloads MessagePack le sont moins — 20–35 % est plus réaliste). À N=10⁸, gain estimé : 3–6 GB.

**Inconvénient :** ajoute 5–20 µs de décompression par cache miss [RocksDB wiki "Compression"]. À N=10⁸ avec dataset >> cache, la majorité des lookups sont des cache misses, donc cet overhead s'applique sur la p99. C'est exactement le percentile sous contrainte par P3.

**Trade-off chiffré :** marge actuelle p99 = 10 ms - 11 µs ≈ 9989 µs. Le coût de décompression représente 0,05–0,2 % de la marge si cache hit ratio reste élevé. Mais sous régime cache-miss réel, p99 dev pourrait monter à 100–500 µs (estimation L19) ; un overhead +5–20 µs reste tolérable, mais le bénéfice (disque) est secondaire à la latence sur ce système.

**Rejet :** l'objectif optimisé est la latence, pas l'espace disque. La compression peut être réactivée plus tard si l'espace disque devient contraignant *et* si la marge p99 mesurée à N=10⁸ le permet.

**Note :** envisager `Zstd` au niveau « bottommost only » (`set_bottommost_compression_type(Zstd)`) si un compromis devient nécessaire — la majorité du dataset est sur le niveau le plus bas, et les compactions y sont plus rares donc l'overhead de compression d'écriture est amorti.

### (D) Configuration retenue : (B) + compression désactivée + write buffer 64 MB + leveled compaction 512 MB

**Profil :** la combinaison effectivement utilisée par `CausalLog::open`. C'est (B) + le rejet de la compression de (C) + un calibrage du write path.

**Pourquoi (D).** Sous H-causal-latence, la métrique critique est p99 lookup. (B) borne la mémoire ; rejeter la compression supprime l'overhead de décompression sur les cache misses. Le calibrage write buffer/compaction n'affecte pas directement les lookups mais évite que `populate_synthetic` ou les insertions d'agents en charge créent des stalls d'écriture qui perturberaient les benchs (les stalls peuvent geler les reads).

---

## Conséquences

### Positives

- **Borne mémoire explicite :** 256 MB (block cache) + write buffer 64 MB + memtable secondaires + buffers de compaction (~100–200 MB sous charge) ≈ **400–500 MB** par process RocksDB. Dimensionnable et observable.
- **Latence prévisible sur cache hit :** ~5–15 µs (L19), dominée par bincode + SHA hashing côté Rust, pas par RocksDB.
- **Aucune décompression sur le chemin chaud :** la p99 sous cache miss est limitée par I/O disque seul (estimation L19 : 100–500 µs sur SSD standard, < 100 µs attendu sur NVMe).

### Négatives / coûts acceptés

- **Espace disque :** +5–10 GB à N=10⁸ comparé à Snappy. À ~$0,1/GB sur stockage local, coût négligeable ; sur stockage cloud, à reconsidérer.
- **Cache ratio à N=10⁸ : ~1,7 %.** Le block cache ne couvre qu'une fraction du dataset. La p99 réelle dépendra fortement du pattern d'accès. Si la majorité des lookups portent sur les entrées récentes (working set causal récent), la localité temporelle aide ; si les lookups sont uniformes (archivage, replay), elle ne joue pas. **Cette ambiguïté n'est pas levée tant que T5 N=10⁸ n'est pas exécuté.**
- **Pression sur les blocs L1+ :** sous forte charge, les blocs de filtre/index L1+ peuvent être évictés. Si mesuré, contre-mesures envisageables : augmenter `block_cache_size` à 512 MB ou 1 GB, ou ajouter `pin_top_level_index_and_filter` (option RocksDB ≥ 7.0).

### Neutres / à surveiller

- **Interaction avec ADR-0010 (`emit_payload` inline).** ADR-0010 §4 spécifie que les payloads > 4 KiB doivent aller en BlobDB via une column family `emit` séparée. Cette CF n'est pas encore implémentée. Quand elle le sera, l'ADR-0011 devra être étendu pour spécifier les options de cette CF (probablement : bloom filter inutile sur BlobDB, cache séparé ou partagé, à décider). **Action requise :** ADR additionnel après implémentation de la CF `emit`.
- **Index secondaire `agent_ts` non implémenté.** La structure est figée dans la section dédiée ci-dessus, mais l'implémentation, les options exactes et le bench dédié restent à produire. Tant que cet index n'existe pas, P3b reste provisoire et `CausalLog::entries_by_agent` reste un scan O(N) test-only. La promotion de P3b à propriété bornée dépend de cette implémentation **et** d'un workload de référence pour la range query.
- **Taille variable de `LogEntry` selon `parent_ids`.** Un nœud de merge avec K parents ajoute 32K bytes. L'hypothèse « ~100 bytes/entrée » dans L18/L19 suppose K ∈ {0, 1}. Si la fraction de nœuds de merge dépasse 10 % avec K moyen > 2, la taille moyenne d'entrée monte à ~200–300 bytes, et le dataset à N=10⁸ passe à 20–30 GB. Le cache ratio chute à ~0,8–1,2 %. **Action requise :** mesurer la distribution de K sur charge réaliste (T6 ou test de longévité) avant qualification T5.
- **Sensibilité aux paramètres système :** `vm.dirty_ratio`, `swappiness`, `O_DIRECT` ou non. RocksDB ne contrôle pas tout. Les mesures T5 doivent documenter ces paramètres OS (cf. `benchmarks/test-protocol.md` §6.1).

---

## Données concrètes utilisées dans la décision

### T5 dev (N=10⁶, cache chaud)
Source : `lab/LESSONS.md` §L19, machine de développement (Linux 6.17, CPU, SSD standard).

| Percentile | Latence |
|------------|---------|
| p50 | 4 µs |
| p95 | 8 µs |
| p99 | 11 µs |
| p99.9 | 18 µs |

Ces chiffres représentent la **borne inférieure** des latences réelles à N=10⁸ : ils ne contiennent quasiment aucun cache miss. Ils valident l'absence d'overhead pathologique côté code Rust (sérialisation, hashing, conversion), mais ne valident pas H-causal-latence sous son régime nominal.

### Estimations N=10⁸ (non mesurées)
- Dataset disque : 10–15 GB sans compression, 4–8 GB avec Snappy.
- Cache hit ratio attendu : 1–3 % en régime uniforme, jusqu'à 30–60 % si localité temporelle forte.
- p99 attendu sur NVMe ≥ 1 GB/s : 100–500 µs (estimation extrapolée de L19 ; à confirmer par mesure).
- p99 attendu sur SSD SATA : 500 µs – 2 ms.
- p99 sur HDD : > 10 ms → H-causal-latence réfutée. **NVMe ou SSD requis explicitement.**

---

## Références

- `poc/causal-log/src/lib.rs::CausalLog::open` — code de référence figé par cet ADR
- `poc/causal-log/benches/causal_lookup.rs` — bench T5
- `lab/LESSONS.md` §L17 (rejet SQLite), §L18 (options RocksDB), §L19 (T5 dev N=10⁶)
- `spec/02-properties.md` §P3 — Traçabilité causale (borne p99 ≤ 10 ms)
- `spec/02-properties.md` §P3b — Consultation fenêtrée (structure d'index renvoyée à cet ADR)
- `spec/04-hypotheses.md` §H-causal-latence
- `benchmarks/test-protocol.md` §6.1 — protocole T5
- ADR-0002 — choix du substrat RocksDB
- ADR-0010 — contrat `emit`, payloads inline / BlobDB
- [Dong et al. 2017, FAST] *Optimizing Space Amplification in RocksDB* — leveled compaction, write amplification
- [Facebook RocksDB Wiki] *Block-based Table Format*, *Memory usage in RocksDB*, *Compression*
- [Bloom 1970, CACM] *Space/Time Trade-offs in Hash Coding with Allowable Errors* — fondation du filtre
- [O'Neil et al. 1996, Acta Informatica] *The Log-Structured Merge-Tree (LSM-Tree)* — fondation du substrat

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
