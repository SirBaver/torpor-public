# FINDING-B-2 — Frontiere hote : bounds check inconsistant (agent_check_cap, agent_add_cause)

**Classe :** Attaque sur la frontiere hote WASM — validation des pointeurs entrants
**Reference :** poc/runtime/src/actor.rs (lignes pre-patch ~1349, ~1596)
**Rejoue :** oui — analyse statique + correctif applique
**Regime :** R1
**Substrat :** Linux

---

## Observation

Deux host functions utilisaient le pattern `ptr + len > data_len` au lieu de
`ptr.checked_add(len).map_or(true, |end| end > data_len)` :

- `agent_add_cause` (ptr + ACTION_ID_LEN, constante = 32)
- `agent_check_cap` (ptr + len, len = resource_len controle par l'agent)

Les autres host functions (`agent_introspect`, `agent_session_info`, `agent_infer`,
`agent_store_get`, `agent_store_put`) utilisaient deja `checked_add`.

## Exploitabilite

**Non exploitable sur la cible (Linux x86-64 / AArch64, 64 bits) :**

Les parametres sont des i32 WASM. Cast `as usize` sur 64 bits :
- max positif : 2 147 483 647 ; max negatif (cast) : 4 294 967 295
- Somme max ptr + len = ~8.6 GB ; usize 64 bits ne deborde pas
- data_len <= 4 GB (limite WASM) ; 8.6 GB > 4 GB => check retourne -1

Le debordement serait possible sur une plateforme 32 bits (usize = u32),
non concernee par ce deploiement.

## Correctif applique

```rust
// Avant
if ptr + len > data_len { return -1; }

// Apres
if ptr.checked_add(len).map_or(true, |end| end > data_len) { return -1; }
```

Commit : session 2026-06-03. Tests S16, S17, S30 repasses PASS apres patch.

## Type de limite

**Correctible** — correctif applique. La frontiere hote est desormais uniforme
sur tous les host functions qui lisent des pointeurs entrants depuis le WASM guest.

## Note

La defense en profondeur est assuree par Wasmtime : `Memory::read` et `Memory::write`
effectuent leur propre bounds-check contre la memoire lineaire. Le correctif elimine
l'incoherence de style et garantit le comportement correct meme sur 32 bits.
