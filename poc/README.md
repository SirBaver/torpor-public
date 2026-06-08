# PoC — Substrat cible

Implémentation Rust/Wasmtime/RocksDB. État : **PoC E2E complet** (2026-05-16) — 53 tests verts, 4/4 scénarios pass, ADR-0001–0025 mergés.

> **Périmètre du QUICKSTART.** Le `cargo build --workspace` ne couvre que la stack Linux (`store`, `causal-log`, `runtime`, `capabilities`, `benchmarks`, `reconstruct`, `agent-sdk`, `sel4-smoke`, `scenarios`). Les sous-arbres **`sel4-hello/`, `redb-fork/` et `redb-p3a/`** exigent la toolchain seL4/AArch64 (cross-compilation, QEMU `virt`, rust-sel4) : ils **ne se construisent pas** via le QUICKSTART et ne font pas partie du workspace cargo. Ce sont les artefacts des jalons C.1→C.11-prov, fournis pour inspection et reproductibilité hardware-gated, pas pour un build immédiat.

## Build / environnement

> **GCC récent (≥ 13) :** la dépendance vendored `librocksdb-sys 0.16.0+8.10.0` (RocksDB 8.10) ne compile pas telle quelle — RocksDB 8.10 n'inclut plus `<cstdint>` transitivement (`error: 'uint64_t' has not been declared`). Préfixer **toute** commande cargo touchant `poc/` par :
>
> ```
> CXXFLAGS="-include cstdint" cargo test|build|bench ...
> ```
>
> Workaround non-invasif (force-include côté C++ uniquement). **Ne pas** ajouter `CFLAGS="-include stdint.h"` : ça casse la compilation assembleur (`.S`) de `zstd-sys`. Seul `CXXFLAGS` est nécessaire ; le défaut est purement dans le code C++ de RocksDB.

## Architecture des modules

| Module | Rôle | Propriété couverte |
|---|---|---|
| `causal-log/` | Log append-only sur RocksDB (EmitType 0x01–0x12) | P3 — traçabilité causale |
| `store/` | Store content-addressed (Merkle DAG), rollback p95=99 µs sur W2 | P2 — rollback transactionnel |
| `capabilities/` | Tracking + arbre de dérivation, révocation lazy en chaîne | P4 — capabilities révocables |
| `runtime/` | Wasmtime + scheduler Tokio, `InferencePool`, watchdog epoch | P1 densité, P6 atomicité |
| `reconstruct/` | `os-poc-reconstruct` : lecture humaine du log causal | Observabilité |
| `agent-sdk/` | Crate Rust→WASM : wrappers A1–A4, `agent_infer`, `AgentProfile` | Surface agent |

## ABI `agent_infer` (ADR-0019, figée)

Signature WASM côté agent :

```
agent_infer(prompt_ptr, prompt_len, timeout_ms, cancel_ptr, out_ptr, out_len) → i32
```

Codes retour : `0=Ok`, `1=Timeout`, `2=Error`, `3=NoSlot`, `4=Cancelled`.

Synchrone côté agent WASM, async côté hôte Tokio. L'`InferencePool` borne le nombre d'inférences concurrentes (`max_concurrent_inferences`, défaut 4).

## EmitType — séquence causale

| Code | Nom | Émetteur |
|---|---|---|
| 0x01 | Spawned | Runtime |
| 0x02 | ActionResult | Agent |
| 0x03 | CommitBarrier | Runtime |
| 0x04 | SchedulerRollbackApplied | Runtime |
| 0x05 | CapRevoke | Runtime |
| 0x06 | Introspect | Agent |
| 0x07 | SelfRollback | Agent |
| 0x08 | ValidationRequest | Agent |
| 0x09 | ValidationVerdict | Supervisor |
| 0x0A | Suspended | Runtime |
| 0x0B | SchedulerRollback | Runtime |
| 0x0C | InferenceRequest | Runtime (enrichi : `priority_class`, `queue_depth_at_admission`) |
| 0x0D | InferenceResponse | Runtime |
| 0x0E | InferenceCancelled | Runtime |
| 0x0F | InferenceFailed | Runtime |
| 0x11 | CompensationOpen | Runtime (atomicité crash ADR-0024) |
| 0x12 | CompensationClose | Runtime (atomicité crash ADR-0024) |

## Scénarios E2E (convention ADR-0021)

| Scénario | Primitives exercées | Backend CI |
|---|---|---|
| S1 `supervision-algorithmique` | `agent_infer`, `request_validation`, `get_verdict` | `FixedResponseBackend` |
| S2 `self-rollback-incoherence` | `agent_introspect (A1)`, `agent_self_rollback (A2)` | `FixedResponseBackend` |
| S3 `inference-cap` | Pool k=4, `WaitingInference`, absence de famine | `SleepyBackend(100ms)` |
| S4 `scheduler-rollback` | Rollback pendant `WaitingInference`, cancellation, D5+D8 | `SleepyBackend(60s)` |

Convention pour ajouter un scénario S<N> :
- `poc/scenarios/S<N>-<slug>/README.md`
- Test Rust `tests::s<N>_<slug>` dans `poc/runtime/src/lib.rs`
- Agents dans `poc/agent-sdk/examples/`
- Backend : `FixedResponseBackend` ou `SleepyBackend` (jamais Ollama en CI)

## Watchdog WASM (ADR-0025)

L'agent déclare son `AgentProfile` au spawn. Le runtime calibre les constantes watchdog en conséquence :

| Profil | Plafond wallclock par `process_one` |
|---|---|
| `Algo` | 100 ms |
| `LlmShort` | 5 s (défaut PoC E2E) |
| `LlmLong` | 30 s |
| `Batch` | 5 min |

Mécanisme : `epoch_interruption` Wasmtime + thread bg `increment_epoch` toutes les `EPOCH_TICK_MS_BASE = 10 ms`. Profil émis dans `Spawned (0x01)` pour traçabilité.

## File d'inférence bornée (ADR-0022 + ADR-0023)

Trois classes de priorité : `Supervisor > Foreground > Batch`. FIFO intra-classe (E1), absence de famine bornée `max_wait_ms = 30 s`, `max_starvation_ms = 10 s` (E3). Politique de rejet : drop-newest avec éviction Batch si file pleine → `NoSlot (3)`.

## Atomicité crash (ADR-0024)

Le couple `(InferenceCancelled 0x0E, SchedulerRollback 0x0B)` est encadré par `CompensationOpen (0x11)` / `CompensationClose (0x12)`. Un crash entre les deux laisse un `0x11` sans `0x12` correspondant, détecté par `os-poc-reconstruct` au recovery (politique `auto-close + warning`). Test via trait `CrashPoint` feature-gated (`crash-injection`).

## Backends d'inférence

| Backend | Usage |
|---|---|
| `OllamaBackend` | Production + tests manuels avec LLM réel (hors CI) |
| `SleepyBackend(duration)` | Tests de timing et de borne (S3, S4) |
| `FixedResponseBackend(response)` | Tests déterministes (S1, S2) |

## Benchmarks

### T5 — Latence causale à 10⁸ entrées

```bash
BENCH_N=100000000 cargo bench --bench causal_lookup -p os-poc-causal-log
```

Résultats actuels (K=4 runs NVMe AWS, 2026-05-15) : p99 371–502 µs. Objectif P3a : p99 ≤ 10 ms.
Statut : **partiellement validé** (1 instance cloud, Modèle A uniforme — portée limitée).
Prochaine étape : T5-qualif sur NVMe dédié K≥3 runs, avec Modèle B (recency-biased).

### T6 — Densité Wasmtime vs Docker

Sur hôte Linux nu uniquement (ne peut pas s'exécuter depuis Docker) :

```bash
cargo run -p os-poc-benchmarks -- t6-density
```

Résultats dev (2026-05-14) : ratio Wasmtime/Docker-Python = 8 670×. Objectif P1 : ≥ 5×.
Statut : **qualitativement validé** (1 run dev, hors environnement de qualification).

## Références

- Design et propriétés formelles : `spec/`
- Chaîne de décisions : `decisions/INDEX.md`
- Leçons empiriques : `lab/LESSONS.md`
- Dettes actives et préconditions : `TODO.md`
