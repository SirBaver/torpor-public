# Sources des figures — blog-04

| Figure | Type | Source | Statut |
|---|---|---|---|
| `capture-swarm-acte1-admission.png` | capture TUI | `demo-tui --scene swarm`, Acte 1 — admission bornée (C2) : `in-flight ≤ cap`, `burst 14/14 terminés`, « au plus cap en vol, le surplus attend » ; substrat PoC Linux | capture réelle au tag — **mécanisme, pas une mesure** |
| `capture-swarm-acte2-densite.png` | capture TUI | même session, Acte 2 — densité : `actifs 9 / dormants 1`, éviction `[e]` → dormant, réveil `[w]` depuis snapshot (S11/S12) | capture réelle au tag — **mécanisme, pas une mesure** |

| `figure-overhead-scaling.svg` | graphe de mesure | **FIGURE 1** de l'article — overhead mémoire/agent vs N (échelle log). Points T6-scaling (K=3) : N=100/300/1000/3000 → 9,1/9,5/9,6/9,6 KB ; fit `overhead(N)=9,65−54/N`, R²=0,988 ; asymptote 9,65 KB ; prédit N=10 000 → 9,64 KB. Source `results/T6/SYNTHESE.md`, substrat PoC Linux (R-blog-1), régime R2 | généré par `plot_overhead.py` (Python pur, sans dépendance) — régénérable |

⚠ Garde-fou anti-survente : la scène `swarm` illustre le **mécanisme d'ordonnancement**, PAS une mesure de densité. Le bandeau de la TUI le dit (« N à l'écran ≠ N soutenables ; densité hébergée vs active distinctes, NON mesurées ici »). Le chiffre de densité (×4 539–7 375 hébergée, ~70 actifs) vient des benchmarks T6/T7/T8 (`results/.../verdict.json`), régime R2, non transférable seL4 (ADR-0065).

Règle : toute figure a une source traçable. Un graphe de mesure (FIGURE 1, courbe overhead/agent) viendra de `results/T6/SYNTHESE.md`, substrat nommé (R-blog-1).
