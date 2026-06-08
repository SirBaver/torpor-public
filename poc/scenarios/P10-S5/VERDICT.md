# P10-S5 — Fairness + priorité sous backend réel

**Date :** 2026-05-30
**Scénario :** P10-S5 (Phase 10, ADR-0052 Axe A)
**Backend :** OllamaBackend / llama3.2:3b @ http://localhost:11434
**Hardware :** AMD Ryzen 5 PRO 4650U (CPU, sans GPU) — **non représentatif du hardware cible (ADR-0052 §D2)**

## Configuration

| Paramètre | Valeur |
|-----------|--------|
| N Foreground | 3 |
| N Supervisor | 1 |
| pool_cap (max_concurrent) | 1 |
| Budget | 900 s |
| max_starvation_ms | 30 000 ms (ADR-0023) |

## Résultats

| Assertion | Verdict | Détail |
|-----------|---------|--------|
| A-priorité (ADR-0022 D1) | **PASS** | sv_max=1780129117682000 µs < fg_max=1780129127607000 µs — le supervisor a terminé ~9,9 s avant le dernier foreground |
| A-E3 — pas de famine (ADR-0023 D2) | **PASS** | 3/3 foreground + 1/1 supervisor complétés |
| A-E1 — FIFO intra-classe (ADR-0023 §D3) | **PASS** | 3 traces Foreground : ordre slot_acquired_instant == ordre admission_seq |

## Observations (non bloquantes)

| Métrique | Valeur |
|----------|--------|
| elapsed | 20 003 ms |
| t_infer médiane | 13 636 ms |
| t_infer p99 | 19 841 ms |
| total_admitted | 4 |
| total_promoted | 0 |
| total_rejected | 0 |

**A-priorité** : avec cap=1, le Supervisor (soumis en dernier) a néanmoins obtenu son slot avant le 2e et le 3e Foreground. La priorité stricte de `InferenceQueue` est confirmée sous inférence réelle.

**A-E1** : les 3 traces Foreground montrent un ordre de service strict par admission_seq. FIFO intra-classe maintenu.

**total_promoted = 0** : aucune famine de Batch (normal — aucun agent Batch dans ce run).

## Verdict global : PASS

## Garde-fous (ADR-0052 §D2)

- t_infer (13–20 s sur CPU) non transférable au hardware cible.
- La propriété A-priorité est vérifiée structurellement par `InferenceQueue` — indépendante du t_infer réel.
- Ce verdict démontre que E1/E3/A-priorité tiennent sous latence variable réelle, pas que le débit de production est atteint.

## Rapport JSON

`poc/scenarios/P10-S5/report.json`
