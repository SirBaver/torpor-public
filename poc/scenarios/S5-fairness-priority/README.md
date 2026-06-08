# S5 — Fairness & Priority (ADR-0021 / ADR-0022 / ADR-0023)

## Objectif

Valider les propriétés C1-fairness-priority de l'InferenceQueue bornée (ADR-0022 + ADR-0023) :

1. **Priorité** : les agents `Supervisor` obtiennent leur slot avant que les agents `Foreground` aient
   tous terminé, même si les Foreground sont admis en premier.
2. **Pas de famine** : tous les agents Foreground completent dans `max_wait_ms ≤ 30s`.
3. **FIFO intra-classe** (E1) : parmi les agents Foreground, l'ordre de service est l'ordre
   d'admission (timestamps 0x0C dans le log causal).

## Configuration

| Paramètre | Valeur |
|-----------|--------|
| Agents Foreground (`density_worker`) | 8 |
| Agents Supervisor (`supervisor_arith`) | 2 |
| Pool cap (max_concurrent) | 2 |
| Backend | `SleepyBackend(100ms)` |
| queue_capacity | 16 |
| Runtime Tokio | single-thread (determinisme) |

Avec cap=2 et SleepyBackend(100ms), le pire cas pour 8 Foreground est :
- 4 rounds × 100ms = 400ms << 30s (max_wait_ms)

## Assertions

### A1 — Priorité
Les 2 supervisors complètent AVANT que tous les 8 foreground aient fini.
Mesurable via les timestamps `0x0D` (InferenceResponse) dans le log causal.

### A2 — Pas de famine
Tous les 8 foreground completent dans le budget temps total (< 5s mesuré).

### A3 — FIFO intra-classe (E1)
Parmi les foreground, l'agent admis en premier (admission_seq le plus bas) est servi en premier.
Vérifiable via les timestamps `0x0C` dans le log et l'ordre des `0x0D`.

## Test Rust

Le test `tests::s5_fairness_priority` dans `poc/runtime/src/lib.rs` :
- Spawn 10 acteurs simultanément (8 Foreground + 2 Supervisor)
- Utilise le runtime `current_thread` pour déterminisme
- Vérifie les 3 assertions ci-dessus

## Référence

- ADR-0021 : convention scénarios
- ADR-0022 : InferenceQueue bornée — politique de priorité et rejet
- ADR-0023 : équité formelle — E1 FIFO, E3 absence de famine bornée
