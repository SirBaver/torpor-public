# ADR-0002 — Choix du substrat pour le PoC

**Date :** 2026-05-10
**Statut :** Acceptée

---

## Contexte

`spec/02b-substrate_requirements.md` définit sept exigences S1–S7 que le substrat doit satisfaire pour héberger les propriétés P1–P6. `spec/03-state-of-the-art.md` a évalué les familles candidates contre cette grille. Il faut choisir le substrat du PoC pour commencer l'expérimentation.

Les quatre familles survivantes après filtrage S1–S7 sont :

| Famille | S1 | S2 | S3 | S4 | S5 | S6 | S7 | Disponibilité PoC |
|---------|----|----|----|----|----|----|-----|-------------------|
| Runtime acteur typé (BEAM-inspired, à construire) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | Non — 12–18 mois |
| WASM Component Model + WASI Preview 2 | ✓(b) | ✓ | △ | ✓ | ✓ | ✓(b) | ✓ | Oui — Wasmtime stable |
| seL4 + couche acteur | ✓ | ✓ | △ | ✓ | ✓ | △ | ✓ | Non — 6–12 mois |
| CHERI + microkernel | ✓ | ✓ | — | ✓ | ✓ | ✓ | ✓ | Non — 2028+ |

Le runtime acteur typé idéal n'existe pas encore. Le PoC doit valider les hypothèses bloquantes (H-rollback-latence, H-causal-latence, H-commit-barrier) sans attendre 18 mois.

## Décision

Le substrat retenu pour le PoC est :

> **Wasmtime (WASM Component Model + WASI Preview 2) + scheduler Rust async (Tokio) + RocksDB content-addressed + log causal append-only**

Wasmtime joue le rôle de **bac à sable d'isolation** (S1b, S4, S6b) et de **frontière d'effets** (interception WASI pour S4). Tokio fournit la séquentialité par acteur (S5) et le scheduling (S7). RocksDB fournit S2 (capture d'état cohérente) et supporte P2 + P3.

L'ensemble est écrit en **Rust** pour contrôler l'overhead mémoire (cible P1) et avoir accès aux deux librairies sans friction.

## Alternatives considérées

| Alternative | Raison du rejet |
|-------------|-----------------|
| BEAM/Erlang production | NIFs brisent S1 ; pas de store content-addressed natif ; P3 nécessite un log externe. Garde son rôle de **Référence** pour calibrer H-densité (benchmark W1 avant construction). |
| seL4 + couche acteur | Temps de mise en place > 6 mois pour un PoC exploratoire ; S3 (observabilité causale) non fourni par seL4 et à construire de zéro. |
| Runtime acteur typé custom | Objectif final, pas point de départ. Construire le substrat et valider les hypothèses en parallèle est trop risqué. |
| Python + asyncio | Overhead mémoire du runtime Python ≫ P1 ; pas de contrôle de l'isolation S1 ; non représentatif d'un substrat de production. |
| SQLite comme moteur Layer 0 | Rejet **architectural**, indépendant du langage hôte : B-tree row-based + planificateur relationnel inadaptés à un log append-only à lookup par clé opaque. Détaillé en **§ Choix du moteur de stockage Layer 0** ci-dessous. |

## Choix du moteur de stockage Layer 0

La décision ci-dessus nomme RocksDB comme moteur du store et du log causal (Layer 0). Le présent ADR comparait des **familles de substrat** (BEAM, seL4, Python) ; il ne comparait pas les **moteurs KV embarqués** entre eux. Cette section comble cette lacune : elle établit pourquoi RocksDB est retenu face aux trois autres candidats embarqués sérieux (SQLite, LevelDB, LMDB), sur des critères structurels et non sur des chiffres de benchmark non sourçables.

### Profil d'usage du Layer 0

Le Layer 0 (`poc/causal-log/`, `poc/store/`) a un profil d'accès précis, défini par les propriétés P2 (store content-addressed) et P3 (traçabilité causale, `spec/02-properties.md`) :

- **Append-only.** Les entrées (log causal, blobs content-addressed) sont écrites une fois et jamais mises à jour ni supprimées en place. Pas de `UPDATE`, pas de mutation de ligne.
- **Lookup point par clé opaque.** L'accès dominant est `get(action_id)` ou `get(content_hash)` où la clé est un SHA-256 (32 bytes), uniformément distribué, sans structure exploitable. Pas de JOIN, pas de range query relationnelle ; le seul range scan envisagé est un scan de préfixe `agent_id` sur l'index secondaire (ADR-0011 §`agent_ts`).
- **Pas de sémantique relationnelle.** Aucune contrainte d'intégrité référentielle SQL, aucune requête multi-table, aucun planificateur.
- **Write-heavy.** Le débit d'écriture suit le débit d'émission d'actions des agents (GPU < 1 ms/inférence, cf. L9) ; les écritures dominent les lectures sur le chemin nominal.

### Critères de sélection

| Critère | Pourquoi il compte pour le Layer 0 |
|---------|-------------------------------------|
| Structure adaptée à l'append-only write-heavy | Le chemin nominal est l'écriture. Un moteur optimisé pour l'écriture séquentielle batchée (memtable → flush) plutôt que pour l'écriture en place (mutation de page B-tree) minimise l'amplification de write et le coût par `append`. |
| Lookup O(1) amorti par clé opaque | P3 borne le lookup point (p99 ≤ 10 ms à N=10⁸). La clé étant un hash uniforme, aucune localité n'est exploitable hors cache : il faut un filtre probabiliste (bloom) pour éviter les reads SST inutiles. |
| Bloom filter configurable | Sur clé hash uniforme, le bloom filter par SST est le seul mécanisme qui évite O(L) reads disque sur lookup négatif et localise l'SST cible sur lookup positif (ADR-0011 §Décision, L18). |
| Column families | Le Layer 0 a besoin de namespaces de stockage séparés dans une seule DB transactionnelle : CF `default` (log causal), index secondaire `agent_ts` (ADR-0011), payloads BlobDB (ADR-0010/0017). L'atomicité cross-CF d'un `WriteBatch` est requise pour l'invariant log ↔ index (ADR-0011 §Atomicité). |
| Bindings Rust matures | Le substrat est écrit en Rust (cf. §Décision). Un binding maintenu, production-grade, sans FFI fragile, est requis. |
| Production-grade | Le PoC doit produire des mesures interprétables et transférables ; un moteur alpha ou single-purpose disqualifie les chiffres. |

### Comparatif des candidats

| Moteur | Structure | Append-only write-heavy | Bloom filter | Column families | Bindings Rust | Verdict |
|--------|-----------|--------------------------|--------------|-----------------|---------------|---------|
| **SQLite** | B-tree row-based + planificateur SQL | Inadapté (écriture en place, overhead relationnel) | Non | Non (tables, mais pas de CF KV avec atomicité de batch équivalente) | `rusqlite` (mature) | **Rejeté** |
| **LevelDB** | LSM tree | Adapté | Non configurable (filtre interne non paramétrable par CF) | Non | bindings tiers, peu maintenus | **Rejeté** |
| **LMDB** | B+tree MVCC copy-on-write | Inadapté (write-amplification sous insertion soutenue) | Non (sans objet sur B+tree mmap) | Sous-DB nommées, mais modèle single-writer | `heed`/`lmdb-rs` (matures) | **Rejeté** |
| **RocksDB** | LSM tree | Adapté (memtable → flush batché) | Oui, configurable par CF (bits/clé, full vs block-based) | Oui, avec `WriteBatch` atomique cross-CF | `rocksdb` (mature, production-grade) | **Retenu** |

### Justification des rejets

**SQLite — rejet architectural (formalise L17).**
SQLite est un B-tree row-based piloté par un planificateur de requêtes relationnel : il est conçu pour les `UPDATE` en place et les `JOIN`, pas pour un log append-only à lookup par clé opaque. Le profil du Layer 0 n'utilise aucune de ses forces (transactions relationnelles, requêtes multi-tables) et paie son overhead structurel : écriture en place dans les pages B-tree, planificateur sur le chemin de lecture, journal/WAL par transaction. Même avec WAL activé, l'amplification d'écriture du B-tree sous insertion soutenue est structurelle, pas un paramètre à régler. Le rejet est **indépendant du langage hôte** — il s'applique à SQLite via `rusqlite` en Rust autant qu'à `Python+sqlite3`. C'est la correction de la lacune n°2 : la ligne « Python+SQLite » du tableau ci-dessus rejetait Python pour son overhead mémoire, pas SQLite pour son inadéquation structurelle. Les deux rejets sont distincts. Référence : `lab/LESSONS.md` §L17.

**LevelDB — insuffisant.**
LevelDB est bien un LSM tree (structure correcte), mais lui manquent trois capacités exigées par le Layer 0 : (1) pas de column families — impossible de séparer log causal, index secondaire et BlobDB dans une seule DB avec atomicité de batch cross-CF (ADR-0011 §`agent_ts`, ADR-0010) ; (2) modèle single-writer global, sans la granularité de tuning d'écriture concurrente attendue ; (3) bloom filter interne non configurable par usage (pas de contrôle des bits/clé ni du choix full vs block-based, contrairement à ce que L18 exige). RocksDB est précisément le fork de LevelDB par Facebook qui ajoute ces trois capacités ; choisir LevelDB serait choisir la version dépourvue des features dont le Layer 0 dépend.

**LMDB — inadapté au profil write-heavy.**
LMDB est un B+tree MVCC copy-on-write mmap, excellent en lecture (lectures sans copie, sans verrou, latence très basse) et c'est précisément son point fort. Mais le profil du Layer 0 est write-heavy append-only, et c'est là que le modèle LMDB est inadapté : (1) le copy-on-write d'un B+tree sous insertion soutenue produit une amplification d'écriture structurelle (pages parentes recopiées à chaque commit) que l'append-only batché d'un LSM évite par conception ; (2) le modèle single-writer (un seul writer en transaction à la fois) sérialise le chemin d'écriture dominant ; (3) la taille de la DB est bornée par une `map_size` fixée à l'ouverture, contrainte rigide pour un store dont la croissance est dictée par le débit d'émission des agents. Le profil où LMDB gagne (read-mostly, working set en RAM) n'est pas celui du Layer 0. À noter que ce même arbitrage s'inverse sur la cible seL4 où l'index est read-only statique — voir §Renvoi seL4.

**RocksDB — retenu.**
RocksDB est un LSM tree : les écritures vont en memtable RAM puis sont flushées en batch en SST files, ce qui correspond exactement au profil append-only write-heavy (pas de mutation en place, amplification de write maîtrisée par le leveled compaction). Il fournit les trois capacités manquantes à LevelDB : bloom filter configurable par CF (bits/clé, full filter — ADR-0011, L18), column families avec `WriteBatch` atomique cross-CF (invariant log ↔ index, ADR-0011 §Atomicité), et write buffer dimensionnable. Le binding `rocksdb` (crate Rust) est mature et production-grade. Les options retenues et leur justification à la lumière de la littérature LSM sont figées par **ADR-0011**.

### Renvoi vers la cible seL4 (ADR-0042 / ADR-0043)

Sur la cible seL4 (Phase 9+), le moteur d'index n'est pas RocksDB mais **redb** (B+tree, fork no_std) — ce qui peut sembler contredire le rejet de LMDB ci-dessus. Il n'y a pas contradiction, parce que le **rôle** et le **profil** diffèrent :

- Sur Linux (Layer 0, ce PoC), RocksDB porte le store et le log dans leur **rôle autoritaire**, sur un profil write-heavy append-only où le LSM gagne.
- Sur seL4, l'index redb est un **cache reconstructible, jamais autoritaire** (ADR-0038 §3, ADR-0042 §Amendement) : la source de vérité est le journal append-only content-addressed Q3-C, et l'index est rebuild en O(N) depuis ce journal s'il se corrompt. Son profil d'usage est donc le **lookup read-only sur DB statique** (P3a), exactement le profil où un B+tree (redb, comme LMDB) bat un LSM (benchmark P3a : redb p99 739 µs vs RocksDB 1 850 µs, ×2 — ADR-0042 §Benchmark).

Autrement dit : LMDB est rejeté pour le rôle write-heavy autoritaire du Layer 0 Linux, et un B+tree (redb) est retenu pour le rôle read-only reconstructible de l'index seL4. Le critère discriminant est le profil d'écriture, pas le moteur en absolu. Voir ADR-0042 (choix redb) et ADR-0043 (rôle d'index non-autoritaire, topologie 2-processus).

## Périmètre du PoC

Le PoC valide les hypothèses bloquantes dans l'ordre de criticité :

1. **H-rollback-latence** — store content-addressed RocksDB, états synthétiques 50–500 MB, profondeurs 1–1000 actions en arrière.
2. **H-causal-latence** — 10⁸ entrées synthétiques dans RocksDB, lectures aléatoires p50/p95/p99/p99.9.
3. **H-commit-barrier** — runtime Wasmtime avec interception WASI, suite de tests d'effets externes.
4. **H-densité** — benchmark W1 Wasmtime vs Docker en overhead mémoire par instance.

Le PoC **ne valide pas** : le scheduling multi-nœuds, la sécurité cryptographique des capabilities, la conformité formelle de S1. Ce sont des validations de second niveau.

## Architecture de référence du PoC

```
poc/
├── store/        # RocksDB content-addressed (S2, P2)
├── causal-log/   # Index causal append-only (P3)
├── runtime/      # Wasmtime + Tokio scheduler (S1b, S4, S5, S6b, S7)
├── capabilities/ # Tracking in-memory (hash map agent_id → cap set) (P4)
├── benchmarks/   # W1, W2, W3 sur hardware de référence
└── tests/        # Suite SEF-1 à SEF-6
```

## Conséquences

**Positives :**
- Wasmtime est production-stable (v20+), activement maintenu par Bytecode Alliance.
- WASI Preview 2 fournit une interception des effets externes sans modifications du kernel hôte — H-commit-barrier est testable dès la semaine 1.
- Rust garantit l'absence de GC overhead, ce qui rend les mesures de densité mémoire (H-densité) interprétables.

**Négatives / coûts acceptés :**
- WASM Component Model impose une sérialisation des types à la frontière des composants — overhead à mesurer pour P1.
- S1(a) (isolation hardware-enforced) n'est que S1(b) (isolation software-enforced par sandbox) avec Wasmtime. Le PoC valide les hypothèses de performance, pas la garantie de sécurité formelle.
- S6 est conditionnel : Wasmtime + WASI Preview 2 expose `wasi:random` comme source non-déterministe injectable — à vérifier que l'implémentation de référence est bien substituable.

**Neutres / à surveiller :**
- Si H-rollback-latence ou H-causal-latence sont réfutées avec RocksDB, le problème est dans le store, pas dans Wasmtime — le substrat d'isolation reste valide et le store peut être remplacé.
- La transition PoC → substrat de production (runtime acteur typé custom) remplacera Wasmtime par un runtime natif, mais le store, le log causal et le modèle de capabilities peuvent être réutilisés.

## Références

- `spec/02b-substrate_requirements.md` — exigences S1–S7
- `spec/03-state-of-the-art.md` — analyse comparative des substrats
- `spec/04-hypotheses.md` — hypothèses bloquantes et prototypes minimaux
- Wasmtime v20+ : https://wasmtime.dev
- WASI Preview 2 : https://github.com/WebAssembly/WASI/tree/main/preview2
- `lab/LESSONS.md` §L17 — rejet architectural de SQLite pour le Layer 0 (formalisé en §Choix du moteur de stockage Layer 0)
- ADR-0011 — options RocksDB retenues pour le Layer 0 (bloom filter, column families, write buffer)
- ADR-0010 / ADR-0017 — contrat `emit`, payloads BlobDB (motive le besoin de column families)
- ADR-0042 — choix du moteur d'index seL4 (redb B+tree, fork no_std)
- ADR-0043 — rôle de l'index redb : cache reconstructible non-autoritaire, topologie 2-processus
- [O'Neil et al. 1996, Acta Informatica] *The Log-Structured Merge-Tree (LSM-Tree)* — fondation du LSM
- RocksDB (fork de LevelDB par Facebook, ajoute column families / bloom filter configurable) : https://rocksdb.org
