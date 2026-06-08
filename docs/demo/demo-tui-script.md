# Script de démo TUI live — OS-pour-IA

**Public :** mixte (décideurs/investisseurs + techniques). **Format :** terminal live.
**Substrat de la démo :** pipeline `code_review_runner` — agent `reviewer` (WASM) → agent `judge` (WASM), boucle REJECT→fix→APPROVE, tout passe par le log causal.

> **Régime de la démo : R1 (contrôle des effets — P2/P3/P4/P6).** Cette démo ne montre **rien** du régime R2 (densité, pool d'inférence, déterminisme). Ne revendiquer aucune propriété R2 ici.

> **Mode par défaut : `mode: rejeu`** — affiché en permanence à l'écran. L'inférence est en conserve (réponses enregistrées d'un vrai run Ollama, cf. `reference_responses.jsonl`). Le `--live` rebranche Ollama. **Le rejeu prouve le contrôle des effets, pas une performance d'inférence (garde-fou F1).** Ne jamais maquiller le rejeu en mesure live.

**Substrat technique : Linux.** Tous les verdicts cités portent sur Linux et ne transfèrent pas à seL4 (D7). C'est de la rigueur, pas une faiblesse.

---

## Accroche d'ouverture (une scène vraie, sans jargon)

> « Voici deux IA qui travaillent ensemble. La première relit un bout de code, la seconde juge son rapport et rend un verdict. Vous allez voir, au centre de l'écran, le lien entre leurs deux décisions se dessiner. Ce lien n'est pas une note dans un journal qu'on pourrait réécrire après coup. C'est une empreinte cryptographique. Tout à l'heure, je vais essayer de tricher avec cet historique devant vous — et l'écran va me prendre la main dans le sac. »

Affiché à l'ouverture : `mode: rejeu` · `régime: R1 (effets)` · `substrat: Linux`.

---

## Temps fort 1 — La collaboration se dessine

Le DAG se construit en direct. Le `reviewer` émet son rapport (un nœud). Le `judge` lit ce rapport et rend son verdict : son nœud référence l'`action_id` exact du rapport lu. **L'arête cross-agent reviewer→judge EST un hash.**

- **(a) Caption investisseur :** « Quand la seconde IA répond à la première, le lien entre leurs décisions est une empreinte, pas un commentaire. L'historique est traçable et rejouable. »
- **(b) Drill technique `[d]` :**
  - `action_id = SHA256(bincode(LogEntry))` — content-addressed (`poc/causal-log/src/lib.rs:188`).
  - Le nœud du juge porte `parent_ids = [action_id_du_rapport]` (`code_review_runner.rs:216`, `Message::caused(...)`).
  - On déplie : `hash` du verdict, son `parent` (= hash du rapport), le `payload` MessagePack. On montre que le `parent` du juge **est** le `hash` du reviewer, octet pour octet.
- **(c) Statut épistémique & régime :**
  - **Prouvé** — DAG cross-agent content-addressed + contrat `emit` verts : S27 (`emit-contract`, P6 nominal/I-CSR), S2 (`Introspect(0x06)→SelfRollback(0x07)` observable dans le log). **Régime R1.**
  - **Limite honnête (à dire à voix haute) :** l'arête prouve « le juge a cité ce rapport », pas « le juge a *sémantiquement* compris ce rapport ». En mono-tenant, citer un `action_id` relève du niveau **B-light** (S32 / ADR-0036) : le DAG est authentique et infalsifiable, le lien *sémantique* fort cross-agent (B-fort) est multi-tenant — **conçu, non livré**.

---

## Temps fort 2 — `[t]` L'effraction (tamper-evident, P3 — intégrité)

On falsifie une entrée déjà écrite dans le log. La chaîne aval casse **visiblement** : les nœuds qui référençaient l'entrée modifiée pointent dans le vide — leur `parent_ids` ne correspond plus à aucun hash recalculable.

- **(a) Caption investisseur :** « J'essaie de réécrire l'histoire. Le système le détecte instantanément : modifier une entrée invalide toutes les suivantes. L'audit est infalsifiable par construction. »
- **(b) Drill technique `[d]` :**
  - On modifie un octet du `payload` d'une entrée → son `action_id` recalculé change (car `action_id = SHA256(bincode(LogEntry))`).
  - Affichage : `hash_attendu` (référencé par l'enfant via `parent_ids`) vs `hash_recalculé` (de l'entrée falsifiée) → **divergence**.
  - Tous les enfants dont `parent_ids` contenait l'ancien hash deviennent orphelins → l'écran surligne la rupture de chaîne aval.
- **(c) Statut épistémique & régime :**
  - **Prouvé** — tamper-evident (P3 — intégrité) repose sur le content-addressing déjà vert ; campagne causale adversariale `SEF-13` / `S32`. **Régime R1.** (« P3a » = latence de lookup, sous-lettre distincte — Q1.)
  - **Limite honnête :** « tamper-*evident* » = falsification **détectable**, pas **empêchée**. Le système ne vous empêche pas d'écrire dans la base ; il rend toute écriture illégitime **visible**.

---

## Temps fort 3 — `[r]` Le rollback atomique (P2)

Un agent annule une décision. L'état local revient exactement à un point antérieur — **sans étape intermédiaire observable**. Tout-ou-rien.

- **(a) Caption investisseur :** « Une décision est annulée d'un bloc. L'état revient au point d'avant, sans état bâtard intermédiaire. C'est la marche arrière propre. »
- **(b) Drill technique `[d]` :**
  - `hash_before` (état au point k) capturé avant la séquence ; après rollback `hash_after_rollback` → **égalité octet pour octet** `hash_before == hash_after_rollback`.
  - Log : `SchedulerRollback (0x0B)` + journal de compensation (`CompensationOpen 0x11` / `CompensationClose 0x12`).
- **(c) Statut épistémique & régime :**
  - **Prouvé** — S7 (`rollback-equivalence` / SEF-2) : après 1000 actions, rollback à l'action 500 → hash identique à celui mesuré à l'action 500. Renforcé par S2 et `SEF-12`. **Régime R1.**
  - **Limite honnête :** couvre l'**état local** (snapshot + caps). Un effet externe **déjà émis** (après commit barrier) n'est pas rétractable — contrat `emit` (S27/S28). La compensation de saga métier est **hors-périmètre** documenté.

---

## Temps fort 4 — `[x]` L'agent intrus (isolation par capabilities, P4)

Un agent tente une action hors de son périmètre de capabilities. **Bloqué à la frontière WASM.** Non contournable par l'agent.

- **(a) Caption investisseur :** « Une IA tente de sortir de son périmètre. Elle est arrêtée net, à la frontière, par le système lui-même — pas par une règle qu'on pourrait oublier d'écrire. »
- **(b) Drill technique `[d]` :**
  - Tentative d'accès → refus → `CapabilityDenied (0x14)` dans le log, horodaté et attribué à l'agent fautif.
  - Variante : accès mémoire hors-bornes WASM → `Wasmtime` trap (`MemoryOutOfBounds`) → `AgentCrash (0x13)`. Le runtime (ContentStore + CausalLog) **n'est pas corrompu** ; un autre agent continue d'émettre normalement.
- **(c) Statut épistémique & régime :**
  - **Prouvé** — S9 (`capability-isolation` / SEF-3), S30 (`wasm-adversarial-trap`). **Régime R1.**
  - **Limite honnête :** isolation **par capabilities, vérifiée à la frontière, non contournable par l'agent** — primitive *substrat*. Verdict **Linux** ; non transférable à seL4 sans re-validation W^X matérielle (D7). On ne dit pas « isolation totale ».

---

## Scènes additionnelles — sélecteur `--scene`

> Le démonstrateur expose quatre scènes : `--scene effects` (les quatre temps forts ci-dessus,
> défaut), `mission-resume`, `incident`, `swarm`. Chacune affiche son **régime** en permanence
> à l'écran. **Toutes substrat Linux** — les verdicts ne transfèrent pas à seL4 (D7).

### Scène `--scene mission-resume` — un agent accomplit une mission

Un agent exécute une tâche en 4 étapes. À l'étape 3, **interruption simulée** : on efface sa
mémoire vive. Au lieu de repartir de zéro, le système relit les étapes déjà committées dans le
log causal et reprend à l'étape 3 — **sans redemander au LLM** le travail déjà fait. Le compteur
d'inférences ne bouge pas sur les étapes relues : la preuve est à l'écran.

- **(a) Caption investisseur :** « Chaque appel à une IA se paie. Ici l'agent travaille, on
  l'interrompt, on efface sa mémoire vive — et à la reprise il ne refait pas ce qui était déjà
  fait, parce que le système en gardait la trace. Le compteur d'appels au modèle ne rebouge pas
  sur les étapes déjà passées. »
- **(b) Drill technique `[d]` :** chaque étape est une `ActionResult` content-addressed dans le
  log. À la reprise, le runner relit ces entrées (`read_action_results`) et les réinjecte comme
  contexte ; le compteur d'inférences ne bouge pas sur les étapes relues.
- **(c) Statut épistémique & régime :**
  - **Prouvé** — relecture autoritaire des résultats émis (P3) ; mécanisme S13 (persistance).
    **Régime R1.**
  - **Limite honnête (à dire à voix haute) :** l'interruption est **SIMULÉE** (on relit le log,
    on ne tue pas le process) → c'est **P3 (traçabilité)**, **PAS P6 (atomicité crash)** ni de la
    durabilité — aucune perte de page cache n'est testée ici. Le log est la **source de vérité des
    résultats émis** ; l'état *autoritaire* reste le **ContentStore** (ADR-0027), pas le log.

### Scène `--scene incident` — triage multi-agent (fan-out / fan-in)

Un incident de production présente trois symptômes simultanés. Trois spécialistes (infra, BDD,
sécurité) l'analysent **en parallèle** (fan-out) ; un agrégateur synthétise leurs trois analyses
en un rapport final (fan-in). Le **DAG causal cross-agent** se construit en direct.

- **(a) Caption investisseur :** « Trois IA décortiquent le même incident en parallèle, chacune
  sous son angle ; une quatrième recoud le tout. Quand le rapport final dit quelque chose, on
  remonte d'un clic à l'analyse exacte qui l'a motivé — et à l'IA qui l'a produite. Qui a dit
  quoi, et sur quelle base : c'est dans l'historique, attribué et rejouable. »
- **(b) Drill technique `[d]` :** le rapport final porte les `action_id` des trois analyses
  comme parents (`Message::caused`). Le DAG `incident → [infra, db, sécurité] → rapport` est
  matérialisé par des hashes, pas des commentaires.
- **(c) Statut épistémique & régime :**
  - **Prouvé** — DAG causal cross-agent content-addressed ; mécanisme `incident_runner`. **Régime R1.**
  - **Limite honnête :** **B-light mono-tenant (ADR-0036)** — les liens causaux sont vérifiés en
    *existence* (O(1)), **pas** protégés par capability cross-agent. *tamper-evident ≠ tamper-proof.*
    *citer un `action_id` ≠ comprendre sémantiquement.* Le visuel fan-out/fan-in est un DAG
    d'attribution, **pas** un protocole de consensus garanti.

### Scène `--scene swarm` — ordonnancement borné + densité (MÉCANISME, pas une mesure)

Un essaim d'agents arrive. Acte 1 : l'**admission** est bornée (file C2) — au plus *cap* agents
en vol, le surplus attend, rien n'est perdu ni affamé. Acte 2 : la **densité** — `[e]` évince un
agent (il sort de la mémoire active → *dormant*), `[w]` le réveille (il reprend depuis son dernier
snapshot). Tous les chiffres affichés sont des **compteurs réels** (`in_flight`, `dormant_count`).

- **(a) Caption investisseur :** « Deux leviers, sous vos yeux. Quand trop d'agents arrivent
  d'un coup, ils font la queue au lieu de noyer la machine — aucun n'est perdu, aucun n'est oublié.
  Et un agent qui ne fait rien est mis en sommeil : il libère la mémoire vive, puis reprend
  exactement où il s'était arrêté quand on le réveille. Les chiffres à l'écran sont des compteurs
  réels — mais c'est le mécanisme qu'on montre ici, pas une mesure de capacité. »
- **(b) Drill technique `[d]` :** Acte 1 = `IoAdmissionQueue` (sémaphore, `in_flight ≤ cap`
  garanti). Acte 2 = `Scheduler::evict_agent` / `wake_agent` (S11/S12), reprise depuis
  `last_snapshot`. Backend **simulé** (`SleepyBackend`).
- **(c) Statut épistémique & régime :**
  - **Mécanisme démontré** — bornes dures C1/C2 (ADR-0022/0023/0030) + evict/wake (ADR-0031).
    Régime affiché : **« mécanisme d'ordonnancement (R2 non mesuré — backend simulé) »**.
  - **Limite honnête (en dur à l'écran) :** **MÉCANISME, pas une mesure.** *N à l'écran ≠ N
    soutenables.* Densité **hébergée** (dormants) et densité **active** (~70 simultanés, cap
    **14 agents/s**, `spec/07 §3.3`) sont **deux métriques distinctes, NON mesurées ici**. **Aucun
    ~100 agents/s** (projection hardware non qualifiée). Aucune arithmétique RAM (le « 50 MB/agent »
    est un objectif de design, pas une mesure).

### Walkthrough seL4 — isolation forte matérielle (hors TUI)

Script séparé `poc/sel4-hello/demo-isolation.sh` : build + boot QEMU AArch64 d'un jalon seL4
montrant ce que **Linux ne peut pas garantir** (red team B). Pas une TUI — un walkthrough scripté
avec transcript rejouable (`docs/demo/sel4-transcripts/`).

- **(a) Caption investisseur :** « Sur Linux, le bac à sable de nos agents est gardé par du
  logiciel : si une faille le perce, tout le processus est exposé. Sur seL4 — un micronoyau dont
  le cœur est prouvé mathématiquement — la séparation est imposée par les page tables du matériel.
  Un agent qui tente d'écrire là où il n'a pas le droit ne reçoit pas un avertissement : il est
  fauché par le processeur. Voici ce barrage, à l'image, sur la cible réelle (QEMU AArch64). »
- **(b) Drill technique :** **W^X matériel (C.10)** — un agent tente d'écrire sur une page de code
  exécutable (RX) ; le résultat n'est pas un avertissement logiciel mais un **`vm fault` du noyau
  seL4** (`vm fault on data at address 0x4` → `C10_NEG_PASS`). Les page tables matérielles, dont
  les capabilities seL4 ne sont pas révocables depuis le domaine agent, rendent l'écriture
  impossible. (Optionnel : **I4 non-interférence C.7** — une évasion WASM d'un agent ne touche pas
  le VSpace d'un autre.)
- **(c) Statut épistémique & régime :**
  - **Prouvé sur QEMU AArch64** — W^X matériel (C.10), arc C.1→C.11-prov (ADR-0047/0049).
  - **Limite honnête :** verdict **d'isolation**, **pas de performance** — la **latence est non
    recevable sur QEMU** (ADR-0046) ; D-P3a (média réel) reste bloqué infrastructure. seL4
    n'élimine pas les bugs Wasmtime, il **borne leur rayon d'impact** au VSpace de l'agent touché.

---

## Clôture (une phrase, sans survente)

> « Ce système ne rend pas vos IA plus intelligentes — il rend leurs **effets** maîtrisables : un historique infalsifiable, une annulation propre, une isolation que l'agent ne peut pas contourner. Sur ce qui sort d'un agent, vous reprenez la main. »

---

## Checklist anti-survente — 5 phrases INTERDITES

| Interdit de prononcer | Reformulation honnête |
|---|---|
| « L'OS garantit les six propriétés. » | « Cette démo montre le **régime R1** : contrôle des effets (P2/P3/P4/P6). Densité, pool, déterminisme (R2) ne sont pas dans cette démo. » |
| « Notre système rend vos agents IA fiables / intelligents. » | « Le système contrôle les **effets**, pas la qualité sémantique des décisions. Un LLM qui décide mal *dans son périmètre de caps* n'est pas un échec du système (frontière LLM = non-objectif). » |
| « L'historique causal est impossible à falsifier. » | « L'historique est **tamper-evident** : une falsification est *détectée* (chaîne aval cassée), pas matériellement *empêchée*. » |
| « Le lien cross-agent prouve que le juge a compris le rapport. » | « Le lien prouve que le juge a **cité ce rapport exact** (B-light, S32). Le lien sémantique fort (B-fort, multi-tenant) est **conçu, non livré**. » |
| « Isolation totale, et c'est vrai partout. » | « Isolation **par capabilities, vérifiée à la frontière WASM, non contournable par l'agent** (S9/S30). Verdict **Linux**, non transférable à seL4 (D7). » |
| (mission-resume) « L'agent **survit au crash** / c'est durable. » | « L'interruption est **simulée** (relecture du log). C'est **P3 (traçabilité)**, pas **P6** ni durabilité. L'état autoritaire reste le ContentStore (ADR-0027). » |
| (swarm) « On héberge des millions d'agents / ~100 agents/s. » | « Cette scène illustre le **mécanisme** d'ordonnancement (admission/inférence bornées, evict/wake). Elle **ne mesure aucune densité**. N à l'écran ≠ N soutenables ; ~100 agents/s = projection hardware **non validée** (spec/07). » |
| (swarm) « Voilà notre densité prouvée. » | « Backend **simulé** : compteurs réels (in_flight, dormant_count) prouvent les **bornes** C1/C2 et l'evict/wake — pas une densité. Densité hébergée vs active = métriques distinctes. » |
| (seL4) « seL4 élimine les failles de notre sandbox. » | « seL4 **borne le rayon d'impact** d'une faille Wasmtime au VSpace de l'agent. Verdict d'**isolation** sur QEMU ; **latence non recevable** (ADR-0046). » |

---

### Index des preuves citées
- Substrat démo : `poc/runtime/src/bin/code_review_runner.rs`, `poc/agent-sdk/examples/{code_reviewer,severity_judge}.rs`
- Content-addressing / tamper-evidence : `poc/causal-log/src/lib.rs:188` (`action_id`), `:169` (`LogEntry`)
- Rejeu : `poc/scenarios/S2-self-rollback-incoherence/reference_responses.jsonl`
- T1 : S27 (`emit-contract`), S2 (`self-rollback-incoherence`)
- T2 : SEF-13 (`causality-adversarial`), S32 (`causal-forgery-b-light` — limite B-light)
- T3 : S7 (`rollback-equivalence` / SEF-2), SEF-12 (`rollback-adversarial`)
- T4 : S9 (`capability-isolation` / SEF-3), S30 (`wasm-adversarial-trap`)
- Scène mission-resume : `poc/runtime/src/bin/long_task_runner.rs`, `poc/agent-sdk/examples/task_step.rs`, S13 (`persistence-restart` / SEF-1)
- Scène incident : `poc/runtime/src/bin/incident_runner.rs`, `poc/agent-sdk/examples/incident_aggregator.rs`, ADR-0036 (B-light multi-cause)
- Scène swarm : `poc/runtime/src/io_queue.rs` (C2), `poc/runtime/src/inference/mod.rs` (C1), `poc/runtime/src/scheduler.rs` (evict/wake S11/S12), spec/07 §3.3 (cap 14 agents/s)
- Walkthrough seL4 : `poc/sel4-hello/c10-wx/` (W^X C.10, ADR-0047), `poc/sel4-hello/c7-crash/` (I4 C.7), `docs/demo/sel4-transcripts/`
