# S14 — Traçabilité causale — lookup causal (SEF-5 / P3a)

## Propriété vérifiée

**SEF-5** : Pour tout `action_id` retourné par `append`, `log.get(action_id)` retourne
l'entrée causale complète et correcte en **p99 ≤ 10 ms**, mesurée sur un log de **10⁸ actions**
(spec `benchmarks/equivalence-scenarios.md §SEF-5` + `spec/02-properties.md §P3`).

## Deux propriétés vérifiées par passe

| Propriété | Critère |
|-----------|---------|
| **P-α** Performance | p99 ≤ 10 000 µs (10 ms) sur 10 000 lookups |
| **P-β** Complétude | 1 000 entrées vérifiées bit-à-bit contre le ground truth |

**Ground truth** (déterministe via `populate_synthetic`) :
- `entry.agent_id == [0xAA; 16]`
- `entry.hash_before == [0xAA; 32]`, `entry.hash_after == [0xBB; 32]`
- `entry.emit_payload.is_none()`
- `entry.action_id() == id` — intégrité SHA-256 content-addressed

## Protocole

```
1. Population unique  : populate_synthetic(N=10⁸, sample=10 000) → work/db/
   + sauvegarde des action_ids échantillonnés → work/samples.json
2. K=3 passes de mesure indépendantes (même DB, --load-samples) :
   - P-β : 1 000 entrées → ground truth
   - P-α : 10 000 lookups → latences → p99
```

La DB est partagée entre les K passes pour éviter les effets thermiques
cumulés d'une repopulation répétée.

## Exécution

```bash
cd poc
./scenarios/S14-causal-lookup/run.sh

# Overrides :
N_ENTRIES=100000000 N_SAMPLES=1000 N_READS=10000 K_RUNS=3 \
  ./scenarios/S14-causal-lookup/run.sh
```

## Portée

- **Dans le périmètre** : lookup point `get(action_id)` sur DB statique (P3a).
  Régime cache-mixte (le page cache OS varie entre les passes).
- **Hors périmètre** : P3b (end-to-end emit→fsync→get), P3c (concurrence writes/reads —
  traité par T5-p3c), lookup sous charge soutenue d'écriture.

## Cleanup

La DB générée pèse ~15 GB. Supprimer après le run :

```bash
rm -rf poc/scenarios/S14-causal-lookup/work/
```

## Résultats attendus

Sur AMD Ryzen 5 PRO 4650U + WD SN530 NVMe (classe 2) :
p99 ≈ 500–5 000 µs selon le régime cache (T5 référence : 4 855 µs worst case).
Borne 10 ms confirmée par T5 K=3 classe 2 (ADR-0026).
