<!-- Corps de la GitHub Release `blog-04-densite` (releases jumelles FR/EN). ⟨à épingler⟩ au tag. -->

# blog-04 — Le coût d'un agent endormi

**Régime :** R2 (ressources) · **Statut :** PARTIEL · **Substrat :** Linux (Wasmtime vs Docker+Python) / NVMe consumer (R-blog-1)

## Claim → Preuve

| Claim de l'article | Borne | Preuve (permalink `@blog-04-densite`) | Substrat |
|---|---|---|---|
| Densité **hébergée** (RAM dormante) | ×4 539–7 375 ; loi overhead(N)=9,65−54/N (R²=0,988) | ⟨results/.../T6-qualif/verdict.json:Lxx⟩ | Linux |
| Latence de réveil | p99 311 µs (réf.) / 378 µs (saturé), budget 10 ms | ⟨results/.../T7-T8/verdict.json:Lxx⟩ | Linux |
| Densité **active** (≠ hébergée) | ~70 agents (14 agents/s × cycle 5 s) | ⟨spec/07-plafonds-architecturaux.md⟩ | Linux |

## Reproduire
```bash
git clone <url> && cd os-public/poc && git checkout blog-04-densite
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui --bin demo-tui -- --scene swarm
# scène swarm = MÉCANISME (évict/réveil), PAS une mesure de densité
```

## Limites (piège #1 — le flanc n°1)
- **Densité hébergée ≠ active.** ~10⁶ dormants n'est PAS ~70 actifs.
- **R2**, P1 **sacrifiable** jusqu'à 3× (jamais sous 1×) — pas un « ×5 ferme ».
- Verdict **PARTIEL** : RAM dormante seulement ; comparaison active (P1b) abandonnée (non transférable).
- **Non transférable seL4** (moteur **redb**, pas RocksDB ; ADR-0065).
