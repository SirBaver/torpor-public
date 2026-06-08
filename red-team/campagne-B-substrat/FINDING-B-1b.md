# FINDING-B-1b — Dependance morte `wasmtime-wasi` : retiree

**Classe :** Reduction de surface d'attaque (dependance declaree non utilisee)
**Methode :** audit statique d'imports + `cargo audit` avant/apres, 2026-06-03
**Rejoue :** N/A — action de durcissement, pas un vecteur d'attaque
**Type :** Correctible — **correctif applique**

---

## Observation

`wasmtime-wasi = "25"` etait declaree comme dependance du workspace (`poc/Cargo.toml`)
et du runtime (`poc/runtime/Cargo.toml:17`), mais **aucun fichier `.rs` du repo ne
l'importe** (`grep -rn wasmtime_wasi --include='*.rs'` → 0 occurrence hors `target/`).

Le runtime n'expose pas WASI aux guests : il construit un `Linker<AgentState>` avec
des host functions custom dans le namespace `env` uniquement. WASI n'a jamais ete cable.

## Impact

La dependance morte tirait dans l'arbre l'advisory **RUSTSEC-2026-0149**
(`path_open(TRUNCATE)` contourne `FilePerms::WRITE`, CVSS 7.5 high), attribue au
crate `wasmtime-wasi`. Surface inutile pour zero benefice fonctionnel.

## Correctif

Retrait des deux lignes `wasmtime-wasi` (`poc/Cargo.toml`, `poc/runtime/Cargo.toml`).

Verification :
- `cargo check -p os-poc-runtime` → **OK** (aucune erreur ; seuls warnings preexistants
  sans rapport dans `rollback_runner.rs`).
- `cargo audit` : **16 → 15** advisories. RUSTSEC-2026-0149 disparait.

## Reste a la charge du crate `wasmtime` core (non joignables)

RUSTSEC-2025-0046 (`fd_renumber`), -0020 (ressources WASI), -0088 (pooling allocator)
restent listes car attribues au crate `wasmtime` core (conserve), mais ne sont pas
joignables : aucun import WASI dans le `Linker`, allocateur on-demand par defaut.
Detail dans FINDING-B-1.
