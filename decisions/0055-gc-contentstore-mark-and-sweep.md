# ADR-0055 — Garbage collection orphelins ContentStore : mark-and-sweep

**Date :** 2026-06-02
**Statut :** Acceptée

---

## Contexte

### État de croissance du ContentStore

`ContentStore` (fichier `poc/store/src/lib.rs`) est organisé sur RocksDB en deux column families :
- `blocks` : clé = `data_hash` (hash SHA-256), valeur = bloc de données brut.
- `headers` : clé = `snapshot_id` (hash du `SnapshotHeader` bincodé), valeur = `SnapshotHeader` sérialisé.

`SnapshotHeader` contient les champs : `data_hash`, `parent: Option<BlockHash>`, `seq: u64`, `ts_us: u64`.

### Ordre d'écriture et orphelins

L'ordre normal d'écriture est : `put_block(data)` → `put_snapshot(header)` → `append` dans `CausalLog`. Sous un crash brutal (entre `put_block` et `put_snapshot`), un bloc peut être présent dans CF `blocks` sans aucun header de CF `headers` le référençant — ce bloc est **orphelin**. Cet état est **toléré** par la discipline ADR-0027 (régime no-force : store en avance admis, log en retard non admis). 

Le `CausalLog` (RocksDB séparé) porte une garantie de complétude : chaque `LogEntry.hash_after` contient un `snapshot_id` qui identifie univoquement le `SnapshotHeader` correspondant. Par suite, une référence du log vers un bloc absent constituerait une **référence pendante** (corruption stricte, détectée par `rollback_path` / `AgentCrash 0x02` — ADR-0027 §D3).

### Absence de métriques et de déclencheur objectif

ADR-0049 §D3a identifie la croissance non bornée du store observée sur cycles reopen comme **déclencheur du chantier GC**. À date, ce déclencheur n'a pas été atteint. Cependant, deux prérequis pour rendre le GC spécifiable ont été identifiés lors de la consultation architect du 2026-06-02 :

1. Une **métrique opérationnelle** de croissance (différence blocks − headers au fil du temps).
2. La fermeture du **trou de design N:1 header→block** — relation que le refcount introduirait mais que l'algorithme doit gérer sans refcount au stade PoC.

### Relation N:1 : obstacle au refcount

Plusieurs headers peuvent référencer le même `data_hash` (déduplication de contenu). La suppression d'un bloc ne peut donc pas être déclenchée par la disparition d'un seul header — il faut parcourir **tous les headers vivants** pour s'assurer que aucun ne référence le bloc avant de le supprimer. Le refcount per-header esclave une transaction RocksDB monolithique à chaque `put_snapshot` et chaque cycle de GC, un coût non justifié au stade PoC.

---

## Décision

### D1 — Invariant : seuls les blocs sont GC-ables

Un header ne doit **jamais** être supprimé par le GC. Un header pointant un bloc absent constitue une **corruption** (non un orphelin toléré) — cette incohérence est détectée et crée un `AgentCrash` lors d'une tentative `rollback_path`. 

Le GC ne supprime donc que des blocs sans aucun header vivant pointant vers eux.

### D2 — Algorithme : mark-and-sweep, pas de refcount

**Raison du rejet du refcount** : un refcount persistant introduit un invariant transactionnel complexe. À chaque `put_block` / `put_snapshot`, il faut incrémenter/décrémenter le refcount et maintenir une cohérence lors de la compaction RocksDB. Au stade PoC où une seule instance enregistre des snapshots, cet invariant est disproportionné. Si le ContentStore croît à des millions de blocs et que le sweep full-scan devient prohibitif, la décision devra être réexaminée (Phase 7+).

**Algorithme retenu : mark-and-sweep en deux phases.**

1. **Phase mark** : itérer la CF `headers` en lecture seule, collecter l'ensemble `live_data = { h.data_hash | pour tout header h }`.
2. **Phase sweep** : itérer la CF `blocks` en lecture seule, générer une liste des blocs dont le hash ∉ `live_data`, puis les supprimer (par batch RocksDB).

Cet algorithme est simple, déterministe, et ne demande aucune métadonnée persistante au-delà des structures existantes.

### D3 — Mode d'exécution : offline obligatoire

Le GC online (execution pendant l'exécution du runtime) est **interdit** tant que #7b (commit cross-store atomique, ADR-0051 §D4) n'est pas résolu. 

**Race window** : un GC concurrent à une séquence `put_block` → append log → put_snapshot peut, entre `put_block` et `put_snapshot`, classer le bloc comme orphelin et le supprimer. L'append du log crée ensuite une référence vers un bloc absent — une **référence pendante**, corruption stricte.

**Précondition obligatoire** : le GC ne s'exécute qu'à l'arrêt quiescent du runtime. Tous les appends en attente doivent être flushés au CausalLog ; aucun arrêt brutal (SIGKILL) ne peut précéder le GC sans une étape de synchronisation préalable.

### D4 — Métrique de déclenchement et seuil

**Métrique** : différence `Δ = blocks_count − headers_count`, estimée via l'appel RocksDB `estimate-num-keys` sur les deux CFs respectives.

**Seuil de déclenchement** (les **deux conditions doivent être vraies**) :

1. **Condition statique** : `Δ > max(1024, 0.02 × headers_count)`
   - Tolère le bruit d'un crash isolé (orphelins ponctuels bornés).
   - À `headers_count = 10 000`, seuil = 200.

2. **Condition dynamique** : sur une fenêtre glissante de 10 minutes, la **pente OLS** de Δ sur le temps est **> 0** (croissance stricte).
   - Exige une croissance tendancielle, pas un palier constant post-crash.
   - Analogue du critère ADR-0033 (OLS sur RSS) appliqué au disque.

**Réserve empirique** : `estimate-num-keys` est bruité par les tombstones et la compaction L0. La métrique doit être validée empiriquement sur un nombre connu d'orphelins avant d'armer le seuil en production.

### D5 — Interface ContentStore : itérateurs en lecture seule

Deux itérateurs **en lecture seule** sont ajoutés à `ContentStore` pour supporter le GC :

```rust
/// Phase mark : itère tous les data_hash vivants (référencés par au moins un header).
pub fn iter_header_data_hashes(&self) -> impl Iterator<Item = BlockHash>;

/// Phase sweep : itère tous les blocs stockés.
pub fn iter_block_hashes(&self) -> impl Iterator<Item = BlockHash>;
```

**Pas d'itérateur public** `iter_snapshot_ids()` — ce serait une fuite d'API (utile **uniquement** au GC, aucun cas d'usage externe). Les primitives de suppression (`delete_block`) sont internes au module GC et ne sont **pas** exposées par `ContentStore`.

### D6 — Localisation : le module GC réside dans `poc/runtime/src/`

Le module GC (nommé `gc_orphans`) doit résider dans `poc/runtime/src/` (ou un binaire dédié `poc/runtime/src/bin/`), **pas** dans `poc/store/src/`.

**Raison** : `poc/store` dépend de RocksDB uniquement et ne dépend **pas** de `poc/causal-log`. Le GC a besoin des deux CausalLog et ContentStore pour valider les références croisées. `poc/runtime` dépend déjà des deux et est donc le bon lieu. Cela préserve la séparation des responsabilités : `poc/store` reste un composant de stockage pur, `poc/runtime` intègre les politiques de maintenance.

### D7 — Déclencheur objectif inchangé

Le déclencheur objectif reste celui d'ADR-0049 §D3a : **croissance non bornée du store observée sur cycles reopen**. Cet ADR formalise ce déclencheur en donnant une définition opérationnelle (D4) et en fermant le trou de design N:1 (D2). Le HOLD sur le chantier GC reste actif jusqu'à ce que l'une des deux conditions de déclenchement (D4 statique + dynamique) soit atteinte pendant une exécution de PoC.

---

## Conséquences

### Code

- **`poc/store/src/lib.rs`** : ajouter les deux itérateurs (D5). Implémentation : ouverture d'un `DBIterator` sur chaque CF via `db.iterator_cf()`, sans matérialisation complète de l'ensemble (streaming).

- **`poc/runtime/src/gc_orphans.rs`** (nouveau) : module GC implémentant l'algorithme mark-and-sweep (D2). Contient :
  - `fn mark_phase(store, log)` → `HashSet<BlockHash>` des blocs vivants.
  - `fn sweep_phase(store, live_data)` → vecteur des blocs supprimés (pour trace/audit).
  - `fn run_offline_gc()` — point d'entrée gardé en amont par les conditions de déclenchement (D4, D3).

- **`poc/runtime/src/bin/orphan_metric_sampler.rs`** (nouveau, use case) : programme utilitaire qui échantillonne `estimate-num-keys` sur les deux CFs du store et du log à 1 Hz, sortie CSV. Permet de calibrer le seuil de déclenchement (D4) et de valider la métrique avant production.

- **Script d'analyse** (`poc/scenarios/orphan-metric/analyze.py` ou équivalent) : charge le CSV et calcule l'OLS sur une fenêtre glissante 10 min, pour aider la décision de déclenchement (condition dynamique D4).

### Spécification

- **`spec/10-modele-durabilite.md`** : aucun amendement requis. L'invariant I-CSR (intégrité du chemin CausalLog → ContentStore) reste inchangé ; cet ADR ne fait que formaliser la maintenance du store sous la contrainte I-CSR.

### Traces

- **ADR-0049 §D3a** : reconfirmé. Le déclencheur dormant (GC réclamé par croissance non bornée) reçoit désormais une définition opérationnelle (D4) et un plan d'implémentation (D2, D5, D6).
- **ADR-0051 §D4** : complété. La re-séparation CAS/index (déclencheur du chantier intégrant #7b) et ce chantier GC sont **distincts** : GC est opérationnel dès que le store croît ; la re-séparation en CF distinctes est une **refacto architecturale** à faire au moment de #7b (commit cross-store atomique).
- **ADR-0027 §D3** : aucun amendement. La discipline no-force (orphelin toléré, référence pendante interdite) reste l'invariant pivot.

### INDEX.md

Ajouter une ligne pour ADR-0055 dans la table des ADRs :

| [0055](0055-gc-contentstore-mark-and-sweep.md) | Garbage collection orphelins ContentStore : mark-and-sweep | 2026-06-02 | Acceptée | — | — | 0049, 0051, 0027, 0033 |

---

## Références

- ADR-0027 — Régime de durabilité log vs ContentStore (discipline no-force, orphelin toléré)
- ADR-0049 — Clôture du PoC seL4 (déclencheur dormant GC)
- ADR-0051 — Clôture campagne adversariale (§D4 : commit cross-store atomique comme nœud architectural du GC)
- ADR-0033 — Critère OLS sur RSS (analogie métrique de croissance LSM)
- `poc/store/src/lib.rs` — ContentStore, CFs `blocks` et `headers`
- `poc/causal-log/src/lib.rs` — CausalLog.append()
- `spec/10-modele-durabilite.md` — Invariant I-CSR (intégrité référentielle cross-store)
- [Gray & Reuter 1992] *Transaction Processing* — garbage collection dans les systèmes persistants
- [Rosenblum & Ousterhout 1991] *The Design and Implementation of a Log-Structured File System* — stratégies mark-and-sweep appliquées au disque

---

*Format : [MADR](https://adr.github.io/madr/)*
