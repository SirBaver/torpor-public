<!-- Corps de la GitHub Release `blog-05-capabilities` (releases jumelles FR/EN). ⟨à épingler⟩ au tag. -->

# blog-05 — On a essayé de tromper notre propre garde-frontière

**Régime :** R1 (effets) · **Statut :** prouvé (P4) · **Substrat :** Linux / **RocksDB** (R-blog-1)

## Claim → Preuve

| Claim de l'article | Élément | Preuve (permalink `@blog-05-capabilities`) | Substrat |
|---|---|---|---|
| Refus capability à la frontière | `CapabilityDenied 0x14`, accès nul | ⟨decisions/0029-sef3-scope-covers-cap-denied.md⟩ | Linux |
| Confused-deputy : isolation tenue | propriété (périmètre testé) | ⟨results/.../SEF-3/verdict.json⟩ | Linux |
| Trou d'audit trouvé → corrigé → revalidé | finding #6, rate-limit par ressource | ⟨decisions/0051-cloture-campagne-tri-findings.md⟩ | Linux |

## Reproduire
```bash
git clone <url> && cd os-public/poc && git checkout blog-05-capabilities
CXXFLAGS="-include cstdint" cargo run -p os-poc-runtime --features demo-tui --bin demo-tui -- --scene effects
# [x] intrus : cap reports/ tente confidential/ → refus 0x14, accès nul
```

## Limites (piège #8 / #6)
- Une red team interne **n'est pas une preuve d'imprenabilité** — réfutation tentée et documentée (trou d'audit compris).
- *Isolation vérifiée à la frontière, non contournée par l'agent, dans le périmètre testé* (P4). Pas plus.
- Confused-deputy rejoué **sur Linux seulement** ; isolation inter-process seL4 démontrée par ailleurs (ADR-0049 §D1), pas ce scénario.
