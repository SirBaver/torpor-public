# Reproduction autoritaire — blog-03 (le vrai système)

**Substrat :** PoC Linux — Rust / Wasmtime / **RocksDB** / NVMe consumer (R-blog-1). Cible seL4 = **redb** (non concerné).
**Propriété :** P2 — rollback transactionnel O(profondeur), ≤ 100 ms à profondeur 500 ; mesuré 17–20 ms (SEF-2).

```bash
git clone <url-os-public> && cd os-public/poc
git checkout blog-03-rollback
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --scene effects
# [r] rollback sur un dialogue vivant (Message::Rollback) · ou --scene mission-resume (reprise sans recompute)
```

**Limite :** couvre l'état local, **pas** les effets externes déjà émis (saga = non-objectif). Transcript SEF-2 réel : voir `expected/` (capturé au tag).
