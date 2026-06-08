# ADR-0020 — Toolchain agent SDK

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

Le PoC bout-en-bout (`docs/archive/poc_E2E.md` v3) requiert des modules WASM compilés
depuis Rust pour les scénarios S1–S4. Ces modules doivent importer les host
functions A1–A4 + `agent_infer` et exposer une fonction `process(i32, i32)`.
Trois décisions structurantes à prendre : (1) la cible de compilation WASM,
(2) l'organisation du code partagé (crate helper), (3) le pattern de boucle
agent.

---

## Décisions

### D1. Cible : `wasm32-unknown-unknown` (et non `wasm32-wasip1`)

**Décision.** La crate `agent-sdk` et tous les agents WASM compilés pour ce
chantier ciblent `wasm32-unknown-unknown`.

**Justification.** La recommandation initiale du brief (`wasm32-wasip1`) est
remplacée pour une raison opérationnelle bloquante : `wasmtime-wasi` est
désactivé sur l'hôte (ADR-0019 §contexte, décision structurante D-B/D-C). Un
module `wasm32-wasip1` importe automatiquement des symboles WASI
(`wasi_snapshot_preview1::proc_exit`, `fd_write`, etc.) que le runtime ne
fournit pas → échec d'instantiation.

`wasm32-unknown-unknown` + `#![no_std]` (ou std minimal) ne génère aucun
import WASI. Les modules importent uniquement les host functions A* depuis le
module `"env"`, exactement comme les modules WAT en ligne existants.

**Conséquence.** Le débogage via `stdout` (argument en faveur de wasip1) n'est
plus disponible. Mitigation : l'agent peut écrire dans le `ContentStore` (via
`commit_barrier` + `emit`) pour exposer des valeurs intermédiaires — lisibles
par `os-poc-reconstruct`. Pour les panics, le `panic_handler` boucle à
l'infini, ce qui devient un timeout Tokio côté hôte.

**Migration Phase 6.** Si un besoin de débogage natif WASM se confirme, on peut
ajouter des stubs WASI minimalistes dans le Linker (fonctions noop sauf
`proc_exit` qui retourne) et migrer vers `wasm32-wasip1`. Hors scope ici.

### D2. Crate `poc/agent-sdk/`

**Décision.** Une crate Rust `agent-sdk` dans `poc/agent-sdk/` qui expose :

- Les déclarations `extern "C"` des host functions (sous
  `#[cfg(target_arch = "wasm32")]`).
- Des wrappers Rust idiomatiques (safe, typés) autour de chaque host function.
- Un no-op gracieux sur les targets non-WASM (compilation native pour
  `cargo check`, CI sur `x86_64`).

**Structure :**

```
poc/agent-sdk/
├── Cargo.toml
├── src/
│   └── lib.rs       # wrappers A1–A4
└── examples/
    └── echo.rs      # agent minimal : introspect + emit
```

La crate n'a pas de dépendances (ni runtime, ni causal-log) — les agents
compilés doivent rester aussi petits que possible. Tout accès aux structures
Rust du runtime est hors scope (les agents interagissent exclusivement via host
functions).

**Pourquoi `rlib` et non `cdylib`.** Les agents WASM sont des *binaires*
(exemples ou `[[bin]]`), pas des bibliothèques dynamiques. `agent-sdk` est une
`rlib` (bibliothèque Rust normale). Les exemples compilés en cibles WASM
produisent des binaires `.wasm` autonomes. Ajouter `cdylib` créerait un
`agent_sdk.wasm` inutile.

### D3. Déclarations extern "C"

**Décision.** Les imports sont déclarés avec l'attribut
`#[link(wasm_import_module = "env")]` pour que le linker WASM les place dans
le module d'import `"env"` — cohérent avec les modules WAT existants
(`(import "env" "commit_barrier" ...)`).

Signatures ABI (toutes exprimées en types WASM i32/i64) :

| Host function | Signature Rust |
|---|---|
| `commit_barrier` | `() -> ()` |
| `emit` | `(emit_type: i32, ptr: *const u8, len: i32) -> ()` |
| `agent_introspect` | `(buf: *mut u8, max_len: i32) -> i32` |
| `agent_self_rollback` | `(depth: i32) -> i32` |
| `agent_request_validation` | `(risk: i32) -> i32` |
| `agent_get_verdict` | `() -> i32` |
| `agent_checkpoint` | `() -> i32` |
| `agent_terminate` | `() -> ()` |
| `agent_session_info` | `(buf: *mut u8, max_len: i32) -> i32` |
| `agent_add_cause` | `(action_id_ptr: *const u8) -> i32` |

`agent_infer` sera ajoutée en semaine 2 (B2), après validation du pipeline de
chargement WASM en semaine 1.

### D4. Pattern de boucle agent

**Décision.** Chaque agent expose une fonction `process(ptr: i32, len: i32)`
marquée `#[no_mangle] pub extern "C"`. Cette fonction est appelée par `run_loop`
pour chaque `Message::Data`. La boucle ReAct (observer → raisonner → agir) se
déroule *à l'intérieur* d'un appel `process`, pas entre plusieurs appels —
valide pour les scénarios S1–S4.

**Stub `main`.** Les exemples Cargo nécessitent un `fn main()` sur les targets
hôtes. On ajoute un stub `fn main() {}` conditionné sur
`#[cfg(not(target_arch = "wasm32"))]` pour permettre `cargo check` sans target
explicite. Sur wasm32, le symbole `main` n'est pas exporté.

### D5. Build reproductible

**Commande de référence :**

```sh
cargo build --target wasm32-unknown-unknown \
            -p agent-sdk \
            --examples \
            --release
```

Output : `target/wasm32-unknown-unknown/release/examples/<name>.wasm`

Un script `poc/agent-sdk/build-agents.sh` sera ajouté en semaine 5 (B10) pour
encapsuler la compilation de tous les exemples avec les options reproductibles
(`RUSTFLAGS="-C opt-level=s"` pour la taille).

---

## Alternatives considérées

### `wasm32-wasip1` avec stubs WASI minimalistes

Aurait permis d'utiliser `eprintln!` pour le débogage. Rejetée en Phase 2 car :
- Les stubs WASI (au moins `proc_exit`, `fd_write`, `fd_read`) nécessitent une
  implémentation dans le Linker qui n'apporte aucune valeur fonctionnelle.
- L'ajout de stubs noop pour `fd_write` masquerait silencieusement les
  `println!` sans output visible → confus.
- `wasm32-unknown-unknown` + `emit` host function offre le même débogage de
  façon explicite et auditable dans le log causal.

### Trait objet `dyn Agent` côté hôte

Aurait permis de passer des agents Rust natifs (pas de WASM) au scheduler.
Rejetée : la valeur de B4 est précisément de valider le pipeline Rust→WASM.
Les agents WAT inline existants couvrent déjà les tests unitaires. Le SDK WASM
teste le bout-en-bout.

---

## Conséquences

- Nouveau membre de workspace : `poc/agent-sdk`.
- Nouveau target installé : `wasm32-unknown-unknown` (`rustup target add`).
- Les exemples ne supportent pas `cargo run --example echo` (pas de `main`
  réel sur WASM). Seul `cargo build` suivi du chargement par le runtime est
  le chemin d'exécution.
- `cargo check -p agent-sdk` (native) doit passer sans erreur (les wrappers
  sont des no-ops sur non-wasm32).
- La crate `agent-sdk` n'a pas de tests unitaires propres — les tests
  d'intégration dans `poc/runtime` (B1 + scénarios S1–S4) valident l'ensemble.

---

## Références

- `docs/archive/poc_E2E.md` v3 — §3.2 ADR-0020, Annexe A structure cible
- ADR-0019 — `agent_infer` (signatures à ajouter en semaine 2)
- `poc/runtime/src/actor.rs` — WAT modules existants (référence ABI)
- Wasmtime Linker : https://docs.rs/wasmtime/latest/wasmtime/struct.Linker.html
