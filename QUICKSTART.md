# Getting Started

## Prérequis

- Rust stable + cargo (`rustup`)
- Cible WASM : `rustup target add wasm32-unknown-unknown`
- GCC 15.x uniquement : `export CXXFLAGS="-include cstdint"` (contourne librocksdb-sys 0.16.0)
- Ollama + qwen2.5:3b : optionnel, uniquement pour les tests avec vrai LLM (hors CI)

## Build

```bash
export CXXFLAGS="-include cstdint"   # GCC 15.x
cargo build --workspace
cargo build --target wasm32-unknown-unknown -p agent-sdk --examples --release
```

## Lancer tous les scénarios

```bash
bash poc/scenarios/run-all.sh
```

Produit `poc/scenarios/report.json`. Quatre scénarios, verdict `pass` ou `fail` par scénario.

## Lancer les tests unitaires

```bash
cargo test --workspace --release
```

53 tests attendus verts. Les tests CI utilisent `FixedResponseBackend` et `SleepyBackend` — Ollama n'est pas requis.

## Lire le log causal

```bash
cargo run -p os-poc-reconstruct -- <chemin-vers-db-rocksdb>
```

Lecture séquentielle des événements par agent, dans l'ordre causal.

## Les quatre scénarios E2E

| Scénario | Ce qu'il démontre |
|---|---|
| S1 `supervision-algorithmique` | ABI `agent_infer` + canal de validation A3 + composition cross-agent |
| S2 `self-rollback-incoherence` | Introspection A1 + self-rollback A2 sur incohérence LLM détectée |
| S3 `inference-cap` | Borne dure du pool d'inférence k=4, état `WaitingInference` observable |
| S4 `scheduler-rollback` | Rollback scheduler pendant inférence en vol → séquence `0x0C → 0x0E → 0x0B` |

Détail de chaque scénario : `poc/scenarios/S<N>-*/README.md`.

## Pour aller plus loin

- **Doc technique du PoC** : `poc/README.md`
- **Design du système** : `spec/` (vision, propriétés, hypothèses, glossaire)
- **Décisions d'architecture** : `decisions/INDEX.md`
- **Leçons empiriques** : `lab/LESSONS.md`
- **Synthèse du projet** : `README.md`
