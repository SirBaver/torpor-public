# Expériences lab — cadre de reproductibilité

## Convention de nommage

`E<NN>-<slug>.md` — chaque fichier est une expérience autonome.

## Structure d'un fichier d'expérience

```
Identifiant   E<NN>
Hypothèse     lien vers spec/04-hypotheses.md
Date          YYYY-MM-DD
Statut        Planifiée | En cours | Complète | Abandonnée
```

Chaque expérience contient :
1. **Objectif** — ce qu'on cherche à apprendre, et pourquoi c'est utile pour le design de l'OS
2. **Configuration exacte** — modèle, system prompt, variables d'environnement
3. **Procédure** — commandes exactes pour reproduire, dans l'ordre
4. **Critères** — ce qui constitue un PASS, un FAIL, une observation neutre
5. **Résultats** — sortie brute + interprétation
6. **Verdict** — ce que ça change (ou non) dans la spec ou le design

## Variables contrôlables

| Variable | Défaut | Override |
|----------|--------|---------|
| Modèle | `qwen2.5:3b` | `OLLAMA_MODEL=xxx docker compose up -d daemon` |
| System prompt | `config/system_prompt.txt` | `SYSTEM_PROMPT_FILE=/app/config/xxx.txt docker compose up -d daemon` |
| Format de sortie | `""` (prose libre) | `OUTPUT_FORMAT=json docker compose up -d daemon` |
| Reset DB | non | `--fresh` dans smoke_test.sh |

## Expériences

| ID | Titre | Statut |
|----|-------|--------|
| [E01](E01-ablation-prompt-namespace.md) | Ablation règles namespace du system prompt | Complète — 2026-05-13 |
| [E02](E02-model-7b-qwen.md) | Modèle 7B — qwen2.5:7b smoke test + session longue | Complète — 2026-05-13 |
| [E03](E03-machine-output.md) | Sortie machine-first vs prose — impact inférence | Complète — 2026-05-14 |
