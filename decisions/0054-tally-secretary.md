# ADR-0054 — Secrétaire de vote WASM (variante C) : quorum et abstention

**Date :** 2026-05-31
**Statut :** Acceptée

---

## Contexte

Le scénario de comité technique de déploiement (N=3 voter_agents LLM 3B
non-déterministes) exige un composant qui agrège les votes, calcule la
majorité, et émet la décision dans le log causal. Variante C retenue : un
acteur WASM sans LLM, `tally_secretary.wasm`, qui calcule la majorité par
code pur et émet la décision liée causalement aux N votes via `agent_add_cause`
(ADR-0036 B-light, borne MAX_EXTRA_CAUSES = 16, suffisante pour N ≪ 16).

Deux variantes écartées :
- **A** (runner émet la décision) : le runner n'est pas un acteur — écrire dans
  le log hors `agent_add_cause` rouvre T6 (ADR-0036) et forge des arêtes sans
  `agent_id` valide.
- **B** (secrétaire LLM) : place un LLM sur une agrégation à définition
  arithmétique close ; L97/L98/L101 démontrent que les 3B re-évaluent au lieu
  d'agréger — on rendrait indécidable une fonction décidable.

---

## Décision

### D1 — Règle de quorum : majorité simple (> N/2)

`votes_pour > N / 2` avec N constant (votes attendus, paramètre de spawn).

**Justification :** Sur N=3, unanimité et majorité qualifiée 2/3 sont identiques
à la majorité simple (toutes requièrent ≥ 2) ; la distinction n'a de portée qu'à
N > 3. Pour un PoC dont l'objectif est la vérifiabilité du graphe causal, la règle
la plus simple est la seule justifiée.

### D2 — Abstention / timeout : dénominateur N (attendus)

Si N' < N votes sont reçus (crash watchdog ADR-0025 ou timeout), le secrétaire
calcule le quorum sur N (votes attendus), non sur N' (votes reçus). Un vote
manquant est une abstention — ne pèse ni pour ni contre.

**Justification :** Le dénominateur N' rendrait le résultat dépendant du pattern
de crash, transformant un crash adversarial en vecteur d'influence non-tracé ;
N garantit la stabilité de la décision vis-à-vis des pannes et la reproductibilité
depuis le log causal.

### D3 — Invariant causal

La décision porte comme causes les `action_id` des votes reçus (N' causes ≤ N).
Les absences sont constatables via `Terminated (0x03)` dans le log — fait causal
observable sans nécessiter de cause dans le vecteur de la décision.

### D4 — La proposition est un nœud du DAG

La proposition soumise au vote DOIT être un `ActionResult` dans le log, parent
commun des N votes (via `Message::caused`). Sans ce nœud, les votes sont des
racines sans cause commune et un audit ne peut pas répondre « sur quoi a-t-on
voté ? ».

---

## Alternatives considérées

| Option | Raison du rejet |
|--------|-----------------|
| Majorité qualifiée 2/3 | Identique à majorité simple sur N=3 ; surcharge sans bénéfice |
| Unanimité | Trivial sous non-déterminisme LLM ; masque les tensions sémantiques |
| Dénominateur N' | Crash devient vecteur d'influence ; instabilité non-tracée |
| Secrétaire LLM (B) | LLM sur calcul arithmétique = indécidable ; L97/L98/L101 |
| Runner émet (A) | Hors modèle d'acteurs, rouvre T6 ADR-0036 |

---

## Conséquences

- `tally_secretary.wasm` : code pur, `votes_pour > N / 2`, N en paramètre de spawn.
- `agent_add_cause` : N' appels (votes reçus), borne ≤ 16 ADR-0036 respectée.
- `consensus_runner` : spawne d'abord un nœud-proposition (D4), transmet son
  `action_id` aux voter_agents via `Message::caused`.
- SEF requis : `sef-tally` — relit votes + décision depuis le log, recompute tally,
  asserte `décision_dans_log == recompute(votes_reçus, N)`.

---

## Références

- `decisions/0036-autorité-causale-agent-add-cause.md` — B-light, MAX=16, T6
- `decisions/0025-profils-watchdog-wasm.md` — Terminated reason=0x04
- `decisions/0010-contrat-emit.md` — contrat émission, agent_id obligatoire
- `decisions/0003-modele-causal-dag.md` — caused_by[], sémantique input causal
- `lab/LESSONS.md` §L97/L98/L101 — comportements LLM 3B sur formats structurés
