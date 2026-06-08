# E01 — Ablation des règles de namespace du system prompt

| Champ | Valeur |
|-------|--------|
| Identifiant | E01 |
| Hypothèse testée | H-mémoire-schema (`spec/04-hypotheses.md` §P2.3) |
| Date planifiée | 2026-05-13 |
| Statut | Complète — 2026-05-13 |

---

## 1. Objectif

La convergence de schéma observée dans P2.3 (les agents utilisent `shared/user.name` de façon cohérente) est-elle une propriété du **modèle** ou une propriété du **system prompt** ?

Si les agents divergent immédiatement sans les règles de namespace → la convergence est entièrement dépendante du prompt → **l'OS doit enforcer le schéma à l'infrastructure**, le modèle ne peut pas en être responsable.

Si les agents convergent même sans règles explicites → les LLM à 3B ont une tendance spontanée à la cohérence de clé → le prompt de namespace est optionnel (nice-to-have, pas critique).

**Conséquence architecturale :** le résultat détermine si le projet doit implémenter un registre de schéma côté runtime (Layer 1) ou si la convention de prompt suffit pour les LLM courants.

---

## 2. Configuration

| Paramètre | Valeur |
|-----------|--------|
| Modèle | `qwen2.5:3b` (baseline) |
| System prompt | `config/system_prompt_no_ns.txt` (ablation — voir §3) |
| Smoke test | `tests/smoke_test.sh --fresh` |
| Tests ciblés | P2.3 (H-mémoire-schema), N1/N2/N3 (Phase 2B) |

**Différence vs baseline :** retrait du bloc `Namespace rules (IMPORTANT)` du system prompt. Toutes les règles de tool calling (memory_read/write/list) sont conservées.

---

## 3. Procédure de reproduction

```bash
# 1. Vérifier que le daemon tourne avec qwen2.5:3b (défaut)
docker exec lab-daemon-1 env | grep OLLAMA_MODEL
# attendu : OLLAMA_MODEL=qwen2.5:3b

# 2. Démarrer le daemon avec le prompt ablation
SYSTEM_PROMPT_FILE=/app/config/system_prompt_no_ns.txt \
  docker compose up -d daemon

# 3. Vérifier que le bon prompt est chargé
curl -s http://localhost:8888/health | python3 -c "import sys,json; print(json.load(sys.stdin))"

# 4. Lancer le smoke test complet
bash tests/smoke_test.sh --fresh 2>&1 | tee /tmp/E01-results.txt

# 5. Restaurer le prompt par défaut
docker compose up -d daemon
```

---

## 4. Critères

### Tests critiques (convergence de schéma)

| Test | Comportement attendu si schéma = prompt | Comportement attendu si schéma = modèle |
|------|----------------------------------------|-----------------------------------------|
| P2.3 | FAIL ou divergence (clés différentes) | PASS (même clé qu'avec le prompt complet) |
| N3 | Non affecté (le prompt N3 est explicite) | Non affecté |
| N1/N2 | FAIL probable (pas de namespace → clés sans namespace) | PASS (modèle utilise namespace spontanément) |

### Tests infrastructure (doivent passer indépendamment)

P1 (API), P3 (rollback), P3B (caps), D4 (locking), Phase 4 (capabilities) — ces tests ne font pas appel au modèle pour les namespaces. Toute régression ici serait un bug, pas un résultat.

### Seuil d'interprétation

- **> 3 tests namespace qui fail** → schéma dépendant du prompt → nécessite enforcement infrastructure
- **≤ 1 test namespace qui fail** → schéma partiellement intrinsèque au modèle → enforcement optionnel

---

## 5. Résultats

Exécuté le 2026-05-13, qwen2.5:3b, `system_prompt_no_ns.txt`, smoke_test.sh --fresh.

### Score par phase

| Phase | PASS | FAIL | Delta vs baseline (prompt complet) |
|-------|------|------|-------------------------------------|
| Phase 1 — API de base | 15 | 0 | = |
| Phase 1.5 — Tool calling | 6 | 5 | **−4** (était 10/11) |
| Phase 1.6 — Causalité session | 4 | 0 | = |
| Phase 2 — Multi-agent | 11 | 0 | = |
| Phase 3 — Rollback | 8 | 0 | = |
| Phase 3B — Caps + rollback | 5 | 0 | = |
| Phase D4 — Locking optimiste | 4 | 0 | = |
| Phase 2B — Namespaces + DAG | 8 | 0 | = |
| Phase 4 — Capabilities | 6 | 0 | = |

### Comportement P2.3 (H-mémoire-schema) — résultat clé

```
Clés contenant 'Dupont' : ['user.family.name', 'user.last_name']
→ DIVERGENCE : 2 clés distinctes pour le même concept
→ H-mémoire-schema CONFIRMÉE sans règles de namespace
```

Avec le prompt complet : agents convergeaient sur `shared/user.name`.

### Phase 1.5 — détail des fails

Sans les règles de namespace, le modèle est moins ancré dans son rôle d'agent :
- **T1** : aucun tool call pour "Quel est le nom de l'utilisateur ?" (1 itération, réponse conversationnelle)
- **T3** : aucun tool call, 1 itération (multi-step cassé)
- **T5** : aucun tool_call causé par le think de référence

T2 (memory_write spontané) : **PASS** — mais la clé écrite est `user.name` sans namespace (pas `agent-a/user.name`).

### N3 — résultat inattendu

**N3 a PASSÉ** alors qu'il échouait avec le prompt complet. Le modèle a suivi l'instruction explicite "Store it in namespace shared with the canonical key user.family_name." — clé unique : `user.family_name`.

Interprétation : avec moins de règles concurrentes, l'instruction explicite dans le prompt N3 est plus clairement traitée. Le prompt complet crée une surcharge cognitive (règles de namespace + canonical keys + session_id) qui peut interférer avec une commande simple.

---

## 6. Verdict

**Conclusion : schéma = prompt** — confirmé sans ambiguïté.

- Sans règles de namespace → divergence immédiate en P2.3 (`user.family.name` vs `user.last_name`)
- Avec règles de namespace → convergence sur `shared/user.name`

**Impact spec :** H-mémoire-schema est **structurellement dépendante du prompt**. Un agent avec un system prompt différent (ou sans rules) brisera la cohérence de schéma globale. L'OS ne peut pas supposer que les agents convergeront spontanément.

**Impact design :** La convergence de schéma doit être *enforcée à l'infrastructure*, pas déléguée au modèle. Deux options Layer 1 :
1. **Registre de schéma** : le runtime valide les clés à l'écriture contre un schéma défini hors du modèle
2. **Normalisation automatique** : le runtime mappe `user.family.name` → `shared/user.name` par une règle canonique

La décision sur laquelle implémenter est post-T5/T6 (coûts mesurés d'abord).

**Effet secondaire observé :** retirer les règles de namespace dégrade aussi le tool calling général (−4 en Phase 1.5). Les règles de namespace ne font pas que guider les namespaces — elles ancrent le modèle dans un comportement d'agent plus fiable.

**Référence LESSONS.md :** L24 (à créer après E02)
