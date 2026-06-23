# Reproduction autoritaire — blog-05 (le vrai système)

**Substrat :** PoC Linux — Rust / Wasmtime / **RocksDB** (R-blog-1). Cible seL4 = **redb** (non concerné ; isolation inter-process seL4 démontrée par ailleurs, ADR-0049 §D1, mais le confused-deputy n'a été rejoué que sur Linux).
**Propriété :** P4 — isolation par capabilities, vérifiée à la frontière, non contournée par l'agent (périmètre testé).

```bash
git clone <url-os-public> && cd os-public/poc
git checkout blog-05-capabilities
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --scene effects
# [x] intrus : data_accessor (cap reports/) tente confidential/ → refus 0x14, accès nul, refus journalisé
```

Red team : scénarios SEF-3 / SEF-9 (confused deputy) — isolation tenue + trou d'audit trouvé/corrigé/revalidé. Transcripts réels : `expected/` (capturé au tag) ; voir aussi `red-team/`, `decisions/0050`, `decisions/0051`.
