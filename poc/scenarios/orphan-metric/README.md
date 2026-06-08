# Scénario : métrique de croissance orphelins ContentStore

**Référence :** ADR-0055 §D4  
**Objectif :** mesurer `Δ = blocks_count − headers_count` sur cycles reopen et évaluer les deux conditions de déclenchement du GC.

## Outils

- `orphan-metric-sampler` — binaire Rust, échantillonnage 1 Hz, sortie CSV
- `analyze.py` — script Python, OLS sur fenêtre glissante 10 min, verdict déclenchement

## Procédure

```bash
# 1. Lancer le sampler pendant N secondes (ou sans --duration-s pour illimité)
cargo run --bin orphan-metric-sampler -- \
  --db-store /path/to/store \
  --duration-s 600 \
  --out metrics.csv

# 2. Analyser
python3 analyze.py metrics.csv
```

## Conditions de déclenchement GC (ADR-0055 §D4)

Les **deux** conditions doivent être vraies simultanément :

1. **Statique** : `Δ > max(1024, 0.02 × headers_count)`
2. **Dynamique** : pente OLS de Δ sur fenêtre 10 min > 0

Un palier constant post-crash (Δ stable) satisfait la condition 1 mais pas la 2 → pas d'action.

## Réserve

`estimate-num-keys` est bruité par les tombstones et la compaction L0. Valider
empiriquement sur un nombre connu d'orphelins avant d'armer le seuil en production.
