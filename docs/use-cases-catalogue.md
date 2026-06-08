# Catalogue de cas d'usage — mise à l'épreuve du système

**Statut :** Proposition (à trier / intégrer)
**Date :** 2026-06-01
**Convention cible :** ADR-0021 (`poc/scenarios/S<N>-<slug>/README.md`)

---

## 0. Mode d'emploi

Ce document propose une **multitude de cas d'usage** organisés par niveau de
complexité croissante. L'objectif est double : (a) **mettre en avant** une
primitive ou une propriété de façon isolée et lisible, et (b) **éprouver les
fondations** par des cas qui composent, puis qui attaquent.

Chaque cas est volontairement décrit à la maille « brief » — pas un README
complet. Les READMEs (sections ADR-0021 D1.b) se rédigent à l'intégration, une
fois le cas retenu et son numéro `S<N>` canonique attribué.

### Numérotation

Les cas portent un identifiant catalogue `UC-N` **provisoire**. À l'intégration,
chacun reçoit un numéro de scénario `S<N>` (entier strictement croissant, jamais
réutilisé — ADR-0021 D2). Les `UC-N` ne préjugent pas de l'ordre `S<N>`.

### Légende

| Symbole | Signification |
|---|---|
| **Propriété** | P1a/P1b densité · P2 rollback · P3a/b/c traçabilité · P4 isolation · P5 déterminisme · P6 atomicité crash |
| **Impact** | `aucun` (exerce l'existant) · `code` (révision probable) · `ADR` (amendement/nouveau) |
| **Complexité** | ◐ faible · ◐◐ moyenne · ◐◐◐ élevée · ◐◐◐◐ très élevée |
| **Recevabilité** | ✅ recevable maintenant · ⏳ conditionnel (dépendance nommée) · 🚫 hors périmètre (raison nommée) |
| **Régime** | `R1` effets (P2/P3/P4/P6 — actif partout) · `R2` ressources (P1/P5/C1/C2 — inférence locale uniquement) · `R1+R2` les deux simultanément |

### Garde-fous transverses (à appliquer dans chaque cas)

Trois artefacts du PoC Linux peuvent faire passer un système fragile pour
robuste. Ils sont rappelés ici parce qu'ils conditionnent la **recevabilité du
verdict**, pas seulement la rédaction :

1. **Page-cache du noyau (L32).** Un `kill -9` laisse le page-cache intact : un
   commit « durable » jamais écrit sur disque survit quand même → durabilité
   fantôme. Seul un crash qui *invalide le cache* (VM power-off, ou
   `echo 3 > drop_caches` + kill) ajoute de l'information pour P6 niveau machine.
2. **Inférence stubbée (F1).** Saturer un `SleepyBackend` mesure une file
   d'attente, pas une dégradation réelle. Tout cas de plafond d'inférence sous
   charge est non recevable tant que l'inférence est stubbée.
3. **Biais L68 (« happy path = preuve »).** Un chemin nominal vert ne prouve pas
   la propriété. Pour tout cas adversarial, l'**oracle doit pouvoir falsifier** :
   si l'oracle est aveugle au vecteur (ex. compter les `0x14` pour détecter un
   masquage de refus), il *est* l'angle mort, pas le témoin.

Et un garde-fou de cadre : **frontière LLM = non-objectif** (spec/08 §0.1). Le
DAG garantit *happened-before*, pas la connaissance effective ni la « bonté »
sémantique de la sortie LLM. Un LLM produisant une décision nuisible *dans son
périmètre de caps* n'est pas une violation de propriété système.

---

## Niveau 0 — Cartographie de l'existant (ne pas redévelopper)

Pour éviter les doublons et situer les nouveaux cas. Ces scénarios sont déjà
implémentés et verts.

| Scénario | Primitive / ADR | Propriété | Ce qu'il couvre déjà | Régime |
|---|---|---|---|---|
| S1 supervision-algorithmique | Superviseur Rust déterministe / ADR-0006 | P4 (supervision) | Worker LLM + superviseur déterministe (pas un 2e LLM corrélé) | R1 |
| S2 self-rollback-incoherence | A1 + A2 / ADR | P2 | Auto-correction sur décision LLM incohérente | R1 |
| S3 inference-cap | InferencePool / ADR-0022 | C1 (borne dure) | Cap k=4, état `WaitingInference` observable | R2 |
| S4 scheduler-rollback | Rollback + révoc. caps / ADR-0007 | P2 × P4 | Rollback scheduler + révocation D5+D8 | R1 |
| S6 crash-atomicity | Crash injection / ADR-0024 | P6 (SEF-4) | 4 `CrashPoint` × 2 actions × K=5 = 40 runs, **niveau process** | R1 |
| S12 scheduler-coordinator | C1/C2 réveil à la demande / ADR-0030/0031 | P1b | Dormants réveillés, `cap_io` respecté, actifs bypass C2 | R2 |
| S13 persistence-restart | Réouverture RocksDB / SEF-1 | P2/P6 | État + log + bloc 64o bit-à-bit après redémarrage propre | R1 |
| S14 causal-lookup | CausalLog get / SEF-5 | P3a | p99 ≤ 10 ms sur N=10⁸ (mesuré ×5–7 sous cible) | R1 |
| SEF-6 (replay) | Horloge substituable / ADR-0028 | P5 | Déterminisme de transition (replay 1000 messages) | R2 |
| SEF-7.1/.2/.3 | `agent_add_cause` adversarial / ADR-0036 | P3 | Forgerie refusée, flood borné à 16, robustesse reconstruct | R1 |
| SEF-9 confused-deputy | Rate-limit `0x14` / ADR-0051 §D2 | P4 (audit) | Masquage de refus levé (agrégation par resource ≤32) | R1 |

**Trou structurel identifié (rappel ADR-0050 D4) :** l'atomicité **niveau
machine** (cache invalidé) en **régime concurrent** (N actions en vol) n'est pas
testée. S6 reste niveau process (page-cache intact). → voir UC-17.

---

## Niveau 1 — Vitrines mono-primitive

Un cas = une primitive ou une propriété, chemin nominal, lisible en 5 minutes.
Comble les trous de cartographie où une primitive existe sans scénario dédié.

| ID | Primitive / ADR | Propriété | Démontre | Impact | Cplx | Recev. | Régime |
|---|---|---|---|---|---|---|---|
| **UC-1** | `agent_add_cause` (légitime) / ADR-0003, ADR-0008 | P3c | Construction d'un **vrai nœud de merge** (N>1 parents) dans le DAG | aucun | ◐ | ✅ | R1 |
| **UC-2** | Délégation cap scope-prefix / ADR-0005 | P4 | Atténuation : un agent délègue une cap à scope plus restreint, exercée puis refusée hors-scope (`0x14`) | aucun | ◐◐ | ✅ | R1 |
| **UC-3** | Session bornée / ADR-0012 | P3 | `SessionBoundary (0x0A)` forcée à N_max actions / 24h ; pas de mémoire cross-session sans citation | aucun | ◐◐ | ✅ | R1 |
| **UC-4** | A3 canal de validation — chemin **timeout** / ADR-0013, ADR-0014 | P4 | `0x08` → pas de réponse → `Timeout` (`0x09`) à 30 s → l'agent reprend `Active` et **décide** (pas de transition auto) | aucun | ◐◐ | ✅ | R1 |
| **UC-5** | Watchdog WASM / ADR-0025 | — | Budget d'exécution par `AgentProfile` ; interruption coopérative (époque) d'une boucle | aucun | ◐◐ | ✅ | R1 |
| **UC-6** | Restart policy / ADR-0013, ADR-0014 | P4 | `one_for_one` vs `rest_for_one` sur crash worker (`AgentCrash 0x13`) ; isolation de faute | aucun | ◐◐ | ✅ | R1 |
| **UC-7** | A1 introspection / spec 02c | P3 (côté agent) | L'agent lit `last_action_id`/`seq`/`lifecycle` pour s'auto-situer avant décision | aucun | ◐ | ✅ | R1 |
| **UC-8** | Contrat `emit` / ADR-0010 | P6 | Séquence `emit → commit_barrier → log_append → store_put` ; atomicité cross-store nominale | aucun | ◐◐ | ✅ | R1 |

**Notes.**
- **UC-1** est le pendant *légitime* de SEF-7 : SEF-7 prouve qu'on *refuse* une
  forgerie ; UC-1 prouve qu'on *restitue* correctement un merge réel via
  `reconstruct`. Les deux sont nécessaires (l'un sans l'autre laisse un angle
  mort sur la branche acceptée).
- **UC-4** : le chemin nominal (verdict `approved`) est implicitement couvert par
  S1 ; le **chemin timeout** ne l'est pas explicitement et porte la décision
  ADR-0014 (timeout fixe 30 s, pas de retry, pas d'action auto).
- **UC-5** : le watchdog confinant le budget CPU de l'agent est une primitive
  d'effet (isolation d'exécution), indépendante de la topologie d'inférence → R1.

---

## Niveau 2 — Compositions

Deux à trois primitives qui interagissent. C'est là qu'apparaissent les
interactions de sémantique (TOCTOU, slot zombie, ligne du commit barrier).

| ID | Primitives / ADR | Propriété | Démontre | Impact | Cplx | Recev. | Régime |
|---|---|---|---|---|---|---|---|
| **UC-9** | Rollback + invalidation cap en cascade / ADR-0007 | P2 × P4 | Rollback d'une action ayant délégué des caps → invalidation récursive O(depth) → sous-agent voit sa cap révoquée au prochain accès (**pas de cache**, re-vérification) | aucun | ◐◐◐ | ✅ | R1 |
| **UC-10** | `agent_infer` annulé pendant `WaitingInference` / ADR-0019, ADR-0031 | P2 × C1 | Rollback scheduler sur agent en inférence → `cancel` (CancellationToken) → **libération immédiate du slot** → `InferenceCancelled (0x0E)`. Ordre `cancel → send Rollback → log SchedulerRollback` | aucun | ◐◐◐ | ✅ | R1+R2 |
| **UC-11** | A2 self-rollback vs effet déjà émis / spec 02c §4 | P2 | Tentative de self-rollback sur action **post-`emit`** → refus. La ligne de démarcation est le commit barrier | aucun | ◐◐ | ✅ | R1 |
| **UC-12** | Compensation journal / ADR-0024 | P6 | `CompensationOpen (0x11)` sans `Close (0x12)` après crash → `reconstruct` détecte l'orphelin → **aucun état partiel observable** | aucun | ◐◐◐ | ✅ | R1 |
| **UC-13** | Propagation d'erreur cross-agent / ADR-0015 | P3/P4 | Crash de A (`0x13`) ; B causalement dépendant → synthèse `Lifecycle::Terminated` à la lecture, pas de message orphelin livré à B | aucun | ◐◐◐ | ✅ | R1 |
| **UC-14** | Anti-famine Batch→Foreground / ADR-0023 | P1b | Agent `Batch` affamé par un flux `Foreground` continu → promu après `max_starvation_ms`. Invariant d'équité mesurable | aucun | ◐◐ | ✅ | R2 |
| **UC-15** | Déterminisme + horloge + aléa / SEF-6, ADR-0028 | P5 | Deux instances, même séquence 1000 messages, `LogicalClock` en replay → même hash final. **Reproductibilité sémantique seulement** si LLM impliqué | aucun | ◐◐◐ | ✅ | R2 |
| **UC-16** | Révocation récursive profonde / ADR-0005, ADR-0007 | P4 | Arbre de délégation à k niveaux → mesure du coût `revoke()` O(depth) sous profondeur croissante | code? | ◐◐ | ✅ | R1 |

**Notes.**
- **UC-10 est le cas TOCTOU central.** L'intérêt n'est pas le chemin nominal mais
  la **fenêtre** entre « slot acquis » et « rollback décidé ». L'oracle doit
  vérifier qu'aucun slot zombie ne subsiste (`available_permits()` revenu à
  l'état attendu) **et** l'ordre des `EmitType`, pas seulement que le test passe.
  **R1+R2** : le rollback (P2) est R1 ; la libération du slot C1 est R2.
- **UC-14** : l'anti-famine est une propriété du pool d'inférence (C1/C2) → R2.
  Recevable avec `SleepyBackend` pour l'invariant d'équité ; non recevable pour
  la mesure de latence réelle (F1).
- **UC-15** : avec LLM réel sous Ollama, le bytewise est impossible (sampling).
  L'oracle est : même état final `ContentStore` + même séquence d'`EmitType`. Ne
  pas écrire un assert de hash bytewise du log — il échouerait pour une raison
  non-sémantique (réordonnancement de payloads JSON).
  **R2 conditionnel** : P5 suppose S1∧S6 ; sémantique seulement avec LLM local ;
  bytewise impossible.
- **UC-16** : si `revoke()` O(N) actuel (documenté L21) devient un point chaud
  sous profondeur, c'est le déclencheur d'un ADR d'optimisation — pas avant
  mesure (discipline non-anticipatoire).

---

## Niveau 3 — Adversarial / falsification

Ces cas attaquent une propriété. La règle : **l'oracle doit pouvoir falsifier**.
Un cas adversarial qui ne peut produire qu'un verdict « pass » est un théâtre.

| ID | Vecteur / ADR | Propriété attaquée | Falsifie si… | Impact | Cplx | Recev. | Régime |
|---|---|---|---|---|---|---|---|
| **UC-17** | **Crash machine concurrent + cache invalidé** / ADR-0050 D4, ADR-0027 §D3 | P6 | Après recovery : (a) un commit **acké** absent/non-reconstructible ; OU (b) un commit **partiel** visible ; OU (c) un `parent_ids` pendant. Le log post-recovery doit être un **préfixe valide** de la séquence ackée | code/ADR | ◐◐◐◐ | ✅ | R1 |
| **UC-18** | WASM adversarial (OOB, div0, `unreachable`, boucle) / C.11, ADR-0048 | P4 (isolation), terminaison | Le trap d'un agent tue/corrompt un **autre** agent ou le store ; ou la boucle n'est pas interrompue | code | ◐◐◐ | ✅ | R1 |
| **UC-19** | Flood de refus > 32 resources distinctes / T12, ADR-0051 §D2 | P4 (complétude d'audit) | Une resource sensible nouvelle est masquée alors que l'ensemble distinct dépasse la borne 32 | aucun/ADR | ◐◐◐ | ✅ | R1 |
| **UC-20** | Forgerie causale citant une **vraie** action d'un autre agent / T6(ii), ADR-0036 | P3 (intégrité causale) | Le superviseur conclut « B a réagi à A » alors que B a seulement cité un `action_id` fuité | ADR | ◐◐◐ | ✅ | R1 |
| **UC-21** | DoS pool d'inférence sous **inférence réelle** / T5, ADR-0050 D6 | P1b/C2 | Dégradation p99 au-delà du budget sous saturation réelle | code | ◐◐◐ | ⏳ Phase 10 + C2 recalibré | R2 |
| **UC-22** | Collusion multi-agents (saturation C1 coordonnée) / spec 08 §3 | P1b/P4 | Deux agents coordonnés contournent une garantie mono-agent | ADR | ◐◐◐ | 🚫 hors périmètre Phase 7 | R1+R2 |

**Notes — constructibilité des oracles (critique).**
- **UC-17.** Garde-fou de recevabilité : un crash **niveau process** (page-cache
  intact) n'est **pas** recevable pour cet axe — il ne falsifie pas plus que S6.
  Le verdict n'est valide que sous invalidation de cache effective (VM power-off
  ou `drop_caches` + kill). C'est le cas le plus structurant du niveau 3 : c'est
  le trou jamais testé. **R1** : P6 est indépendant de la topologie d'inférence.
- **UC-19.** L'oracle qui *compte* les `0x14` est l'angle mort (il ne voit pas ce
  qui est silencié). Seul un **témoin hors-bande au point de décision**, émis
  avant le rate-limit et hors du log causal, falsifie. UC-19 est l'extension de
  SEF-9 *au-delà* de la borne 32 : il documente où l'attribution se dégrade — ce
  qui peut soit confirmer la borne comme suffisante, soit motiver un ADR.
- **UC-20.** En mono-tenant, c'est une **limite documentée**, pas un bug : pas de
  capability cross-agent (reportée à B-fort, déclenché par le passage
  multi-tenant — ADR-0036). UC-20 sert à *écrire* le critère de sortie B-fort, pas
  à « réparer » quelque chose maintenant.
- **UC-21 / UC-22.** Gardés dans le catalogue pour traçabilité, mais **non
  recevables maintenant** : UC-21 saturerait un stub (F1) avec un C2 sous-estimé
  ×3–4 (L32) ; UC-22 est explicitement hors périmètre Phase 7. Les qualifier
  serait valider un vœu.

---

## Niveau 4 — Cross-cutting / multi-propriété / frontière substrat

Les cas les plus durs : plusieurs propriétés en interaction simultanée, ou un
changement de substrat qui remet en cause la transférabilité des verdicts.

| ID | Périmètre / ADR | Propriétés | Démontre / interroge | Impact | Cplx | Recev. | Régime |
|---|---|---|---|---|---|---|---|
| **UC-23** | Scénario « tempête » | P2 × P4 × P6 | Agent en `WaitingInference` + rollback scheduler concurrent invalidant des caps déléguées + 2e agent atteignant sa frontière de session + crash machine en suivant. **L'ordre d'arbitrage P4 ≻ P2 ≻ P6 tient-il sous interaction réelle ?** | code/ADR | ◐◐◐◐ | ✅ | R1 |
| **UC-24** | Re-validation P4 sur seL4 / D7, ADR-0037/0038 | P4 | Rejouer l'isolation cap (axe 1) sur seL4 (W^X réel, VSpace matérielle) vs Linux logiciel. **Tout verdict doit nommer le substrat** | ADR | ◐◐◐◐ | ⏳ stack seL4 | R1 |
| **UC-25** | Densité active P1b sous inférence réelle | P1b | R_actif ≥ 2× vs Docker à capacité d'inférence équivalente | code | ◐◐◐ | ⏳ Phase 10 (pas de `SleepyBackend`) | R2 |
| **UC-26** | Durabilité power-loss / ADR-0027 | P6 | `WriteOptions::sync=true` sur chemin chaud, durabilité au sens power-loss | ADR | ◐◐◐◐ | 🚫 sur Linux/PoC (cible : hardware qualifié i4i, ou serveur de bloc seL4) | R1 |
| **UC-27** | Portage `reconstruct` sur store natif seL4 / ADR-0018, ADR-0038 | P3 | Même concept de rejeu du log, lecture du store natif au lieu de RocksDB | code | ◐◐◐ | ⏳ store natif seL4 | R1 |

**Notes.**
- **UC-23** est le test d'intégrité de l'ordre d'arbitrage ADR-0001. Il ne
  « passe/échoue » pas trivialement : son intérêt est de produire une **trace**
  où l'on observe quelle propriété a été cédée et si c'est bien la plus basse de
  l'ordre. C'est un candidat à amendement d'ADR si l'ordre se révèle incohérent
  sous interaction (tension non anticipée → ADR de remplacement, conformément à
  ADR-0001). **R1** : P2, P4, P6 sont toutes des propriétés d'effet.
- **UC-24 / UC-26 / UC-27** dépendent de la stack seL4. Garde-fou D7 :
  **non-transférabilité**. Un verdict de robustesse Linux ne se transporte pas à
  seL4 sans re-validation, et inversement. Tout verdict de UC-18/UC-23 obtenu sur
  Linux doit se nommer « P4 sous adversaire WASM, isolation logicielle Linux » —
  pas « P4 » nu.

---

## Annexe — matrice de couverture par propriété

Vérifie qu'aucune propriété ne reste sans cas *recevable maintenant*.

| Propriété | Régime | Existant (N0) | Nouveaux recevables (N1–N4) |
|---|---|---|---|
| P1a densité hébergée | R2 | T6 (dev) | — (mesure, pas scénario) |
| P1b densité active | R2 | S12 | UC-14 ✅ · UC-25 ⏳ |
| P2 rollback | R1 | S2, S4, S13 | UC-9, UC-10, UC-11 ✅ |
| P3a traçabilité lookup | R1 | S14 | UC-7 ✅ |
| P3c causalité concurrente | R1 | SEF-7 | UC-1 ✅ · UC-20 (limite) |
| P4 isolation | R1 | S1, S4, SEF-9 | UC-2, UC-6, UC-16, UC-18, UC-19 ✅ · UC-24 ⏳ |
| P5 déterminisme | R2 | SEF-6 | UC-15 ✅ |
| P6 atomicité crash | R1 | S6, S13 | UC-8, UC-12, **UC-17** ✅ · UC-26 🚫 |
| Supervision / cycle de vie | R1 | S1 | UC-3, UC-4, UC-5, UC-13 ✅ |
| Pool C1/C2 | R2 | S3, S12 | UC-10 (R1+R2) · UC-21 ⏳ |

**Priorité suggérée (sous discipline non-anticipatoire et ordre ADR-0001) :**
1. **UC-17** — c'est le trou structurel (P6 niveau machine, régime concurrent).
   P6 est falsifiable nettement et la cible concrète existe (ADR-0027 §D3).
2. **UC-10**, **UC-9** — interactions P2×P4/C1 à sémantique subtile, recevables
   et peu coûteuses, fort rendement en confiance sur les fondations.
3. **UC-1**, **UC-12**, **UC-13** — comblent des branches acceptées non vues
   (merge légitime, orphelin de compensation, terminaison propagée).
4. Le reste au fil de l'eau ; les ⏳/🚫 restent en attente de leur dépendance
   (Phase 10, stack seL4, hardware qualifié) — ne pas les qualifier avant.
