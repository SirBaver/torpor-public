# S21 — Délégation cap scope-prefix (UC-2 / ADR-0005)

**Régime :** R1 (P4 — isolation par capabilities, propriété d'effet)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P4** : atténuation par scope-prefix. Agent A délègue à B une cap avec un scope
plus restreint ("/data/sub") dérivée d'une cap "/data" rw+delegate. L'oracle
vérifie les deux sens : accès autorisés couverts par le scope, accès refusés
hors-scope ou hors-permission.

### Invariants (ADR-0005)

- B PEUT accéder à "/data/sub" (exact) et "/data/sub/*" (sous-chemins).
- B NE PEUT PAS accéder à "/data" (trop large) ni "/data/other" (scope différent).
- B NE PEUT PAS écrire (cap read-only déléguée).
- B NE PEUT PAS déléguer (delegate=false dans la cap déléguée).
- C_A (source) reste valide après délégation.

---

## Protocol

Test direct sur `CapabilityStore` sans agent WASM (impact = aucun).

```
grant_root(A, rw+delegate, "/data")    → C_A
delegate(C_A, A→B, read, "/data/sub") → C_B

check(B, C_B, "/data/sub")        == true   (exact)
check(B, C_B, "/data/sub/x")      == true   (scope_covers)
check(B, C_B, "/data")            == false  (hors-scope)
check(B, C_B, "/data/other")      == false  (scope différent)
check(B, C_B, "/data/sub", write) == false  (permission dépassée)
check(B, C_B, "/data/sub", delg)  == false  (delegate=false)
check(A, C_A, "/data", rw+d)      == true   (source inchangée)
```

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s21_cap_delegation_scope_prefix --exact --nocapture
```

---

## Références

- **ADR-0005** — Design capabilities, atténuation par scope-prefix, `check()` sans cache.
- **ADR-0007** — Invalidation sur rollback (pendant : S17).
- **SEF-9** — confused-deputy rate-limit × audit (pendant adversarial).
- `poc/capabilities/src/lib.rs::delegate` — implémentation.
- `poc/runtime/src/lib.rs::tests::s21_cap_delegation_scope_prefix` — test.
