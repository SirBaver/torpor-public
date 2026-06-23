# Reproduction autoritaire — blog-04 (le vrai système)

**Substrat :** PoC Linux — Wasmtime vs Docker+Python, NVMe consumer (R-blog-1). **Régime R2.** Cible seL4 = **redb** (non concerné).
**Verdict :** PARTIEL — densité **hébergée** (RAM dormante) ×4 539–7 375 ; densité **active** (~70 agents, 14 agents/s) ≠ hébergée ; non transférable seL4 (ADR-0065).

```bash
git clone <url-os-public> && cd os-public/poc
git checkout blog-04-densite
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui \
  --bin demo-tui -- --scene swarm
# Mécanisme : admission bornée (in-flight ≤ cap) + éviction/réveil. Compteurs réels.
# CE N'EST PAS une mesure de densité (le grand chiffre vient des benchmarks T6/T7/T8).
```

Transcripts mesures (densité dormante T6-qualif, latence réveil T7/T8) : `expected/` (capturé au tag) ; sources `results/.../verdict.json`.
