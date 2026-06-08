# S29 — Révocation récursive profonde (UC-16 / ADR-0005 / P4)

**Régime :** R1 (P4 — isolation cap, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P4 (révocation récursive, mode eager)** : `revoke(cap_root)` supprime
immédiatement toute la chaîne de délégation (BFS itératif, O(depth)). Après
révocation, tous les descendants perdent l'accès — pas de cache, pas de check
différé, pas de tombstone. Conformité ADR-0005 §amendment (2026-05-15).

### Note sur l'implémentation (eager vs lazy)

Le `poc/README.md` décrit les caps comme « révocation lazy en chaîne » — cette
description est stale. L'amendement ADR-0005 (2026-05-15) clarifie : le PoC
Rust implémente le **mode eager** (BFS dans `CapabilityStore::revoke()`). Le mode
lazy est la référence conceptuelle pour Phase 4+ (caps inter-nœuds, TTL). Ce
test valide le mode eager sur une chaîne de k=4 niveaux.

### Invariants

1. `revoke(cap_a)` restitue `count = k + 1` (la racine + k descendants).
2. Après révocation, `check(agent_i, cap_i, resource, perm) = false` pour
   tout nœud i ∈ [0, k].
3. Suppression immédiate : un `check()` juste après `revoke()` retourne `false`.

---

## Acteurs et topologie

```
A (root) → B → C → D → E    (profondeur k=4, branching=1)
```

Chaque agent délègue sa cap au suivant avec `full_perm = {read, write, execute, delegate}`.
Seul l'agent A peut déléguer (il est le propriétaire de la cap racine).

---

## Protocole

```
caps.grant_root(A, full_perm, "/data")         → cap_a
caps.delegate(cap_a, A, B, full_perm, "/data") → cap_b
caps.delegate(cap_b, B, C, full_perm, "/data") → cap_c
caps.delegate(cap_c, C, D, full_perm, "/data") → cap_d
caps.delegate(cap_d, D, E, full_perm, "/data") → cap_e

PRECHEK : caps.check(A/B/C/D/E, cap_a/b/c/d/e, "/data", read) = true ×5

caps.revoke(cap_a)  →  count = 5

ORACLE P4 : caps.check(A/B/C/D/E, cap_a/b/c/d/e, "/data", read) = false ×5
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Arbre avec branching > 1 | `populate_tree` + benchmarks benchmark_revoke |
| Révocation partielle (sous-arbre) | ADR-0007 D8 (revoke_owned_after) |
| Coût O(depth) mesuré (ns/µs) | Lab de performance dédié si nécessaire |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s29_revoke_recursive_deep --exact --nocapture
```

---

## Références

- **ADR-0005 §amendment (2026-05-15)** — mode eager (BFS) vs lazy (Phase 4+).
- **ADR-0007** — Rollback + invalidation cap en cascade.
- `poc/capabilities/src/lib.rs::revoke()` — implémentation BFS.
- `poc/runtime/src/lib.rs::tests::s29_revoke_recursive_deep` — test.
