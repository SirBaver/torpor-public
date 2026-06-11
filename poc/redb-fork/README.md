# redb-fork — fork no_std de redb 4.1.0

Fork du moteur d'index [redb](https://github.com/cberner/redb) version 4.1.0
(© 2021 Christopher Berner), porté en `no_std` pour le jalon seL4 C.5 (ADR-0042).

## Licence

Comme l'upstream, ce fork est distribué sous double licence **MIT OR Apache-2.0** :
`LICENSE-MIT` et `LICENSE-APACHE` sont les fichiers de licence d'origine du projet redb.
Le copyright du code d'origine appartient à Christopher Berner et aux contributeurs de redb.

## Modifications par rapport à l'upstream

Portage `no_std` appliqué par `patch_nostd.py` (réécritures mécaniques) puis ajustements manuels :

- `std::sync` → `spin` / `core::sync::atomic` / couche `compat.rs` (Condvar, PoisonError) ;
- `std::collections::HashMap/HashSet` → `BTreeMap`/`BTreeSet` (`compat.rs`) — hashbrown retiré
  (conflit E0464 avec `build-std=alloc` sur la toolchain nightly seL4) ;
- backend fichier remplacé par le trait `StorageBackend` branché sur virtio-blk (cf. `backends.rs`) ;
- features `std`/`logging` désactivées, `panic = "abort"`.

Ce fork n'est **pas** destiné à être republié sur crates.io ni mergé upstream :
c'est un artefact du PoC seL4, figé sur la base 4.1.0.
