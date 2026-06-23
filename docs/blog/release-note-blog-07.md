<!-- Corps de la GitHub Release `blog-07-cinq-paris` (releases jumelles FR/EN). Article méta : la preuve = le corpus d'ADR. Pas de commande de reproduction propre. -->

# blog-07 — Pourquoi pas Docker, SQLite, ou Unix ?

**Régime :** mixte (déclaré pari par pari) · **Statut :** synthèse · **Renvoie aux** preuves des articles 2, 4, 5.

## Claim → Preuve (par pari)

| Pari | Régime | Preuve (permalink `@blog-07-cinq-paris`) |
|---|---|---|
| #1 DAG ≠ arbre | R1 | ⟨decisions/0003-modele-causal-dag.md⟩ → blog-02 |
| #2 LSM RocksDB ≠ B-tree SQLite | R2 / **Linux** | ⟨decisions/0002-choix-substrat.md⟩, ⟨0011⟩ |
| #3 Wasmtime+Tokio ≠ Docker | R2 / **Linux** | loi overhead R²=0,988 → blog-04 |
| #4 capabilities ≠ identité Unix | R1 | ⟨decisions/0005-design-capabilities-revoke.md⟩ → blog-05 |
| #5 supervision asymétrique | principe | ⟨decisions/0006-modele-supervision.md⟩ → blog-01 |

## Limites (axe interdit #2)
- Contraste **factuel par pari**, jamais « X× plus rapide que Docker/SQLite/Unix ». Pas de duel de benchmarks.
- Un pari gagné **dans nos conditions** n'est pas une loi universelle. Bornes = PoC Linux/**RocksDB** (cible seL4 = redb), non transférées (ADR-0065).
- redb (B-tree) sur seL4 vs RocksDB (LSM) sur Linux = **deux stacks pour deux substrats**, pas une contradiction.
