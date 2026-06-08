# B3 Phase 9 — Revue moteur d'index : redb / sled

**Date :** 2026-05-28  
**Contexte :** ADR-0038 §B3 — moteur d'index persistant no_std seL4 (10⁸ entrées, P3a ≤ 10 ms)

---

## redb (v4.1.0)

### Trait `StorageBackend` — interface pluggable

redb expose un trait public `StorageBackend` dans `src/db.rs` :

```rust
pub trait StorageBackend: 'static + Debug + Send + Sync {
    fn len(&self) -> std::result::Result<u64, io::Error>;
    fn read(&self, offset: u64, out: &mut [u8]) -> std::result::Result<(), io::Error>;
    fn set_len(&self, len: u64) -> std::result::Result<(), io::Error>;
    fn sync_data(&self) -> std::result::Result<(), io::Error>;
    fn write(&self, offset: u64, data: &[u8]) -> std::result::Result<(), io::Error>;
    fn close(&self) -> std::result::Result<(), io::Error>;
}
```

Deux implémentations fournies : `FileBackend` (std::fs::File) et `InMemoryBackend` (Vec<u8>
 via RwLock). Le trait est sémantiquement correct pour un backend block seL4 (lecture/écriture par offset, sync, resize). Difficulté : le type de retour est `std::io::Error` — non disponible en no_std.

### Support no_std

**NON.** Aucun flag `no_std` dans `Cargo.toml`. Aucun `#![no_std]` dans `src/lib.rs`. Environ 50+ usages de `std::` dans les fichiers clés (`db.rs` : 30 occurrences, `transactions.rs` : 20). L'infrastructure interne (thread locks via `std::sync`, hash maps via `std::collections`, I/O via `std::io`) est profondément std-tied.

Coût estimé d'un fork no_std : remplacement de `std::io::Error` par un type custom, `std::sync::*` par `spin::*` ou `portable-atomic`, `std::collections` par `hashbrown`. Effort : 3–6 semaines pour un portage partiel, 2–3 mois pour un portage propre incluant les tests.

### Version et statut

- Version stable : **4.1.0** (2026)
- Rust minimum : 1.89 (édition 2024)
- Actif, mainteneur unique (Christopher Berner)
- MIT OR Apache-2.0
- LOC approximatif : ~8 000 lignes Rust (src/ seulement)

### Backend custom : faisabilité

Brancher un backend seL4 block device est **faisable** si redb est porté en no_std (l'interface trait est l'abstraction correcte). Sans fork, non utilisable sur seL4. Avec fork no_std, le trait `StorageBackend` devient :

```rust
pub trait StorageBackend: 'static + core::fmt::Debug + Send + Sync {
    fn len(&self) -> Result<u64, StorageError>;
    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), StorageError>;
    fn set_len(&self, len: u64) -> Result<(), StorageError>;
    fn sync_data(&self) -> Result<(), StorageError>;
    fn write(&self, offset: u64, data: &[u8]) -> Result<(), StorageError>;
    fn close(&self) -> Result<(), StorageError>;
}
```

Le backend seL4 implémenterait ce trait via IPC seL4 vers le serveur de stockage (ADR-0038).

---

## sled (v1.0.0-alpha.124)

### Interface pluggable

**Aucun trait de backend pluggable trouvé.** L'implémentation est monolithique autour de `std::fs` + mmap. Pas de point d'extension pour un backend custom.

### Support no_std

**NON.** Fortement couplé à std (mmap, threads, atomics std).

### Statut

- Version : `1.0.0-alpha.124` — en alpha depuis 2019, jamais sorti en v1.0 stable
- Dernier push : 2026-04-04
- 171 issues ouvertes
- **Non recommandé** : instabilité chronique de l'API

---

## Qualification B3

| Critère | redb | sled |
|---------|------|------|
| Backend pluggable | ✓ (`StorageBackend` trait) | ✗ |
| no_std compatible (tel quel) | ✗ (std-tied) | ✗ |
| no_std adaptable (fork) | PARTIEL (3–6 sem) | NON (trop couplé) |
| Stable | ✓ (v4.1.0) | ✗ (alpha) |
| LOC raisonnable | ✓ (~8 KLOC) | ✓ mais instable |

**Candidat retenu : redb fork no_std**

redb est le seul candidat sérieux. Son `StorageBackend` est l'interface exacte requise pour un backend block seL4. Un fork no_std est faisable (3–6 semaines) et produit un B+tree ACID-complet, supérieur à une réécriture from scratch (ADR-0038 §B3 option LSM custom estimée 4–6 mois).

---

## Benchmark P3a 10⁸ sur Linux (à faire)

Avant d'investir dans un fork no_std de redb, il faut qualifier ses performances à l'échelle P3a (10⁸ entrées, lookup point get() ≤ 10 ms p99). Protocole :

- Population : 10⁸ entrées (clé : action_id u64, valeur : 100 B)
- Mesure : K=3 passes de 10 000 get(action_id) aléatoires
- Critère PASS : p99 ≤ 10 ms (identique T5/SEF-5 sur RocksDB)
- Référence : p99 SEF-5 RocksDB = 1 368 / 1 727 / 1 850 µs

Si redb passe P3a → fork no_std justifié.  
Si redb échoue P3a → investigation LSM custom ou alternative (embedded-lsm, fjall).

---

## Sources

- `github.com/cberner/redb` master, `src/db.rs` (trait), `src/tree_store/page_store/backends.rs` (impl)
- `github.com/cberner/redb` master, `Cargo.toml` (version 4.1.0, Rust 1.89)
- `github.com/spacejam/sled` main (alpha.124, 171 issues, last push 2026-04-04)
