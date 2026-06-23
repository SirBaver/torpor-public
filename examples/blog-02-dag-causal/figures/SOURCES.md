# Sources des figures — blog-02

| Figure | Type | Source | Statut |
|---|---|---|---|
| DAG falsifié → orphelin | diagramme conceptuel | bloc ```mermaid``` dans `docs/blog/article-02-dag-causal-hash.md` | schéma conceptuel (pas une mesure) |
| `capture-effects-tamper.png` | capture TUI | `demo-tui --scene effects`, temps fort `[t]` (id stocké ≠ id recalculé, juge orphelin) ; substrat PoC Linux (R-blog-1) | capture réelle au tag |

Règle : toute figure a une source traçable. Une figure conceptuelle = Mermaid versionné dans l'article. Un graphe de mesure (aucun ici) viendrait d'un `verdict.json` cité, avec substrat nommé (R-blog-1). Si la plateforme finale ne rend pas Mermaid → pré-rendu SVG déposé ici, même source.
