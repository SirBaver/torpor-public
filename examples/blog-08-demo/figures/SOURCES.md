# Sources des figures — blog-08

| Figure | Type | Source | Statut |
|---|---|---|---|
| `capture-lineage-falsification-arbre.png` | capture TUI | `demo-tui --scene lineage`, vue MODÉLISATION `[m]` après falsification `[t]` : arbre (source → profil → artefact PUBLISHED) + bandeau « hash stocké ≠ recalculé, l'étape aval pointe dans le vide → détecté » ; substrat PoC Linux (R-blog-1) | capture réelle au tag |
| `capture-lineage-falsification-preuve.png` | capture TUI | même session, `[d]` : panneau PREUVE — tamper-evidence (P3), `action_id stocké ≠ recalculé`, `P3 traçabilité = VIOLATION DÉTECTÉE`, réf `poc/causal-log/src/lib.rs:188`, provenance structurelle bornée à CE runtime | capture réelle au tag |

Note (§21 vs §27) : `[t]` (falsification) déclenche la détection P3 — la rupture se voit dans PROPRIÉTÉS et le panneau PREUVE, **pas** dans l'arbre. À ne pas confondre avec `--llm-wrong` (§27), où le modèle se trompe mais le lineage reste intègre.

Règle : toute figure a une source traçable. Une capture TUI nomme sa scène, son temps fort et son substrat (R-blog-1). La scène `lineage` = même moteur que `effects`, payloads data — pas une capacité nouvelle.
