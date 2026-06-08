# Scénarios d'intégration du PoC OS-pour-IA

Scénarios bout-en-bout qui démontrent les primitives clés du PoC : supervision
algorithmique, self-rollback, borne dure d'inférence, rollback scheduler avec
révocation de capabilities, équité de file d'inférence, atomicité crash.
Voir `docs/archive/poc_E2E.md` §4 pour le brief complet.

## Vue d'ensemble

| Scénario | Primitive démontrée | Acteurs | Backend LLM | Test Rust |
|---|---|---|---|---|
| **S1** | Supervision algorithmique (verdict superviseur déterministe) | `worker_prime` + `supervisor_arith` | `FixedResponseBackend` | `tests::s1_supervision_algorithmique` |
| **S2** | Self-rollback sur incohérence détectée (A1+A2) | `worker_double_check` | `FixedResponseBackend` | `tests::s2_self_rollback_incoherence` |
| **S3** | Borne dure du pool d'inférence (k=4 concurrent) | 12× `density_worker` | `SleepyBackend(100ms)` | `tests::s3_inference_cap` |
| **S4** | Rollback scheduler pendant `WaitingInference` + révocation cap (D5, D8) | `rollback_target` | `SleepyBackend(60s)` | `tests::s4_scheduler_rollback` |
| **S5** | File d'inférence bornée — priorité (Supervisor > Foreground > Batch), équité E1+E3 (ADR-0022/0023) | 8× Foreground + 2 Supervisor | `SleepyBackend(100ms)` | `tests::s5_fairness_priority` |
| **S6** | Atomicité crash P6 (SEF-4 / ADR-0024 + ADR-0027 régime SIGKILL) — 4 kill_points × 2 actions × K=5 = 40 runs | `AGENT_WAT` minimal | aucun | `S6-crash-atomicity/run.sh` (binaires `sef4-victim` + `sef4-verify`) |
| **S7** | Rollback transactionnel P2 (SEF-2) — 1 000 actions, rollback à 500, 5 propriétés (P-α à P-ε), K=5 | `AGENT_WAT` minimal | aucun | `S7-rollback-equivalence/run.sh` (binaire `sef2-runner`) |
| **S8** | Déterminisme de transition d'état P5 (SEF-6 / ADR-0028) — 2 instances, 1 000 messages, `LogicalClock` substitué, 3 propriétés (P-α à P-γ), K=5 | `AGENT_WAT` minimal | aucun | `S8-determinism/run.sh` (binaire `sef6-runner`) |

Aucun scénario ne dépend d'Ollama : les backends utilisés sont
déterministes (`FixedResponseBackend`) ou simulés (`SleepyBackend`). Cf.
ADR-0021 §reproductibilité.

## Lancer les scénarios S1–S5

```sh
cd poc/
bash scenarios/run-all.sh
cat scenarios/report.json
echo "exit=$?"
```

Sortie attendue (rapport `scenarios/report.json`) :

```json
{
  "timestamp": "...",
  "verdicts": {
    "S1-supervision-algorithmique": "pass",
    "S2-self-rollback-incoherence": "pass",
    "S3-inference-cap": "pass",
    "S4-scheduler-rollback": "pass",
    "S5-fairness-priority": "pass"
  },
  "summary": "5/5 passed"
}
```

Exit code : 0 si les cinq passent, 1 sinon, 2 si la compilation des
agents WASM échoue avant exécution.

## Lancer S6 (atomicité crash — SEF-4)

S6 a un harness séparé parce qu'il fork-execute un binaire qui s'auto-kill
(`process::exit(1)`) — incompatible avec un test Rust qui partagerait le
harness avec d'autres tests.

```sh
cd poc/
bash scenarios/S6-crash-atomicity/run.sh
cat scenarios/S6-crash-atomicity/report.json
```

Requiert `--features crash-injection` (géré par le script). Voir
`scenarios/S6-crash-atomicity/README.md` pour les détails.

## Lancer S7 (rollback transactionnel — SEF-2)

S7 vérifie P2 sur 1 000 actions avec rollback à l'action 500, sur 5 runs.

```sh
cd poc/
bash scenarios/S7-rollback-equivalence/run.sh
cat scenarios/S7-rollback-equivalence/report.json
```

## Lancer S8 (déterminisme de transition d'état — SEF-6)

S8 vérifie P5 en lançant deux instances avec une horloge logique identique
(`LogicalClock`, ADR-0028) et en comparant hash final + séquence d'action_ids.

```sh
cd poc/
bash scenarios/S8-determinism/run.sh
cat scenarios/S8-determinism/report.json
```

Aucune feature requise. Voir `scenarios/S8-determinism/README.md` pour les
détails et l'audit complet des sources de non-déterminisme corrigées.

## Structure d'un scénario

```
scenarios/
├── README.md                            # ce fichier
├── run-all.sh                           # harness commun (B9)
├── report.json                          # généré, ignoré par git
├── S1-supervision-algorithmique/
│   ├── README.md
│   └── reference_responses.jsonl        # réponses LLM de référence (debug)
├── S2-self-rollback-incoherence/
│   ├── README.md
│   └── reference_responses.jsonl
├── S3-inference-cap/
│   └── README.md
└── S4-scheduler-rollback/
    └── README.md
```

Le code d'exécution réel vit dans `poc/runtime/src/lib.rs` (tests
d'intégration) et `poc/agent-sdk/examples/*.rs` (agents WASM). Cette
arborescence ne contient que la documentation et les harness.

Convention de nommage et de structure : voir
[`decisions/0021-convention-scenarios.md`](../../decisions/0021-convention-scenarios.md).

## Prérequis

- Rust toolchain stable + cible `wasm32-unknown-unknown` (`rustup target
  add wasm32-unknown-unknown`).
- `CXXFLAGS="-include cstdint"` sur GCC 15.x (cf. memory
  `project_gcc15_cxxflags`).
- Aucun service externe : pas d'Ollama, pas de réseau.

## Ce que les scénarios ne couvrent PAS (collectivement)

- **Stabilité LLM sous variabilité réelle.** Les backends sont
  déterministes pour rendre les tests reproductibles. La variabilité
  qwen2.5:3b vs llama3.2:3b est mesurée séparément (cf. L23 LESSONS).
- **Équité du pool d'inférence (C1).** S3 démontre la borne dure
  uniquement. L'ordre FIFO strict, la priorité sémantique et l'absence
  de famine sous charge soutenue sont réservés Phase 6.
- **Atomicité crash.** Aucun scénario n'arrête le runtime en cours
  d'opération pour vérifier que l'état repris est cohérent.
- **Watchdog WASM.** Le watchdog (timeout d'agent qui boucle) est testé
  séparément (`tests::t2_watchdog_traps_infinite_loop_agent`), pas dans
  les scénarios S1–S4.
- **Concurrence multi-hôte.** Tout tourne dans un seul processus
  Wasmtime/Tokio.

## Référence brief

- `docs/archive/poc_E2E.md` v3 §4 — Semaine 5 (B9, B10, livrables).
- `decisions/0021-convention-scenarios.md` — convention de scénarios.
- `lab/LESSONS.md` §L46–L49 — surprises rencontrées pendant les
  semaines 1–4.
