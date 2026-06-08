# S2 — Self-rollback sur incohérence détectée

## Ce qui est testé

Un agent LLM (`worker_double_check`) détecte que sa propre réponse
contredit une vérité interne (vérification déterministe) et déclenche
son propre rollback. Le chemin heureux démontre la composition
**A1 (`agent_introspect`) + A2 (`agent_self_rollback`)** :

- A1 : l'agent interroge son état (numéro de séquence courant) avant
  d'agir.
- A2 : l'agent décide d'annuler son dernier snapshot.
- L'enchaînement `Introspect(0x06) → SelfRollback(0x07)` est observable
  dans le log causal.

## Acteur

| Nom | Source | Rôle |
|-----|--------|------|
| `worker_double_check` | `agent-sdk/examples/worker_double_check.rs` | Infer → introspect → self_rollback si incohérent |

## Protocole (une seule phase)

```
process([n: u8]):
  1. agent_infer("Is n prime? JSON: {is_prime: true/false}")
  2. barrier + emit provisional ActionResult       (seq → 1)
  3. agent_introspect (A1) → lit seq courant
  4. barrier + emit Introspect (0x06)              (seq → 2)
  5. is_prime_rust(n) — vérification déterministe
  6a. LLM incorrect → agent_self_rollback(1) → barrier + emit résultat corrigé
  6b. LLM correct   → barrier + emit résultat confirmé
```

## Données de référence

`n = 39`. `FixedResponseBackend` retourne `{"is_prime": true}`
(revendication erronée). `is_prime_rust(39) = false`. Chemin **6a**
toujours emprunté de façon déterministe.

Assertions :
- `Introspect (0x06)` présent dans le log.
- `SelfRollback (0x07)` présent dans le log.
- Résultat final : `{"is_prime":null,"reason":"self_rollback_after_llm_error"}`.

## Ce qui n'est PAS testé

- **Stabilité LLM sous variabilité réelle.** Le test force déterministe ;
  un agent réel qui passerait du faux au vrai entre deux exécutions
  serait flaky. Voir `docs/archive/poc_E2E.md` §B6 et L23 LESSONS.
- **`self_rollback(depth > 1)`** — seul `depth=1` est testé ici.
- **Borne de profondeur du rollback.** ADR-0007 limite la profondeur ;
  les rejets dus à un `depth` trop grand ne sont pas couverts par S2.
- **Restauration des messages déjà envoyés à d'autres agents.** Le
  self-rollback restaure l'état store local de l'agent — il ne
  rappelle pas les messages déjà partis (D3 ouverte, cf. memory
  `project_rollback_dangling`).
- **Atomicité crash pendant le rollback.** Si le runtime crashe entre
  l'application du snapshot et l'émission de `SelfRollback`, l'état au
  redémarrage n'est pas explicitement testé.

## Comment relancer

```sh
cd poc/
export CXXFLAGS="-include cstdint"
cargo build --target wasm32-unknown-unknown -p agent-sdk --examples --release
cargo test -p os-poc-runtime --release -- tests::s2_self_rollback_incoherence --exact
```

## Prérequis

- Rust + cible `wasm32-unknown-unknown`.
- Pas d'Ollama nécessaire (déterministe).

## Références

- ADR-0007 — invalidation des caps post-rollback.
- ADR-0019 — `agent_infer` ABI.
- `poc/runtime/src/lib.rs :: tests::s2_self_rollback_incoherence`.
- `scenarios/S2-self-rollback-incoherence/reference_responses.jsonl` —
  réponses LLM de référence pour debug.
