# ADR-0042 — Voie B3 : moteur d'index persistant seL4

**Date :** 2026-05-28  
**Statut :** Acceptée — **amendée 2026-05-29** (cf. §Amendement)

---

## Amendement (2026-05-29) — rétractation de l'argument « ACID complet »

La mention « **ACID complet (MVCC copy-on-write)** » de §Justification point 3 est **rétractée comme justification du choix de redb**, pour deux raisons :

1. **L'atomicité transactionnelle de redb est sans objet dans le rôle assigné.** L'index B3 est un *cache reconstructible, jamais autoritaire* (ADR-0038 §3 L106). Si l'index se corrompt sous crash, il est rebuild en O(N) depuis le journal append-only. Son atomicité/durabilité transactionnelle ne sert donc à rien. redb est retenu sur le **seul** critère P3a (p99 739 µs, ×13 sous cible) — voir §Benchmark, inchangé.

2. **L'atomicité P6 est structurelle, pas transactionnelle.** La direction globale du projet (ADR-0027 : no-force / group-commit + recovery, force-at-commit explicitement rejeté ; ADR-0038 §Q3-C : content-addressing immuable + unique append atomique, modèle Dolstra/Nix) porte l'atomicité **par structure**, jamais par les transactions d'un moteur d'index. Invoquer l'ACID de redb pour P6 est donc hors-axe par rapport à la direction validée.

**Conséquence ferme :** redb **ne constitue PAS le store durable**. Il s'instancie **en aval** du journal append-only content-addressed Q3-C (ADR-0038 §Q3), jamais directement sur virtio-blk comme l'a fait le jalon de faisabilité C.5. C.5 a validé une *capacité de brique* (portage no_std + StorageBackend sur virtio fonctionnels), pas la topologie d'architecture. Le câblage store-direct de C.5 (`poc/sel4-hello/c5-redb-on-virtio/src/main.rs` L194-211) est une **dette à corriger en C.6**, pas un précédent. Voir LESSONS.md L68.

---

## Contexte

ADR-0038 §B3 a identifié le besoin d'un moteur d'index persistant pour la Phase 9 seL4 :
- 10⁸ entrées (clé : `action_id u64`, valeur : ~100 B)
- P3a : lookup point `get(action_id)` p99 ≤ 10 ms sur DB statique (référence SEF-5 RocksDB)
- Contrainte seL4 : `#![no_std]`, pas de `std::fs`, accès bloc via driver virtio-blk (C.4)

`b3-storage-research.md` a identifié deux candidats : **redb** (v4.1.0) et **sled** (alpha.124). Sled a été éliminé (pas de backend pluggable, instabilité chronique). redb a été retenu comme candidat sous condition de benchmark P3a.

---

## Décision

**redb fork no_std retenu comme moteur d'index B3.**

---

## Justification

### Benchmark P3a — résultats (2026-05-28)

Protocole : 10⁸ entrées (u64 → 100 B), K=3 passes de 10 000 `get()` aléatoires uniformes (Modèle A, worst case), NVMe WD SN530.

| Passe | p50 µs | p95 µs | p99 µs | p99.9 µs |
|-------|--------|--------|--------|----------|
| 1     | 277    | 486    | **739** | 1 229   |
| 2     | 196    | 350    | 581    | 1 023    |
| 3     | 163    | 359    | 572    | 919      |

**p99 pire cas : 739 µs.** Cible : 10 000 µs. **×13 sous la limite.**

Référence RocksDB SEF-5 (même workload, même hardware) : 1 368 / 1 727 / 1 850 µs.  
**Ratio redb/RocksDB : ~0.4–0.5 (B+tree read-only 2× meilleur que LSM compacté).**

Taille DB : 23 GB pour 10⁸ × 108 B → ratio 2.1× (overhead B+tree normal).  
Population : 301 s à ~340 000 inserts/s (batches de 100 000 sur NVMe).

### Adéquation architectural

1. **Interface `StorageBackend` pluggable** : les 5 méthodes (`read`, `write`, `set_len`, `sync_data`, `close`) correspondent exactement à l'API du driver virtio-blk C.4 (lecture/écriture par offset, sync, resize). Le backend seL4 implémentera ce trait via IPC vers le serveur de stockage (ADR-0038).

2. **Faisabilité fork no_std** : effort estimé 3–6 semaines. Substitutions nécessaires :
   - `std::io::Error` → type custom `StorageError`
   - `std::sync::*` → `spin::*` ou `portable-atomic`
   - `std::collections::HashMap` → `hashbrown`
   - `std::thread` → suppression (single-threaded sur seL4)
   - `~8 000 LOC` — volume gérable pour un portage partiel ciblé

3. **ACID complet** : redb garantit atomicité (MVCC copy-on-write), cohérence, isolation et durabilité. Supérieur à une réécriture from scratch (ADR-0038 §B3 option LSM custom = 4–6 mois).

4. **Précédent no_std** : plusieurs projets ont forké redb pour des contextes embarqués/no_std (pattern documenté dans la communauté Rust embedded).

---

## Options rejetées

### sled v1.0.0-alpha.124

Rejeté (b3-storage-research.md) :
- Pas de trait de backend pluggable — monolithique autour de `std::fs` + mmap
- En alpha depuis 2019, API instable, 171 issues ouvertes
- Portage no_std estimé infaisable sans réécriture complète

### LSM custom Rust

Rejeté :
- Effort 4–6 mois (spec ADR-0038 §B3)
- P3a prouve que redb B+tree est 2× meilleur que RocksDB LSM sur lookup — pas de motivation à reproduire un LSM
- Un B+tree ACID (redb) est plus simple à auditer/vérifier formellement (alignement spec/08)

### fjall / embedded-lsm

Non investigués — le benchmark P3a redb est suffisamment convaincant (×13 sous cible) pour justifier le fork sans investiguer d'alternatives.

---

## Étapes suivantes (Phase 9 B3)

1. **Fork redb no_std** : créer `poc/redb-no-std/` basé sur redb v4.1.0
   - Substitution `std::io::Error` → `StorageError` (type minimal)
   - Substitution `std::sync::Mutex` → `spin::Mutex`
   - Suppression de l'API multi-thread (single-threaded root task)
   - Gate `#![no_std]` + `extern crate alloc`

2. **Backend virtio-blk** : implémenter `StorageBackend` via C.4 driver
   - `read(offset, buf)` → `VirtIOBlk::read_blocks`
   - `write(offset, data)` → `VirtIOBlk::write_blocks`
   - `sync_data()` → `VirtIOBlk::flush` (ou no-op si virtio-blk ne supporte pas)

3. **Jalon C.5** : ouvrir une DB redb no_std sur seL4, insérer 1 000 entrées, lire 100, signal `C5_PASS`

---

## Contraintes résiduelles

- **`sync_data()` seL4** : `fsync` n'existe pas sur seL4 — la sémantique de durabilité est définie par ADR-0027 niveau (1) (acquittement serveur RAM). `sync_data()` sera une no-op ou déclenchera un IPC de flush vers le serveur de stockage.
- **Fragmentation B+tree** : avec des insertions séquentielles u64, la fragmentation est minimale. En production avec des clés aléatoires (UUIDs), une stratégie de vacuum peut être nécessaire.
- **23 GB pour 10⁸ entrées** : sur hardware réel avec stockage NVMe PCIe (Phase 10+), ce ratio est acceptable. En Phase 9 QEMU, la DB de test sera limitée à 10⁶ entrées pour la RAM disk.

---

## Références

- `poc/redb-p3a/results/redb-p3a/verdict.json` — résultats benchmark P3a
- `decisions/b3-storage-research.md` — revue redb/sled
- `decisions/0038-store-natif-sel4.md` §B3 — interface `StorageBackend`
- `decisions/0041-voie-b2-driver-block.md` — C.4 validé (virtio-blk prêt)
- `poc/scenarios/S14-causal-lookup/report.json` — référence RocksDB SEF-5
