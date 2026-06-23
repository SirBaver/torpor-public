<!-- Corps de la GitHub Release `blog-03-rollback` (os-public + os-public-en, releases jumelles).
Champs ⟨à épingler⟩ : permalinks réels au tag (jamais main) + lignes verdict.json. -->

# blog-03 — Annuler 500 décisions en 17 ms

**Régime :** R1 (effets) · **Statut :** prouvé · **Substrat de mesure :** Linux / **RocksDB** / NVMe consumer (R-blog-1)

## Claim → Preuve

| Claim de l'article | Borne | Preuve (permalink `@blog-03-rollback`) | Substrat |
|---|---|---|---|
| Rollback transactionnel bout-en-bout | 17–20 ms @ profondeur 500 (cible ≤ 100 ms) | ⟨results/.../SEF-2/verdict.json:Lxx⟩ | Linux/RocksDB |
| Rollback store (micro-bench) | p95 ~99 µs @ 10⁶ ops | ⟨results/.../store-bench/verdict.json:Lxx⟩ | Linux/RocksDB |
| Coût O(profondeur), pas O(N) ; invalidation des caps | propriété | ⟨decisions/0007-rollback-caps-invalidation.md⟩ | substrat Linux |

## Reproduire
```bash
git clone <url> && cd os-public/poc && git checkout blog-03-rollback
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui --bin demo-tui -- --scene effects
# [r] rollback sur dialogue vivant · ou --scene mission-resume
```

## Limites (piège #4 / R-blog-3)
- **Couvre l'état local, PAS les effets externes déjà émis.** Compensation saga = **non-objectif documenté**.
- Borne conditionnelle : Linux/**RocksDB** ; cible seL4 = **redb**, latence non transférée (ADR-0065).
