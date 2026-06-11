# Getting Started

## Prérequis

- Rust stable + cargo (`rustup`)
- Cible WASM : `rustup target add wasm32-unknown-unknown`
- **libclang** (requis par bindgen pour compiler RocksDB/zstd) : Debian/Ubuntu `sudo apt-get install libclang-dev`, Fedora `sudo dnf install clang-devel`
- GCC ≥ 13 : `export CXXFLAGS="-include cstdint"` (contourne librocksdb-sys 0.16.0 ; inoffensif sur GCC plus ancien)
- Ollama + qwen2.5:3b : optionnel, uniquement pour les tests avec vrai LLM (hors CI)

## Build

Le workspace cargo vit dans `poc/` — la racine du dépôt n'a pas de `Cargo.toml`.

```bash
cd "$(git rev-parse --show-toplevel)/poc"
export CXXFLAGS="-include cstdint"   # GCC ≥ 13
cargo build --workspace
cargo build --target wasm32-unknown-unknown -p agent-sdk --examples --release
```

## Lancer tous les scénarios

```bash
cd "$(git rev-parse --show-toplevel)"
export CXXFLAGS="-include cstdint"   # GCC ≥ 13
bash poc/scenarios/run-all.sh
```

Produit `poc/scenarios/report.json` — 10 scénarios comptés (S1–S5, S9–S13), verdict `pass` ou
`fail` par scénario. S14 (lookup causal sur 10⁸ actions, ~15 GB de base) et S15 (crash machine,
requiert sudo) sont `skipped` par défaut — `RUN_S14=1` / `RUN_S15=1` pour les inclure.

## Lancer les tests unitaires

```bash
cd "$(git rev-parse --show-toplevel)/poc"
CXXFLAGS="-include cstdint" cargo test --workspace --release
```

164 tests attendus verts. Les tests CI utilisent `FixedResponseBackend` et `SleepyBackend` — Ollama n'est pas requis.

## Lire le log causal

```bash
cd "$(git rev-parse --show-toplevel)/poc"
cargo run -p os-poc-reconstruct -- <chemin-vers-db-rocksdb>
```

Lecture séquentielle des événements par agent, dans l'ordre causal.

## Les quatre scénarios E2E d'origine

| Scénario | Ce qu'il démontre |
|---|---|
| S1 `supervision-algorithmique` | ABI `agent_infer` + canal de validation A3 + composition cross-agent |
| S2 `self-rollback-incoherence` | Introspection A1 + self-rollback A2 sur incohérence LLM détectée |
| S3 `inference-cap` | Borne dure du pool d'inférence k=4, état `WaitingInference` observable |
| S4 `scheduler-rollback` | Rollback scheduler pendant inférence en vol → séquence `0x0C → 0x0E → 0x0B` |

Détail de chaque scénario : `poc/scenarios/S<N>-*/README.md`.

## Branche seL4/QEMU (jalons C.1→C.11-prov)

Les sous-arbres `poc/sel4-hello/`, `poc/redb-fork/` et `poc/redb-p3a/` ne font **pas** partie du
workspace cargo (voir `poc/README.md` §Périmètre). Ils se construisent via une image Docker de
toolchain (seL4 15.0.0, Rust nightly, `sel4-kernel-loader`, QEMU AArch64) qui n'est pas
distribuée avec ce dépôt — elle se reconstruit depuis l'upstream `rust-root-task-demo` :

```bash
# 1. Construire l'image de toolchain `rust-root-task-demo` (long au premier build : compile seL4)
cd "$(git rev-parse --show-toplevel)/poc/sel4-hello"
git clone https://github.com/seL4/rust-root-task-demo.git
git -C rust-root-task-demo checkout 7dcc192b54d002aa43e0b5bb9f6d00f851243f9a
make -C rust-root-task-demo/docker build

# 2. Lancer un jalon (exemple : C.5 — redb no_std sur virtio-blk)
cd "$(git rev-parse --show-toplevel)/poc/sel4-hello/c5-redb-on-virtio"
make test   # compile dans Docker + boot QEMU + vérifie C5_PASS
```

Les `Cargo.toml` des jalons épinglent `rust-sel4` au rev `7a2321f2` — la même version que celle
compilée dans l'image. Pièges connus (caches cargo non persistés, user Docker `x`, pattern
d'invocation) : `agents/sel4.md`.

## Pour aller plus loin

- **Doc technique du PoC** : `poc/README.md`
- **Design du système** : `spec/` (vision, propriétés, hypothèses, glossaire)
- **Décisions d'architecture** : `decisions/INDEX.md`
- **Leçons empiriques** : `lab/LESSONS.md`
- **Synthèse du projet** : `README.md`
