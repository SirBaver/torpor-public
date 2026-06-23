# Sources des figures — blog-03

| Figure | Type | Source | Statut |
|---|---|---|---|
| `capture-effects-rollback-dag.png` | capture TUI | `demo-tui --scene effects`, temps fort `[r]` (état de l'agent mémoire reculé à seq 1, P2 ACTIVE, DAG complet) ; substrat PoC Linux (R-blog-1) | capture réelle au tag |
| `capture-effects-rollback-preuve.png` | capture TUI | même session, `[d]` après rollback : panneau PREUVE — `emit_type 0x0b (SchedulerRollback)`, seq cible 1, « état restauré ; aucune entrée intermédiaire observable » (P2) | capture réelle au tag |

Règle : toute figure a une source traçable. Une capture TUI nomme sa scène, son temps fort et son substrat (R-blog-1). Un graphe de mesure viendrait d'un `verdict.json` cité.
