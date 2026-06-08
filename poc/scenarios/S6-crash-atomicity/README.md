# S6 — Crash atomicity (SEF-4 / ADR-0024 / ADR-0027)

## Objectif

Vérifier la propriété **P6 — Atomicité crash** par observation, sous le régime de
menace défini par ADR-0027 §D3 :

- **Couvert** : SIGKILL / `std::process::exit` / panic Rust.
  Page cache OS préservé → toutes les écritures RocksDB depuis l'ouverture survivent
  (rejouées par recovery WAL au redémarrage).
- **Hors scope** : power-loss (coupure secteur, kernel panic, hardware fault).
  Phase 7+ — nécessite fsync coordonné sur ContentStore (cf. ADR-0027 D4).

L'agent exécute une séquence déterministe de N actions `commit_barrier + emit` (AGENT_WAT —
ADR-0010 §S4). Pour chaque point de kill exposé par `crash_point.rs` et chaque action
cible, on tue le processus, puis on rouvre la DB et on vérifie que l'état observable
(via le log) est l'un des deux états admissibles :

- `hash_ref_pre[k]`   — l'action `k` n'a laissé aucune trace côté log
- `hash_ref_pre[k+1]` — l'action `k` est entièrement committed (ContentStore + log)

Aucun état intermédiaire (ContentStore partiellement écrit visible côté log, snapshot
référencé sans block, etc.) ne doit jamais être l'état observé.

## Points de kill testés

Définis dans `poc/runtime/src/crash_point.rs` (variant `CrashPoint`) :

| Nom CLI                                  | Site                                         | État log attendu post-recovery |
|------------------------------------------|----------------------------------------------|--------------------------------|
| `pre_put_block`                          | host fn `commit_barrier`, avant `put_block`  | `hash_ref_pre[k]`              |
| `between_put_block_and_put_snapshot`     | entre `put_block` et `put_snapshot`          | `hash_ref_pre[k]` (orphan block) |
| `post_put_snapshot_pre_log_append`       | après `put_snapshot`, avant `emit`/`append`  | `hash_ref_pre[k]` côté log (store en avance — asymétrie ADR-0027 §Coût) |
| `post_log_append`                        | host fn `emit`, après `CausalLog::append`    | `hash_ref_pre[k+1]`            |

Note importante : la propriété SEF-4 est **« observed ∈ {pre[k], pre[k+1]} »** — pas
**« observed = exactement la valeur attendue »**. Sous SIGKILL le page cache OS survit,
donc en pratique :

- `pre_put_block` → toujours `pre[k]` (rien écrit).
- `between_put_block_and_put_snapshot` → toujours `pre[k]` (block orphan, pas dans la chaîne).
- `post_put_snapshot_pre_log_append` → toujours `pre[k]` côté log (l'append n'a pas eu lieu).
- `post_log_append` → toujours `pre[k+1]` (append committed, WAL OS-buffered survit).

Sous power-loss (hors scope, Phase 7+) la situation serait différente — le fsync n'a
pas eu lieu, le WAL peut être tronqué. L'écart entre les deux régimes est précisément
le coût documenté ADR-0027 §D4.

## Configuration

| Paramètre        | Valeur |
|------------------|--------|
| Agent WASM       | `AGENT_WAT` (commit_barrier + emit, ADR-0010 §S4) |
| Payload          | `b"sef4"` (constant, déterministe)                |
| N (actions)      | 10                                                |
| K (répétitions par kill_point × kill_action) | 5                       |
| Kill actions     | 3 et 4 (cibles internes de la séquence)           |
| Agent ID         | `00000000000000000000000000000001`                |
| Runtime          | Tokio current_thread (déterminisme)               |

Le couple (3, 4) couvre l'agent « en croisière » (snapshot précédent existe). L'action
0 est volontairement évitée : `last_snapshot = None` change la sémantique de la
chaîne de parents (hash_before = `[0u8;32]`).

## Critère d'acceptation

Pour chaque (kill_point ∈ 4 points, kill_action ∈ {3, 4}, run ∈ 1..=K) :

- `sef4-victim` doit se terminer avec exit code 1 (déclenché par `crash_point::fire`).
- `sef4-verify` doit se terminer avec exit code 0 (pass).

Total : 4 × 2 × 5 = **40 runs**. Verdict global = pass si les 40 passent.

## Exécution

```bash
cd poc
bash scenarios/S6-crash-atomicity/run.sh
```

Sortie attendue :

```
[S6] Reference run (N=10)
[S6] Kill point pre_put_block, action=3, run=1/5: pass
[S6] Kill point pre_put_block, action=3, run=2/5: pass
...
[S6] Verdict global : 40/40 pass
```

Exit code 0 si 40/40 pass, 1 sinon.

## Sortie

`scenarios/S6-crash-atomicity/report.json` est écrit à la fin :

```json
{
  "timestamp": "...",
  "kill_points": ["pre_put_block", "between_put_block_and_put_snapshot",
                  "post_put_snapshot_pre_log_append", "post_log_append"],
  "kill_actions": [3, 4],
  "k_runs": 5,
  "passed": 40,
  "total": 40,
  "verdict": "pass"
}
```

## Références

- ADR-0024 — Atomicité crash `(InferenceCancelled, SchedulerRollback)` — pattern failpoint
  et trait `CrashPoint`.
- ADR-0027 — Régime de durabilité du log causal vs ContentStore (SIGKILL/panic vs power-loss).
- `spec/02-properties.md §P6` — propriété d'atomicité crash.
- `poc/runtime/src/crash_point.rs` — points d'injection (4 nouveaux ajoutés SEF-4).
- `poc/runtime/src/bin/sef4_victim.rs` — binaire exécutant la séquence et tuant.
- `poc/runtime/src/bin/sef4_verify.rs` — binaire de vérification post-recovery.
