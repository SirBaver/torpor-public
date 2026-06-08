# S3 — Borne dure du pool d'inférence

## Ce qui est testé

N=12 workers identiques (`density_worker`) lancés simultanément avec un
pool d'inférence borné à k=4. Démontre :

- La **borne dure** sur le nombre d'inférences concurrentes via
  sémaphore Tokio.
- L'état `WaitingInference` observable dans le log causal.
- **Absence de famine définitive** : tous les workers finissent par
  passer et se terminer.

## Acteur

| Nom | Source | Rôle |
|-----|--------|------|
| `density_worker` | `agent-sdk/examples/density_worker.rs` | Appelle `agent_infer` une fois, émet la réponse, termine |

## Protocole

```
process(*):
  agent_infer("Count from 1 to 3.")
  barrier + emit InferenceResponse (réponse tronquée à 64 bytes)
  terminate
```

## Configuration

| Paramètre | Valeur test | Description |
|---|---|---|
| `N_WORKERS` | 12 | Nombre de workers simultanés |
| `POOL_CAP` | 4 | Max inférences concurrentes |
| `DELAY_MS` | 100 | Délai `SleepyBackend` (ms) |

Borne théorique sur la durée totale : `ceil(N/POOL_CAP) × DELAY_MS ≈
300 ms` + overhead WASM/scheduler.

## Assertions

- Exactement **12 `InferenceRequest (0x0C)`** dans le log (tous agents).
- Exactement **12 `InferenceResponse (0x0D)`** dans le log.
- `pool.active_count() == 0` à la fin (aucun slot retenu).

## Ce qui n'est PAS testé

- **Équité (FIFO strict).** L'ordre dans lequel les workers acquièrent un
  slot n'est pas vérifié. Le sémaphore Tokio est FIFO en pratique mais
  on ne l'asserte pas. Couvert Phase 6 (C1.3 spec/07).
- **Priorité sémantique.** Pas de hiérarchie de priorité testée
  (spec/07 §C1.3).
- **Borne sur la latence d'attente.** On vérifie que tous finissent,
  pas que le pire cas reste sous un budget temporel.
- **Absence de famine sous charge soutenue.** Burst unique de 12
  workers. Pas de scénario où de nouveaux workers arrivent en continu
  pendant que d'autres sont déjà en attente.
- **Backpressure réseau Ollama.** `SleepyBackend` simule la latence
  uniformément ; le comportement face à un serveur Ollama lent ou en
  rate-limit n'est pas couvert.
- **Préemption.** Une fois qu'un worker tient le slot, rien ne peut le
  lui retirer avant qu'il n'ait fini (l'annulation via scheduler.rollback
  est testée en S4, pas en S3).

## Comment relancer

```sh
cd poc/
export CXXFLAGS="-include cstdint"
cargo build --target wasm32-unknown-unknown -p agent-sdk --examples --release
cargo test -p os-poc-runtime --release -- tests::s3_inference_cap --exact
```

Durée typique : ~3,5 s.

## Prérequis

- Rust + cible `wasm32-unknown-unknown`.
- Pas d'Ollama nécessaire (`SleepyBackend`).

## Références

- ADR-0019 — `InferencePool`, `agent_infer`, lifecycle.
- spec/07 §C1 — borne dure vs équité.
- `poc/runtime/src/lib.rs :: tests::s3_inference_cap`.
