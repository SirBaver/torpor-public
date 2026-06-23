# Bundle compagnon — blog-06 « Couper le courant à 4 moments précis, 40 fois »

| Fichier | Statut | Quoi |
|---|---|---|
| `REPRODUCE.md` | **(A) preuve** | Build + boot seL4/QEMU + test W^X + harnais crash. **Exige la toolchain seL4.** |
| `expected/` | **(A) preuve** | Transcripts réels du noyau (déjà capturés : `docs/demo/sel4-transcripts/`). |
| figure | — | Source = bloc ```mermaid``` dans `docs/blog/article-06-crash-atomicity-sel4.md` (4 kill points, schéma conceptuel). |

## Pourquoi pas d'`illustration/` autonome ici

Contrairement aux autres articles, ce n'est **pas** « extraire et lancer en 30 s » : il faut la toolchain seL4 + QEMU AArch64, non réductible. Le seul artefact honnête accessible à la plupart des lecteurs est le **transcript réel** du noyau (`docs/demo/sel4-transcripts/`) + la figure. On l'assume franchement — cohérent avec le ton de l'article (« le dire est une exigence de rigueur, pas un aveu »).

**Moteur seL4 = redb**, jamais RocksDB (R-blog-2 / O6). Aucune perf seL4 revendiquée (R-blog-4).
