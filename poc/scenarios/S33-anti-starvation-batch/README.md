# S33 — Anti-famine Batch→Foreground (UC-14 / ADR-0023 / P1b)

**Régime :** R2 (P1b — densité active, équité du pool d'inférence)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P1b (anti-famine Batch→Foreground)** : un agent `Batch` affamé par un flux
`Foreground` continu est promu en classe `Foreground` après `max_starvation_ms`
d'attente (ADR-0023). Sans cette promotion, l'agent Batch ne serait jamais servi
tant que la file Foreground est non-vide (priorité stricte Foreground > Batch).

### Mécanisme

`InferenceQueue::pop_next()` appelle `apply_starvation_promotions()` à chaque
dépilement. Les entrées Batch qui ont attendu ≥ `max_starvation_ms` sont promues
en Foreground (déplacées en tête de file Foreground, flag `promoted=true`). Une
entrée Batch promue ne peut pas être promue à nouveau (`promoted=true` bloque la
2ème promotion Foreground→Supervisor).

---

## Timing déterministe (current_thread, comme S5)

```
t=0ms   : FG1 soumis → obtient le slot (SleepyBackend 300ms, pool cap=1).
t=20ms  : Batch soumis → file Batch (FG1 tient le slot).
t=240ms : sleep(220ms) — Batch a attendu ≈220ms > 200ms (max_starvation_ms).
t=240ms : FG2 soumis → file Foreground.
t≈320ms : FG1 termine → pop_next → apply_starvation_promotions :
           Batch (≈300ms > 200ms) promu Foreground front.
           FG2 (≈80ms < 200ms) non promu, reste en Foreground.
           → Batch servi avant FG2.
```

**Pourquoi `current_thread` ?** Dans `multi_thread`, le `_permit` (semaphore) n'est
pas encore libéré quand `try_acquire_owned()` s'exécute dans le dispatcher — race
condition entre la notification et la libération effective. En `current_thread` les
tâches coopèrent : le spawned task drop son permit avant que le dispatcher reprenne.

---

## Oracles

```
Oracle 1 (équité)   : Batch reçoit InferenceResponse (0x0D) — non affamé.
Oracle 2 (promotion): pool.queue_stats().total_promoted >= 1.
Oracle 3 (ordre)    : ts_us(Batch InferenceResponse) < ts_us(FG2 InferenceResponse).
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Promotion Foreground→Supervisor | Même mécanisme, couvert par `t_queue_starvation_promotion` |
| Promotion bornée (promoted ne peut être promu 2×) | `t_promotion_is_bounded_one_step` (queue.rs) |
| Pool réel Ollama (C2 recalibré) | UC-21 ⏳ Phase 10 |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s33_anti_starvation_batch_promoted --exact --nocapture
```

---

## Références

- **ADR-0023** — Anti-famine bornée : politique de promotion Batch→Foreground.
- **ADR-0022** — `InferenceQueue` multi-niveau (Supervisor > Foreground > Batch).
- `poc/runtime/src/inference/queue.rs` `apply_starvation_promotions` — implémentation.
- `poc/scenarios/S5-fairness-priority/` — pendant Supervisor > Foreground.
- `poc/runtime/src/lib.rs::tests::s33_anti_starvation_batch_promoted` — test.
