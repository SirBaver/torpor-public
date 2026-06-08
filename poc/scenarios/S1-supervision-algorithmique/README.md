# S1 — Supervision algorithmique

## Ce qui est testé

Un agent LLM (`worker_prime`) émet une revendication provisoire sur la
primalité d'un entier, un superviseur déterministe (`supervisor_arith`)
calcule la vérité et émet un verdict, le worker finalise son résultat en
fonction du verdict. Le chemin heureux démontre :

- L'ABI `agent_infer` (ADR-0019) : appel synchrone côté agent, traité de
  manière asynchrone par l'`InferencePool` côté hôte.
- Le canal de validation A3 : `request_validation` → `AwaitingValidation`
  → `ValidationVerdict` ramené à l'agent via `get_verdict`.
- La composition cross-agent : deux agents distincts dont les chaînes
  causales se rejoignent via le mécanisme du verdict.

## Acteurs

| Nom | Source | Rôle |
|-----|--------|------|
| `worker_prime` | `agent-sdk/examples/worker_prime.rs` | Infère, émet la revendication provisoire, demande validation, finalise |
| `supervisor_arith` | `agent-sdk/examples/supervisor_arith.rs` | Calcule `is_prime(n)`, émet le verdict 0=Approved / 1=Rejected |

## Protocole (deux phases)

```
Host                Worker                    Supervisor
 │                    │                            │
 │── Data[0x01, n] ──>│                            │
 │                    │── agent_infer() ─────────> │ (FixedResponseBackend)
 │                    │<─ {"is_prime":true} ────── │
 │                    │── emit provisional ────── >│ log causal
 │                    │── request_validation ────> │ (AwaitingValidation)
 │                    │                            │
 │── Data[n, claim] ────────────────────────────── >│
 │                                                 │── is_prime(n) → verdict
 │                                                 │── emit verdict ────> log causal
 │                                                 │── terminate
 │                    │                            │
 │── ValidationResponse[verdict] ────────────────> │
 │── Data[0x02] ─────> │                           │
 │                    │── emit résultat final ───>│ log causal
 │                    │── terminate                │
```

## Données de référence

`n = 39` (= 3 × 13, non premier). `FixedResponseBackend` retourne
`{"is_prime": true}` (revendication erronée — c'est le cœur du test).
Verdict attendu : Rejected. Résultat final attendu :
`{"is_prime":null,"reason":"validation_rejected"}`.

## Ce qui n'est PAS testé

- **Stabilité LLM sous variabilité réelle.** Le backend est fixé pour
  rendre le test déterministe. La variabilité Ollama (qwen2.5:3b) est
  mesurée séparément ; voir LESSONS L23.
- **Plusieurs superviseurs concurrents** sur le même worker (M:N).
- **Timeout `AwaitingValidation`** — testé séparément
  (`tests::test_validation_timeout`, ADR-0014).
- **Cohérence du verdict avec l'état du store au moment de l'émission**
  — le superviseur opère sur la donnée passée dans le `Message::Data`,
  pas sur une lecture concurrente du store.
- **Échec de communication superviseur → worker** (perte de message).

## Comment relancer

```sh
cd poc/
export CXXFLAGS="-include cstdint"     # GCC 15.x
cargo build --target wasm32-unknown-unknown -p agent-sdk --examples --release
cargo test -p os-poc-runtime --release -- tests::s1_supervision_algorithmique --exact
```

Ou via le harness commun (cf. `scenarios/run-all.sh`).

## Prérequis

- Rust + cible `wasm32-unknown-unknown` (ADR-0020 D1).
- Pas d'Ollama nécessaire (`FixedResponseBackend` interne). Pour relancer
  avec Ollama : remplacer `FixedResponseBackend` par
  `OllamaBackend::default()` dans le test ; nécessite alors
  `ollama serve` + `ollama pull qwen2.5:3b`.

## Références

- ADR-0019 — `agent_infer` (signatures, lifecycle `WaitingInference`).
- ADR-0013 — `AwaitingValidation` / lifecycle agent.
- `poc/runtime/src/lib.rs :: tests::s1_supervision_algorithmique`.
- `scenarios/S1-supervision-algorithmique/reference_responses.jsonl` —
  échantillon de réponses LLM pour debug (pas utilisé par le test).
