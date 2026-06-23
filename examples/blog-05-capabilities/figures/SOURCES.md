# Sources des figures — blog-05

| Figure | Type | Source | Statut |
|---|---|---|---|
| `capture-effects-intrus-dag.png` | capture TUI | `demo-tui --scene effects`, temps fort `[x]` : intrus (`data_accessor`, cap `reports/`) tente `confidential/salaires_2024` → bandeau `DENIED ... CapabilityDenied 0x14`, refus tracé dans le DAG ; substrat PoC Linux (R-blog-1) | capture réelle au tag |
| `capture-effects-intrus-preuve.png` | capture TUI | même session, `[d]` après refus : panneau PREUVE — `refus de capability (P4)`, entrée `CapabilityDenied`, agent `rogue-agent00000`, payload `confidential/salaires_2024`, « refus émis PAR LE RUNTIME, pas par l'agent » | capture réelle au tag |

Règle : toute figure a une source traçable. Une capture TUI nomme sa scène, son temps fort et son substrat (R-blog-1). Le refus agit sur les capabilities à la frontière (P4), pas sur l'inférence.
