# Bundle compagnon — blog-03 « Annuler 500 décisions en 17 ms »

| Fichier | Statut | Quoi |
|---|---|---|
| `REPRODUCE.md` | **(A) preuve** | Rollback sur le vrai système (P2), substrat PoC Linux. |
| `expected/` | **(A) preuve** | Transcript SEF-2 à capturer au tag (provenance : commit/tag/host/date). |
| figure | — | Source = bloc ```mermaid``` dans `docs/blog/article-03-rollback-transactionnel.md` (schéma conceptuel). |

Pas d'`illustration/` autonome ici : un rollback in-memory de quelques lignes n'aurait **pas** la propriété « tout-ou-rien transactionnel sur état persistant » qui fait la preuve (R-blog-5). La preuve est dans `REPRODUCE.md`.
