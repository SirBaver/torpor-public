# S7 — Rollback transactionnel (SEF-2 / P2)

## Objectif

Vérifier la propriété **P2 — Rollback transactionnel** (`spec/02-properties.md §P2`)
par observation systémique :

> Après 1 000 actions, un rollback à l'action n°500 produit un état dont le hash
> est identique à celui mesuré après l'action n°500.

La propriété combinée à l'API `Scheduler::rollback` (ADR-0007 / D5 / ADR-0024)
exerce, dans un seul scénario d'intégration, la chaîne :

1. `commit_barrier` + `emit` (host functions actor.rs)
2. ContentStore : `put_block` / `put_snapshot` / `rollback_path`
3. CausalLog : `append` du LogEntry `ActionResult` puis `SchedulerRollback`
4. `Scheduler::rollback` + journal de compensation (0x11 / 0x12)
5. `Message::Rollback` → `run_loop` → restauration `last_snapshot` + révocation caps

## Pourquoi un seul processus (pas victim/verify)

SEF-2 ne teste pas un crash, donc pas besoin du pattern `victim+verify` de SEF-4.
La capture du `hash_at_k` et la vérification post-rollback se font dans le même
processus pour deux raisons techniques :

- Le `SnapshotHeader.ts_us` n'est pas reproductible entre processus distincts,
  donc `hash_at_k` n'est pas pré-calculable hors run.
- L'opération de rollback ne réécrit jamais un block du store (un store
  content-addressed est immuable) : elle repointe `last_snapshot` vers un
  `SnapshotHeader` *existant* identifié par son hash. Le « hash identique »
  attendu par P2 est donc trivialement vrai par construction *si* la chaîne
  content-addressed est cohérente. C'est précisément ce que les propriétés
  ci-dessous testent — la cohérence de la chaîne ET son usage par
  `Scheduler::rollback`.

## Propriétés vérifiées par run

| Code | Propriété | Mesure |
|------|-----------|--------|
| P-α  | `SchedulerRollback.hash_after == hash_at_k` | Trivial sous content-addressed, mais exerce `rollback_path` sur 500 sauts. Falsifié si la chaîne ContentStore est brisée. |
| P-β  | `SnapshotHeader(hash_at_k).seq == k - 1`     | Falsifié si l'indexation seq est cassée. |
| P-γ  | Payload `SchedulerRollback.target_seq == k - 1` | Falsifié si `Scheduler::rollback` envoie le mauvais target_seq ou si le décodage payload est cassé. |
| P-δ  | Action post-rollback : `hash_before == hash_at_k` | **C'est la propriété forte** : la prochaine action après rollback reprend bien depuis l'état restauré. Falsifié si `Message::Rollback` n'a pas correctement mis à jour `last_snapshot` côté actor. |
| P-ε  | Durée rollback ≤ budget (100 ms par défaut) | Borne de performance P2. Mesurée entre l'appel `Scheduler::rollback` et l'apparition de SchedulerRollback (0x0B) dans le log. |

Le verdict pass = les 5 propriétés vraies.

## Configuration

| Paramètre              | Valeur |
|------------------------|--------|
| Agent WASM             | `AGENT_WAT` (commit_barrier + emit) |
| Payload de chaque message | `b"sef2-XXXXXXXX"` (numéroté, déterministe par index) |
| N (actions)            | 1 000  |
| K (action cible)       | 500    |
| Budget rollback        | 100 ms (= borne P2) |
| Répétitions            | 5      |
| Agent ID               | varie par run pour éviter collisions secondary index |
| Runtime                | Tokio current_thread |

## Critère d'acceptation

Pour chaque répétition r ∈ 1..=K_RUNS :
- `sef2-runner` doit se terminer avec exit code 0 (les 5 propriétés vraies).

Total : 5 runs. Verdict global = pass si 5/5 passent.

## Exécution

```bash
cd poc
bash scenarios/S7-rollback-equivalence/run.sh
```

Sortie attendue (release) :

```
[S7] Compilation des binaires (release)...
[S7] run 1/5: pass (rollback=17ms)
[S7] run 2/5: pass (rollback=15ms)
...
[S7] Verdict global : 5/5 pass
```

Exit code 0 si 5/5 pass.

## Sortie

`scenarios/S7-rollback-equivalence/report.json` après run :

```json
{
  "timestamp": "...",
  "scenario": "S7-rollback-equivalence",
  "n_actions": 1000,
  "k_target": 500,
  "rollback_budget_ms": 100,
  "k_runs": 5,
  "passed": 5,
  "total": 5,
  "rollback_duration_ms": [17, 15, ...],
  "verdict": "pass"
}
```

## Note sur le hardware

Le budget de 100 ms est la borne P2 de spec/02. Sur la machine de référence
(AMD Ryzen 5 PRO 4650U + WD SN530 NVMe, classe 2), les valeurs typiques en
release tournent autour de 15–30 ms (rollback_path = 500 lookups RocksDB cache
chaud + un append log). Sur hardware moins favorable, le budget peut être
relevé via `--rollback-budget-ms`.

Note : la durée mesurée inclut un overhead de polling (sleep 5ms entre lectures
du log), donc la durée mesurée est une borne supérieure de la durée réelle du
rollback côté noyau. Pour une mesure stricte de P2, voir le bench dédié
`poc/store/benches/rollback_latency.rs` (H-rollback-latence — p95 = 99 µs sur N=10⁶).

## Références

- `spec/02-properties.md §P2` — propriété de rollback transactionnel.
- ADR-0007 — Capabilities révoquées au rollback (D8).
- ADR-0024 — Journal de compensation (CompensationOpen 0x11 / Close 0x12).
- D5 — Câblage `Message::Rollback`, `SchedulerRollback` 0x0B.
- `poc/runtime/src/scheduler.rs::Scheduler::rollback` — orchestration.
- `poc/runtime/src/actor.rs` (Message::Rollback branch) — application.
- `poc/runtime/src/bin/sef2_runner.rs` — binaire de ce scénario.
