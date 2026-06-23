# Reproduction autoritaire — blog-02 (le vrai système)

**Substrat de mesure :** PoC Linux — Rust / Wasmtime / **RocksDB** / NVMe consumer (R-blog-1).
**Moteur :** RocksDB = PoC Linux. La cible seL4 utilise **redb** (R-blog-2) — non concerné ici.

```bash
git clone <url-os-public> && cd os-public/poc
git checkout blog-02-dag-causal          # permalink épinglé, jamais main

# 1) Démo interactive : couche preuve [d] + falsification [t]
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --scene effects
# Au clavier : [d] hashes complets · [t] falsification → id stocké ≠ id recalculé + juge orphelin

# 2) Vérificateur tiers (exit 0 intègre / 1 mismatch, pointe l'action_id / 2 erreur)
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --bin log-verify -- <chemin-log>
# attendu : sur un journal corrompu d'une entrée → exit 1, désigne la bonne action_id
```

Vérification = `clé == SHA256(octets bruts)` + absence de parents pendants.

Transcript réel attendu : voir `expected/` (capturé au tag).
