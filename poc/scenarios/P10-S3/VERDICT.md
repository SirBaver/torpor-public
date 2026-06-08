# P10-S3 — Borne dure + no-famine sous backend réel

**Date :** 2026-05-30
**Scénario :** P10-S3 (Phase 10, ADR-0052 Axe A)
**Backend :** OllamaBackend / llama3.2:3b @ http://localhost:11434
**Hardware :** AMD Ryzen 5 PRO 4650U (CPU, sans GPU) — **non représentatif du hardware cible (ADR-0052 §D2)**

## Configuration

| Paramètre | Valeur |
|-----------|--------|
| N workers | 6 |
| pool_cap (max_concurrent) | 2 |
| Budget | 600 s |
| max_starvation_ms | 30 000 ms (ADR-0023) |

## Résultats

| Assertion | Verdict | Détail |
|-----------|---------|--------|
| P-α no-famine | **PASS** | 6/6 workers complétés |
| P-β traceability | **PASS** | 6 InferenceRequest + 6 InferenceResponse dans le log |
| P-γ pool vide | **PASS** | active_count == 0 à la fin |

## Observations (non bloquantes)

| Métrique | Valeur |
|----------|--------|
| elapsed | 20 007 ms |
| t_infer médiane | 12 534 ms |
| t_infer p99 | 18 000 ms |
| total_admitted | 6 |
| total_promoted | 0 |
| total_rejected | 0 |

**Overhead scheduler ≈ 0** : elapsed ≈ ceil(6/2) × t_infer_médiane → les 3 vagues de 2 inférences s'enchaînent sans friction mesurable.

## Verdict global : PASS

## Garde-fous (ADR-0052 §D2)

- Les mesures de t_infer (12–18 s sur CPU) sont **non transférables** au hardware cible (GPU 24 GB, t≈2,5 s spec/07 §2).
- k=4–8 et t≈2,5 s de spec/07 §2 restent des **hypothèses non validées** après ce run.
- Ce verdict démontre que le scheduler `InferenceQueue` (ADR-0022/0023) fonctionne correctement sous un backend réel non déterministe — pas que les plafonds de production sont atteints.

## Rapport JSON

`poc/scenarios/P10-S3/report.json`
