# Bundle compagnon — blog-08 « Des effets maîtrisables »

Carte du bundle. L'article 8 est la synthèse : la scène `lineage` rejoue, sur des
payloads data engineering, **le même moteur** que la scène `effects` (traçabilité,
rollback, capabilities). Elle ne prouve aucune capacité nouvelle.

| Dossier / fichier | Statut | Quoi |
|---|---|---|
| `REPRODUCE.md` | **(A) preuve** | Reproduction autoritaire sur le **vrai système**, cloné au tag. Scène `lineage` + auto-test headless. |
| `figures/` | figure | Captures TUI de la scène `lineage` (falsification détectée + preuve P3) + `SOURCES.md`. |
| `expected/` | **(A) preuve** | Transcript headless `--selftest-lineage` (12 assertions PASS), capturé au tag. |

Règle : le code exécutable canonique vit dans `poc/` ; l'article le **cite** (`@tag`), ne le duplique pas.

Deux cas à ne pas confondre (cf. article §21 vs §27) :
- **Falsification** (`[t]`) → la rupture d'intégrité est **détectée** (P3 « Violation détectée », `id stocké ≠ id recalculé`).
- **Modèle faillible** (`--llm-wrong`) → le profil est faux mais **le lineage reste intègre** : un historique fidèle d'une décision fausse. L'intégrité n'est pas la justesse (frontière LLM).
