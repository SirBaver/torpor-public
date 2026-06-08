# E03 — Sortie machine-first vs prose — impact sur le coût d'inférence

| Champ | Valeur |
|-------|--------|
| Identifiant | E03 |
| Hypothèse testée | H-inférence-coût (`spec/04-hypotheses.md`) — corollaire output format |
| Date planifiée | 2026-05-14 |
| Statut | Complète — 2026-05-14 |

---

## 1. Objectif

Les mesures actuelles d'H-inférence-coût (130s / 10 appels à 3B, 725s à 7B) incluent la génération de prose : le modèle produit des phrases d'explication avant et après chaque tool call parce que le system prompt suppose un lecteur humain.

Si la sortie finale est contrainte à `{"ok":true}` (machine-first), le modèle génère moins de tokens par action.

**Question :** Quel est le ratio tokens prose vs tokens machine-first ? Quel est l'impact sur `inference_ms` ?

**Conséquence architecturale :** si le ratio est > 3×, les benchmarks H-densité (T6) et H-inférence-coût doivent être refaits en mode machine-first — les chiffres prose sous-estiment la densité atteignable de l'OS.

---

## 2. Configuration

| Paramètre | Prose (baseline) | Machine-first |
|-----------|-----------------|---------------|
| Modèle | `qwen2.5:3b` | `qwen2.5:3b` |
| System prompt | `config/system_prompt.txt` | `config/system_prompt_machine.txt` |
| OUTPUT_FORMAT | `""` (aucun) | `"json"` |
| Tests | P2.4 (H-inférence-coût) | idem |

**Nouvelle métrique exposée :** `eval_tokens` dans chaque réponse `/think` — nombre de tokens générés (cumulé sur toutes les itérations de la boucle tool-calling).

---

## 3. Procédure de reproduction

```bash
# ── Baseline prose (déjà mesuré en P2.4) ─────────────────────────────────────
# inference_ms et eval_tokens disponibles dans les réponses /think depuis ce run

# ── Machine-first ─────────────────────────────────────────────────────────────

# 1. Démarrer le daemon avec le prompt machine + format json
SYSTEM_PROMPT_FILE=/app/config/system_prompt_machine.txt \
  OUTPUT_FORMAT=json \
  docker compose up -d daemon

# 2. Vérifier la config
curl -s http://localhost:8888/health

# 3. Smoke test complet pour vérifier que l'infra ne régresse pas
bash tests/smoke_test.sh --fresh 2>&1 | tee /tmp/E03-smoke-machine.txt

# 4. Test d'inférence ciblé P2.4 (chaîne de 4 appels avec mesure timing)
#    → utiliser les scripts de P2.4 du smoke test ou curl manuel
#    → noter inference_ms et eval_tokens pour chaque appel

# 5. Restaurer le défaut
docker compose up -d daemon
```

---

## 4. Critères

### 4.1 Infrastructure (ne doit pas régresser)

| Test | Attendu |
|------|---------|
| Phase 1 (API) | 15/15 |
| Phase 1.5 (tool calling) | ≥ 9/11 (T4c restera probablement FAIL) |
| Phase 2B, 3, 4 | inchangé |

### 4.2 Mesure de réduction tokens

| Métrique | Baseline prose | Machine-first | Ratio attendu |
|----------|---------------|---------------|---------------|
| `eval_tokens` / appel | mesurer | mesurer | > 2× réduction |
| `inference_ms` / appel | ~13–50s | < baseline | proportionnel à tokens |

### 4.3 Seuil d'interprétation

- **Ratio tokens > 3×** → impact majeur sur H-densité ; T6 doit être refait en mode machine-first
- **Ratio tokens 1.5–3×** → impact modéré ; noter mais T6 peut se faire en mode prose d'abord
- **Ratio tokens < 1.5×** → la prose ne coûte pas significativement ; hypothèse L25 non confirmée à ce niveau

---

## 5. Résultats

Exécuté le 2026-05-14, qwen2.5:3b, `system_prompt_machine.txt` + `OUTPUT_FORMAT=json`.

### 5.1 Smoke test machine-first (partiel — arrêt après Phase 2)

| Phase | PASS | FAIL | Delta vs prose |
|-------|------|------|----------------|
| Phase 1 — API de base | 15 | 0 | = |
| Phase 1.5 — Tool calling | **0** | **8** | **−10** (était 10/11) |
| Phase 1.6 — Causalité session | 4 | 0 | = |
| Phase 2 — Multi-agent (infra) | 7 | 1 | P2.3 FAIL (aucun write) |

Phase 3+ non exécutée — interrompue par les FAILs critiques de Phase 1.5.

### 5.2 Tokens et timing

| Appel | Prose tokens | Prose ms | Machine tokens | Machine ms | Ratio tokens | Ratio ms |
|-------|-------------|----------|----------------|------------|--------------|----------|
| WRITE simple | 46 | 8016 | 6 | 1683* | **7.7×** | **4.8×** |
| READ simple | 41 | 5762 | 6 | 1583 | **6.8×** | **3.6×** |
| LIST multi-step | 46 | 7223 | 9 | 1863 | **5.1×** | **3.9×** |
| Orchestrateur P2 | ~50 | 13747 | ~6 | 2269 | **~8×** | **6.1×** |
| Agent-A P2 | ~50 | 15475 | ~6 | 1732 | **~8×** | **8.9×** |
| Agent-B P2 | ~50 | 17532 | ~6 | 2159 | **~8×** | **8.1×** |
| Merge P2 | ~30 | 5263 | 6 | 1487 | **~5×** | **3.5×** |

*Premier appel WRITE post-reset : 22378ms (anomalie — context load). Ignoré.

### 5.3 Comportement observé

```
Conformité {"ok":true} : ~5/7 appels directs (sans tool call)
Divergences détectées :
  - WRITE-2 série manuelle : modèle a sorti le tool call comme JSON final
    → {"name": "memory_write", "arguments": {...}} au lieu d'exécuter le tool
  - Phase 1.5 : format=json force toutes les sorties en JSON valide,
    y compris les tool calls → le modèle output {} ou ne fait pas de tool call du tout
```

**Cause racine :** Ollama `format=json` applique la contrainte à l'intégralité de la génération, y compris les séquences de tool calling. Le modèle "voit" qu'il doit produire du JSON valide et court-circuite le mécanisme de tool call natif.

---

## 6. Verdict

**Ratio tokens prose/machine :** ~5–8× (confirmé). La prose représente ~85% des tokens générés.

**Latence :** 3.5–9× plus rapide en machine-first quand le modèle répond directement (sans tool call). Pour la chaîne P2 (4 appels sans tool call utile), total ~7.2s machine vs ~52s prose — **7.2× plus rapide**.

**Blocage identifié : `format=json` est incompatible avec les tool calls Ollama.** Phase 1.5 passe de 10/11 à 0/11. La contrainte JSON s'applique à la génération entière, empêchant le mécanisme de tool call natif. Ce n'est pas un problème de modèle — c'est une limitation de l'API Ollama en mode `format=json`.

**Deux usages distincts à séparer :**

| Usage | Format | Tool calls | Vitesse |
|-------|--------|------------|---------|
| Agent avec mémoire (tool calling actif) | prose libre | ✓ | baseline |
| Agent de calcul / décision (pas de tool call) | `format=json` | ✗ | 5–9× plus rapide |

**Stratégie pour l'OS :** ne pas utiliser `format=json` en général. Testé également : `system_prompt_machine.txt` sans `OUTPUT_FORMAT=json` — le modèle produit ~31 tokens pour les writes (vs 46 prose, −33%) et tool call fonctionne, mais revient à la prose complète pour les read/query (44-48 tokens, aucune conformité `{"ok":true}`). La réduction de tokens via prompt seul est faible et non fiable.

**Impact H-densité :** T6 devra distinguer deux modes d'acteurs :
- Acteurs tool-calling (prose nécessaire) → densité bornée par latence ~6-8s/cycle
- Acteurs calcul pur (format=json possible) → densité ~5–9× plus haute, latence ~1.5-2s/cycle

**Chemin de sortie :** la réduction de tokens n'est pas accessible via l'API Ollama pour les agents tool-calling. Elle devient accessible en sortant du paradigme LLM→prose : le poc/runtime (WASM + host functions `emit`) contrôle ce qui est publié dans le log causal indépendamment de la verbosité interne du LLM. C'est là que la séparation sortie-machine / lisibilité-humaine doit être implémentée.

**Vérification E03b (pure-décision, format=json, pas de tool calls) :** 8/8 — log causal, causalité chaînée, rollback, tous PASS. Profil D validé end-to-end. Tokens : 8 + 9 = 17 pour deux décisions chaînées. Latence stable : 2.4s (hors premier appel post-reset).

**Référence LESSONS.md :** L25, L26 ; **ADR :** ADR-0009
