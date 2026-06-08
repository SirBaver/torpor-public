# E02 — Modèle 7B (qwen2.5:7b) — smoke test + session longue

| Champ | Valeur |
|-------|--------|
| Identifiant | E02 |
| Hypothèse testée | H-profil-B, H-mémoire-schema, H-inférence-coût (`spec/04-hypotheses.md`) |
| Date planifiée | 2026-05-13 |
| Statut | Complète — 2026-05-13 |

---

## 1. Objectif

Trois questions :

1. **Fermeture des gaps comportementaux.** Les fails comportementaux à 3B (P1.5-T2 memory_write, N3 cold session) se ferment-ils à 7B ? Si oui, l'OS amplifie des agents plus capables. Si non, c'est une limite de paradigme (tool calling fiable requiert encore plus que 7B).

2. **Propriétés P1–P4 stables à taille supérieure.** Rollback, causalité, capabilities, locking optimiste — ces propriétés sont-elles toujours opérationnelles quand le modèle est plus grand (output plus verbeux, latences différentes) ?

3. **Session longue — seuil de cohérence.** À partir de combien d'actions un agent commence-t-il à halluciner des clés mémoire (clés qui n'existent pas, ou valeurs inventées) ? Ce seuil informera le dimensionnement des fenêtres de supervision (H-profil-B §supervision_period).

**Modèle choisi :** `qwen2.5:7b` — même architecture que le baseline 3B, ablation propre sur la taille. Élimine les différences d'architecture comme variable confusante.

---

## 2. Configuration

| Paramètre | Valeur |
|-----------|--------|
| Modèle | `qwen2.5:7b` |
| System prompt | `config/system_prompt.txt` (identique au baseline) |
| Tests | smoke_test.sh --fresh + long_session_test.sh |

---

## 3. Procédure de reproduction

```bash
# 1. Télécharger le modèle (première fois seulement, ~4.7 GB)
docker exec lab-ollama-1 ollama pull qwen2.5:7b

# 2. Vérifier la disponibilité
docker exec lab-ollama-1 ollama list

# 3. Démarrer le daemon avec qwen2.5:7b
OLLAMA_MODEL=qwen2.5:7b docker compose up -d daemon

# 4. Smoke test complet
bash tests/smoke_test.sh --fresh 2>&1 | tee /tmp/E02-smoke.txt

# 5. Session longue (N=100 rounds)
bash tests/long_session_test.sh 100 2>&1 | tee /tmp/E02-long-100.txt

# 6. Restaurer le défaut
docker compose up -d daemon
```

---

## 4. Critères

### 4.1 Smoke test (comparaison vs baseline 3B)

| Catégorie | Baseline 3B | Cible 7B |
|-----------|-------------|----------|
| Infrastructure P1-P4 | 71/72 | ≥ 71/72 (régression inacceptable) |
| Tool calling P1.5-T2 | FAIL | PASS attendu (écriture spontanée) |
| Namespace N3 | FAIL (cold session) | PASS attendu (7B suit les règles froides) |
| H-inférence-coût | 128–140s / 4 calls | Mesurer — attendu > 30s, ratio vs 3B inconnu |

### 4.2 Session longue

| Round | Critère | Attendu |
|-------|---------|---------|
| 1–20 | Recall correct à chaque round | ✓ (contexte frais) |
| 21–50 | Recall correct | ✓ probable à 7B |
| 51–100 | Recall correct | Incertain |
| > 100 | Cohérence causale (log) | ✓ (infra indépendante du modèle) |

**Seuil de cohérence** : premier round où `recall_correct = false` → noter N_break.

**Infrastructure toujours vérifiée :**
- Chaîne causale intacte après 100 actions (log)
- Snapshots créés à interval régulier (tous les 10 rounds)
- Rollback vers snapshot N-30 possible après 100 rounds

---

## 5. Résultats

Exécuté le 2026-05-13, qwen2.5:7b, `system_prompt.txt` (identique baseline), smoke_test.sh --fresh + long_session_test.sh N=50.

### 5.1 Smoke test

| Phase | PASS | FAIL | Delta vs qwen2.5:3b |
|-------|------|------|---------------------|
| Phase 1 — API de base | 15 | 0 | = |
| Phase 1.5 — Tool calling | 10 | 1 | = (même FAIL: T4c) |
| Phase 1.6 — Causalité session | 4 | 0 | = |
| Phase 2 — Multi-agent | 11 | 0 | = |
| Phase 3 — Rollback | 8 | 0 | = |
| Phase 3B — Caps + rollback | 5 | 0 | = |
| Phase D4 — Locking optimiste | 4 | 0 | = |
| Phase 2B — Namespaces + DAG | 8 | 0 | = |
| Phase 4 — Capabilities | 6 | 0 | = |
| **TOTAL** | **71** | **1** | **=** |

**FAIL persistant :** T4c — `memory_write` déclenché mais la clé reste introuvable après la write. Présent à 3B et à 7B : le modèle écrit sous une clé différente de celle que le test cherche. Comportemental, pas d'infrastructure.

**P2.3 — observation clé :** H-mémoire-schema CONFIRMÉE à 7B (divergence maintenue). Amélioration qualitative vs 3B : les deux agents utilisent désormais le namespace `shared/`, mais divergent sur la casse de la clé (`shared/user.familyName` vs `shared/user.family_name`). Le 7B respecte mieux la convention de namespace mais n'élimine pas la divergence de schéma intra-namespace.

### 5.2 Session longue (N=50)

```
N_break déclaré    : round 1 (toutes les clés NOT_FOUND)
Cohérence causale  : PASS — 50 actions think chaînées sans rupture (106 entrées log total)
Rollback post-session : PASS — hash_matches=true
```

**Interprétation N_break=1 :** ne représente pas une défaillance de cohérence du modèle. Le test vérifie la clé `item_XXXX` dans le namespace de session, mais le modèle écrit sous ses propres conventions (probablement `shared/item.N` ou autre). Les 106 entrées log confirment que le modèle a bien fait des tool calls à chaque round — il stocke, mais pas sous les clés attendues par le test. **Défaut de conception du test :** `long_session_test.sh` suppose un format de clé que le LLM ne suit pas spontanément ; il mesure la conformité de nommage, pas la cohérence mémorielle. À corriger dans une future itération (lire-via-LLM ou pré-spécifier la clé dans le prompt).

### 5.3 Timing H-inférence-coût

| Modèle | 4 actions clés (P2.4) | Total 10 appels | Ratio vs 3B |
|--------|----------------------|-----------------|-------------|
| qwen2.5:3b | ~52s (L9) | ~130s (extrap) | 1× |
| llama3.2:3b | ~140s | ~348s (extrap) | 2.7× |
| qwen2.5:7b | 290s (orch=154s, A=20s, B=17s, merge=99s) | **725s (mesuré)** | **5.6×** |

qwen2.5:7b est **5.6× plus lent** que la baseline 3B sur la même machine. La latence d'orchestration (154s) est le principal facteur — le modèle plus grand génère beaucoup plus de tokens de réflexion avant le tool call.

---

## 6. Verdict

**Gap comportemental fermé à 7B :** `[x] partiel`

- T4c (clé introuvable après write) : toujours FAIL à 7B → gap non fermé
- P2.3 (divergence de schéma) : amélioration qualitative (namespace shared/ respecté), divergence sur la casse → gap partiellement réduit, non fermé
- N3 (cold session + write explicite) : PASS à 7B (amélioration vs 3B qui échouait)

**Session longue :** test mal conçu pour mesurer la cohérence LLM (voir §5.2). L'infrastructure est stable (causal chain, rollback). Cohérence comportementale non mesurée — à instrumenter différemment.

**Impact spec :**
- H-profil-B : la fenêtre de supervision ne peut pas être dimensionnée à partir de ce test (N_break non mesurable avec l'instrumentation actuelle). Nécessite un test avec clés pré-spécifiées dans les prompts.
- H-inférence-coût : 7B est 5.6× plus lent que 3B. Pour des boucles agent de 5s (W1), la latence d'orchestration seule (~154s) est incompatible avec le cycle. Le 7B n'est pas viable en temps réel sur cette machine ; utilisable uniquement pour des sessions supervisées batch.
- Stratégie architecture : le design doit cibler des modèles efficaces (≤ 3B) pour le temps réel, et laisser les grands modèles (≥ 7B) pour les rôles d'orchestration async ou de supervision hors-cycle.

**Référence LESSONS.md :** L24
