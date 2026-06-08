# ADR-0009 — Profils d'acteurs LLM et séparation machine/humain

**Date :** 2026-05-14
**Statut :** Acceptée

---

## Contexte

E03 et E03b (lab, 2026-05-14) ont mesuré l'impact du format de sortie LLM sur le coût d'inférence et la fiabilité des tool calls. Ces mesures révèlent deux profils d'acteurs LLM aux propriétés radicalement différentes, et précisent où la séparation entre sortie machine et lisibilité humaine doit être implémentée.

**Ce que les mesures montrent :**

| Mode | Tokens/appel | Latence stable | Tool calls | Phase 1.5 |
|------|-------------|----------------|------------|-----------|
| Prose (format libre) | 41–50 | 6–18s | ✓ | 10/11 |
| `format=json` (Ollama) | 6–9 | 1.5–2.5s | ✗ cassé | 0/11 |
| `format=json`, aucun tool call | 6–9 | 2–2.5s | n/a | n/a |

La prose représente ~85% des tokens générés en mode tool-calling. `format=json` réduit ce coût 5–8× mais court-circuite le mécanisme de tool call d'Ollama (régression complète Phase 1.5). Le profil pure-décision (`format=json`, pas de tool calls) fonctionne end-to-end : log causal, causalité chaînée, rollback — 8/8 (E03b).

**La prose joue probablement un rôle d'échafaudage pour les modèles < 7B.** Ce n'est pas seulement un habillage pour lecteur humain — c'est une part du chemin de raisonnement qui mène au bon tool call. Demander au modèle de « moins parler » dégrade la fiabilité du routage texte/outil avant de réduire significativement le coût. La séparation machine/humain ne peut donc pas être faite au niveau de l'API LLM pour les agents tool-calling, indépendamment du backend.

**La couche d'interposition WASM est le bon endroit.** Le poc/runtime expose une host function `emit(ptr, len)` qui est le seul point par lequel un acteur peut publier dans le log causal. Le LLM peut générer de la prose en interne — cette verbosité reste interne à l'acteur, paye uniquement en latence d'inférence, et n'encombre pas le log. Seul ce que le module WASM appelle explicitement avec `emit` atterrit dans le log causal. La distinction machine/humain est donc structurelle, pas une contrainte de prompt.

---

## Décision

**1. Deux profils d'acteurs LLM sont reconnus dans le modèle de coûts du scheduler :**

**Profil T — Tool-calling** : l'acteur utilise les primitives mémoire, déclenche des tool calls, opère en mode prose libre. Coût d'inférence : ~6–18s/cycle sur CPU (qwen2.5:3b). Format de sortie non contraint — la prose est fonctionnelle. `format=json` désactivé.

**Profil D — Pure-décision** : l'acteur prend des entrées structurées et produit une décision JSON sans tool calls. Coût d'inférence : ~2–2.5s/cycle stable. `format=json` activé. Latence ~3–8× inférieure au profil T. Utilisable pour les nœuds de merge, d'arbitrage, de scoring, de planification sans accès mémoire.

Le scheduler doit connaître le profil de chaque acteur pour allouer correctement la capacité d'inférence (H-inférence-coût : cette ressource est bornée et doit être comptabilisée séparément des ressources CPU/mémoire).

**2. La séparation machine/humain vit à la couche `emit()` du runtime WASM, pas au niveau de l'API LLM.**

Le log causal reçoit uniquement ce que les acteurs émettent explicitement via `emit`. Ce contenu peut être compact (décision JSON de 8–9 tokens) ou informatif (texte structuré). La lisibilité humaine est une projection construite à la demande sur ce log, pas une propriété intrinsèque du log.

**3. Révision de ADR-0006 : le pari sur la supervision continue (Modèle A) est abandonné au profit du Modèle B.**

ADR-0006 avait retenu provisoirement le Modèle A (supervision continue, log structuré lisible en permanence) en l'absence de données sur la fréquence réelle de supervision. La condition de révision était : « une mesure réelle montre que la fréquence de supervision est inférieure à une ouverture par heure par session-agent. »

E03 fournit la donnée manquante par un chemin différent : si la séparation machine/humain vit à la couche `emit`, alors le log causal n'a pas besoin d'être human-readable par défaut. Le Modèle A est rendu redondant par l'architecture, pas par une mesure de fréquence.

**Le Modèle B est adopté :** la machine écrit en continu le strict nécessaire (état hashé, émissions structurées, ordre causal). La couche lisible est matérialisée à la demande. P3 (traçabilité causale) se reformule : *la chaîne causale complète peut être reconstruite en temps borné par la profondeur de la chaîne, à partir du log compact* — propriété d'intégrité, pas de latence de lookup.

---

## Alternatives considérées

| Alternative | Raison du rejet |
|-------------|----------------|
| Contraindre la sortie LLM au niveau du prompt seul (`system_prompt_machine.txt` sans `format=json`) | Réduction ~33% des tokens sur les writes, inefficace sur les reads/queries, non fiable. La verbosité est fonctionnelle pour le routage outil/texte chez les < 7B. |
| Un seul profil d'acteur avec `format=json` conditionnel selon l'itération (tool calls = prose, réponse finale = JSON) | Non supporté par l'API Ollama actuelle. `format=json` s'applique à toute la génération, pas à la réponse finale seulement. |
| Maintenir le Modèle A de ADR-0006 et reporter la décision | Le coût est de concevoir T6 (H-densité) et le contrat de `emit` dans le brouillard d'un pari implicite. Chaque décision d'aval est prise sans base. |

---

## Conséquences

**Sur le scheduler (T6 / H-densité) :**
T6 doit mesurer deux bornes séparées : acteurs Profil D (~2.5s/cycle, densité haute) et acteurs Profil T (~6–18s/cycle, densité basse). Un chiffre de densité unique mélange deux régimes à 3–8× d'écart — il n'aurait pas de signification. Le mix d'acteurs D/T d'un déploiement détermine la densité effective.

**Sur P3 (traçabilité causale) :**
La formulation O(1) lookup de P3 tombe avec le Modèle A. P3 se reformule en : *l'intégrité causale est garantie à l'écriture ; la reconstruction d'une vue lisible est bornée par la profondeur de la chaîne demandée*. Cette reformulation est plus forte sur l'intégrité (rien n'est perdu) et plus faible sur la latence de lookup (dépend de la fenêtre). `spec/02-properties.md` §P3 doit être mis à jour.

**Sur P1 (densité d'agents) :**
Le passage au Modèle B réduit l'overhead du log causal continu. La borne de densité 5× est plus atteignable qu'avec le Modèle A. Les deux effets combinés (Modèle B + Profil D pour les acteurs de calcul) peuvent décaler significativement la borne P1 vers le haut.

**Sur `poc/runtime` (`emit`) :**
Le contrat de `emit` doit être précisé : format des émissions, typage, indexation dans le log causal compact. C'est le prochain chantier de conception avant T6.

**Sur le lab :**
Le lab continue de fonctionner en Modèle A (SQLite lisible) pour les tests fonctionnels — c'est la bonne couche pour valider le comportement. Les mesures de coût/densité (T6) se font sur le poc Rust en Modèle B.

---

## Références

- `lab/experiments/E03-machine-output.md` — mesures format=json vs prose ; format=json incompatible tool calls
- `lab/experiments/E03-machine-output.md` §E03b — profil D validé end-to-end (8/8, log causal, rollback)
- `lab/LESSONS.md` §L25 — intuition d'origine sur la verbosité LLM
- `lab/LESSONS.md` §L26 — diagnostic format=json + identification couche emit
- ADR-0006 — modèle de supervision (révisé par ce document)
- `poc/runtime/src/actor.rs` — host function `emit` (point de séparation machine/humain)
- `spec/02-properties.md` §P1, §P3 — propriétés affectées par ce changement

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
