# TODO — OS pour IA

---

## Chantier multi-tenant + B-fort (ouvert 2026-06-07)

**PoC d'apprentissage assumé** (décision utilisateur — aucun besoin métier). But avoué :
créer la configuration où B-light (ADR-0036) est démontrablement insuffisant, pour rendre
B-fort instruisible/testable. Ordre validé par architect : **builder ✓ → multi-tenant → B-fort**.

- [x] **Builder `ActorInstanceBuilder`** *(2026-06-07)* : préalable structurel (point d'entrée
  du futur `.tenant()`). Voir section dédiée ci-dessous + [[L125]].
- [x] **MT-0 — ADR-0057 forme du multi-tenant** *(2026-06-07 — verdict architect)* : CausalLog +
  ContentStore **partagés**, CapabilityStore **isolé** par tenant ; `TenantId` sur `AgentState` +
  `.tenant()` builder (default mono-tenant) ; arme le trigger ADR-0036 §66. Invariant MT-1 =
  INV-A (cap cross-tenant refusée) ∧ INV-B (forgerie causale cross-tenant **réussit** sous B-light
  → oracle inversé de B-fort). Dettes tracées : canal dédup ContentStore, pas de quota I/O/tenant,
  Scheduler tenant-blind. Amende ADR-0013 (trigger décomposition D2 différé). `decisions/0057-*`.
- [x] **MT-1 — `TenantId` + ≥2 tenants** *(2026-06-07)* : cœur — type `TenantId` (default
  sentinelle, zéro régression mono-tenant), champ `tenant` dans `AgentState`, setter `.tenant()`
  sur le builder (+ param dans l'inner-builder), accesseur `ActorInstance::tenant()`,
  `Scheduler` indexe le tenant à `register` (`tenants` map + `tenant_of`, nettoyé au `reap`) —
  **indexé sans être utilisé pour la politique** (tenant-blind, ADR-0057 §D5). Invariants en
  **tests lib** : `INV-MT1-A` (cap d'un tenant non résoluble cross-tenant malgré log+store
  partagés) + `INV-MT1-B` (forgerie causale cross-tenant **réussit** sous B-light — **oracle
  inversé de B-fort**, devra échouer à BF-1). INV-C (sandbox) acquise, non re-testée.
  **120/120 tests lib PASS** (118 + 2), bins OK. *Use case `multitenant_runner.rs` (démo live) :
  optionnel, non fait — les invariants sont déjà couverts par les tests.*
- [x] **BF-0 — ADR-0058 modèle B-fort** *(2026-06-07 — verdict architect)* : `CauseHandle`
  **object-capability sur action_id** (pas agent_id, D1) ; registre `CauseHandleStore` **dédié**
  (≠ CapabilityStore, isolé par tenant, D3) ; **modèle capability pur, pas ACL** → `LogEntry`
  inchangé (option `tenant_id` rejetée, casserait l'action_id content-addressed, D2) ; ABI WASM
  changée `agent_add_cause(handle_id)` (`-1` disparaît, `-3` élargi, D8) ; `Message.cause` →
  `Option<CauseHandle>` (D4) ; délégation interdite par défaut (D5) ; révocation terminaison
  `revoke_issued_by` (D6) + rollback `revoke_issued_after` (**émis** ≠ détenu, D7) ; modèle
  **uniforme**, mono-tenant = cas dégénéré sans branchement sur DEFAULT (D9) ; auto-citation
  sans handle (D10). Remplace partiellement ADR-0036 (§24-58/§39). `decisions/0058-*`.
- [x] **BF-1 — cœur CauseHandle obligatoire** *(2026-06-07)* : `CauseHandleStore` (clé
  `(grantee, action_id)`, isolé par tenant), champ `cause_handle_store` (`AgentState`), setter
  builder, `mint`, **dispatch** dans `agent_add_cause` (auto-citation `entry.agent_id==caller`
  sinon check handle). **Amendement R1** (2 trous découverts à l'impl, verdict architect) :
  **ABI inchangée** (pas de breaking change SDK/WAT), **Message.cause inchangé** (canal TCB, zéro
  churn sur 46 sites), **pas de cache local** (risque n°1 clos), `-1` revient, `-3` élargi.
  Tests : `inv_mt1_b` **inversé** (T2 sans handle → -3, pas d'arête) + miroir `bf1_cross_tenant…`
  (avec handle → succès) + `bf1_self_citation…` (§D10) + **s32 fermé** (forgerie A-5/UC-20 refusée
  même mono-tenant) + s18/s20/adr0003 migrés (mint explicite). **122/122 tests lib PASS**, bins OK.
  ADR-0058 amendé (R1). Voir [[L126]].
- [x] **BF-2 — cycle de vie handle** *(2026-06-07)* : révocation à terminaison (D6) via **garde
  `Drop`** dans `run_loop` (`revoke_issued_by`, couvre break/return/panic) + au rollback (D7) via
  `revoke_issued_after` greffé à côté de `revoke_owned_after` (caps) dans `Message::Rollback`.
  Tests `bf2_revoke_on_issuer_termination` + `bf2_revoke_on_issuer_rollback` **bout-en-bout via
  vrai appel WASM** `agent_add_cause` avant/après révocation (risque n°1 respecté). Limite tracée :
  révocation par émetteur portée sur le store de SON tenant (handle déposé dans un autre tenant
  non atteint). **124/124 tests lib PASS**. Voir [[L127]] (piège `entries_by_agent` ordre hash).
- [x] **BF-3 — robustesse adversariale** *(2026-06-07)* : 3 tests attaquant les 3 liaisons de la
  capability — `bf3_handle_for_other_grantee_rejected` (liaison grantee), `…_for_other_action…`
  (liaison action_id), `…_in_wrong_tenant_store…` (liaison tenant-store) → tous -3 via vrai appel
  WASM. SEF-7.1 (action forgée → -3) et SEF-7.2 (flood MAX_EXTRA_CAUSES → -2) **restent valides
  sous B-fort** (vérifs avant/indépendantes du check handle). Re-délégation (D5) : aucune API de
  délégation sur `CauseHandleStore` (impossible par construction). **Flood store : pas de plafond**
  — décision tracée : `mint` est trusted-only, aucun vecteur guest (YAGNI ; trigger = mint
  guest-reachable). **127/127 tests lib PASS**. B-fort complet.

### Prolongement — décomposition Scheduler + révocation cross-tenant (2026-06-07, verdict architect)

Deux chantiers jumeaux fermant les dettes ADR-0057 §D5 (Scheduler tenant-blind) et ADR-0058
§D6/D7 (révocation scopée au tenant). Ordre validé : **SD-0 → SD-1 → SD-2 → XR-0 → XR-1**.

- [x] **SD-0 — armer le trigger** *(2026-06-07)* : test de supervision cross-tenant
  (`inv_sd_auth_cross_tenant_supervision_*`) — condition ADR-0013 §D2 / ADR-0057 §D5 (« décomposition
  obligatoire dès que la supervision cross-tenant devient un cas *testé* »). Au passage, fix d'un
  flake L127 pré-existant masqué par le baseline (`bf1_self_citation` `.last()` sur agent à 2 actions).
- [x] **SD-1 — extraire `Registry`** *(2026-06-07)* : mécanisme (annuaire/routage/dormant) sorti du
  `Scheduler`, qui devient façade déléguant. INV-SD-NOREG (bins/tests verts).
- [x] **SD-2 — `Supervisor` + `SupervisionAuthority`** *(2026-06-07 — ADR-0059)* : politique extraite ;
  autorité capability-style `{Orchestrator, Tenant(t)}` ; supervision cross-tenant sous `Tenant(t)`
  refusée (`CrossTenantDenied`, aucun effet). Audit = O1 (Err typé, pas d'EmitType ; condition
  O1→O2 tracée). INV-SD-AUTH + INV-SD-INTRA. ADR-0013 §D2 / ADR-0057 §D5 amendés (dette close).
- [x] **XR-0 — `CauseHandleRegistry`** *(2026-06-07)* : registre `TenantId → store` ; store local
  DÉRIVÉ via `get_or_create` (unique point d'insertion → risque n°1 clos) ; 12 sites de test migrés.
- [x] **XR-1 — révocation cross-tenant** *(2026-06-07 — ADR-0060)* : drop-guard (terminaison) + rollback
  balaient TOUS les stores du registre (`revoke_issued_by_all` / `revoke_issued_after_all`). INV-XR-CROSS
  + INV-XR-ROLLBACK (vrai appel WASM) ; INV-XR-INTRA couvert par bf2. ADR-0058 §D6/D7 amendés.
  **131/131 tests lib PASS.** Voir [L128](lab/LESSONS.md). Limite éviction/réveil tracée (ADR-0060, FutureWork).

### Revue sécurité du runtime + remédiation (2026-06-07, via agent architect)

Revue design+sécurité complète du runtime (host fns, modèles d'autorité, frontières TCB,
concurrence). 2 findings CRITIQUES, 4 MAJEURS, validations négatives sur le cœur B-fort /
SupervisionAuthority / dérivation registre. Tous traités.

- [x] **C2 — locks d'autorité anti-poison** *(2026-06-07)* : un panic tenant un Mutex partagé
  empoisonnait → DoS cross-tenant (voire abort() via Drop guard). `lock_or_recover` partout.
  Voir [L129](lab/LESSONS.md).
- [x] **M3 — purge des `.last()` flaky** *(2026-06-07)* : dette L127 résiduelle (inv_mt1_b, bf1, bf2).
- [x] **M4 — H-cb-correct source unique** *(2026-06-07 — ADR-0010 amendé)* : `pending_commit` unique,
  suppression de `barrier_fired` (se désynchronisait sur barrier+request_validation).
- [x] **M2 — terminaison absorbante** *(2026-06-07 — ADR-0014 amendé)* : `Terminated` absorbant +
  drapeau `termination_requested` gardant les 6 host fns mutantes (anti effets post-mortem).
- [x] **C1+M1 — KV référent partagé + garde câblage** *(2026-06-07 — ADR-0061)* : kv_store
  `Arc<Mutex>` partagé-par-tenant (P4 réel) ; `register` refuse un cap_store partagé entre tenants
  distincts. Voir [L130](lab/LESSONS.md). **137/137 tests lib PASS.**

Résiduels mineurs tracés (non bloquants) : oracle de dédup ContentStore (ADR-0057 §D3, connu) ;
refus de supervision non journalisé (ADR-0059 O1, condition de bascule tracée) ; KV non persistant
à l'éviction (ADR-0061, FutureWork) ; garde M1 sur cap_store seulement (KV default per-agent).

---

## Extensibilité — how-to + RFC flotte déclarative (ouvert 2026-06-06)

Demande utilisateur avancé : « créer un agent / une flotte spécifique : comment, où ? ».
Cadrage architect : la « flotte déclarative » n'est pas tranchable (le routage dynamique
résiste au déclaratif) → RFC, pas ADR. Périmètre : documentation seulement (aucun cœur touché).

- [x] **How-to opérationnel** *(2026-06-06)* : `docs/guides/howto-agent-flotte.md` (rédaction
  andragogue). Deux niveaux (agent WASM / runner), commandes copier-coller **vérifiées en exécution**
  (echo.wasm build ; incident-runner build+run exit 0, fan-out/fan-in R1), frontière épistémique
  substrat-vs-responsabilité (phrase-pivot architect), renvoi RFC-0001. Cross-link RUNBOOK.
- [x] **RFC-0001 flotte déclarative (DRAFT)** *(2026-06-06)* : nouveau dossier `docs/design/` +
  `README` (genre RFC≠ADR : exploré/réfutable vs tranché/contraignant). `0001-flotte-declarative.md` :
  problème (qui compose/droit de recompiler), 5 préoccupations d'un runner dont le routage dynamique
  non-déclaratif (point 4), descripteur périmètre 1-3 + trou typé `router`, **4 invariants en tests
  d'acceptation** (risque n°1 = ADR-0036 causalité fabriquée au boot), alternatives, critères DRAFT→ADR.
- [x] **`ActorInstanceBuilder`** *(2026-06-07)* : builder unifiant les 8 constructeurs
  `new_precompiled_*` (`poc/runtime/src/actor.rs`). Constat refactor : **aucune combinaison
  invalide** (l'inner-builder accepte tout le produit cartésien) → pas de fail-closed à prévoir,
  `build()` ne faillit que sur compilation/instanciation → **pas d'ADR** (pur mécanisme, critère
  architect respecté). Stratégie : builder canonique + 8 constructeurs conservés en wrappers fins
  (zéro des 197 sites cassé) ; 2 inner-builders intermédiaires supprimés, `build_instance_inner_with_profile_and_clock`
  passé `pub(crate)` (1 site externe `watchdog_runner` migré). 118/118 tests lib PASS. Point d'entrée
  prévu pour `.tenant()` (MT-1). Voir [[L125]]. *Reste optionnel : migrer les 197 sites legacy +
  déprécier les wrappers — non tiré, à la demande.*
- [x] **§7.2 — relevé exhaustif des runners** *(2026-06-07 — RFC §6 bis, validé architect)* : lecture
  des 51 binaires. **Surface surestimée 2.5×** : ~17 non-flottes (outils/vérificateurs), ~12 mono-agent,
  **~18-21 vraies flottes** + 3 harnais SEF. **8 familles de routage exhaustives**, toutes réductibles
  à un **noyau fini de 5 primitives + 1 callback `on_event`** (style OTP gen_server) → le DSL (alt. d)
  reste rejeté, le mode d'échec §6 (« casser au 2ᵉ cas ») n'a pas lieu. 4 garde-fous tranchés : `ActionId`
  opaque non-forgeable (clôt risque n°1 ADR-0036 au niveau type), `Spawn` = 5ᵉ primitive (famille 4 seule),
  `Ctx` expose le set attendu (quorum), éviction = câblage pas `Route`. **« Composer sans recompiler »
  = propriété BORNÉE** (vrai familles 1/2/3/5/6 via Routers génériques ; faux famille 4 = Rust assumé).
- [x] **§7.4 — ADR builder tranché** *(2026-06-07 — ADR-0062)* : constat que `build()` est **déjà
  l'unique chemin** (les 10 façades `new_precompiled_*` + 2 `restore_*` y délèguent — la dette structurelle
  était déjà payée, cf. note builder 2026-06-07). ADR-0062 acte le builder canonique, **gèle** les façades
  (migration ~189 sites **rejetée** : valeur architecturale nulle ; interdiction 11ᵉ façade), régularise
  `restore_*` (passe explicitement par le builder ; réveil = runtime hors loader), et **prescrit le contrat
  loader D4** (`from_spec` data-driven + `wasm_hash` CAS amont + caps **fail-closed**). D4 non implémenté =
  matière de §7.3. Voir [[L131]].
- [x] **§7.1 — destinataire : NON CONFIRMÉ → RFC-0001 ABANDONNÉE** *(2026-06-07 — verdict architect
  suivi)* : la propriété qui justifiait la RFC est **P-forte** (composer une flotte *arbitraire* sans
  recompiler), pas **P-faible** (instancier un Router pré-livré par config). §6 bis démontre P-faible
  seulement — les familles 1/2/3/5/6 sont les cas *dégénérés* où le routage est trivial ; la **famille 4**
  (topologie pilotée par le contenu d'un emit LLM) reste en Rust et réfute P-forte. L'hybride = compromis
  qui ne sert personne. Décision : **alternative (a)** (how-to). RFC §7/§8 + en-tête passés ABANDONNÉE.
- [x] **§7.3 — prototype loader : SANS OBJET** *(clôture)* : non poursuivi. Note architect : les 4 tests
  §4 testent le câblage substrat, pas l'expressivité du routage → un prototype aurait pu les passer en
  restant inutilisable (fausse validation). Aucun nouvel ADR (RFC abandonnée n'en engendre pas).
### Bibliothèque de Routers génériques (ouvert 2026-06-07 — ADR-0063)

Livrable P-faible sauvé de RFC-0001 §6 bis. Module `poc/runtime/src/fleet/`. Ordre validé par
architect : **ADR → driver (le vrai livrable, §0) → 2 Routers → test frontière**.

- [x] **Incrément 1 — `FleetDriver` + trait `Router` + `FanInRouter`/`QuorumRouter`** *(2026-06-07 —
  ADR-0063, 141/141 tests lib PASS)* : driver **référence** le Scheduler (jamais ne le possède),
  surface mécanisme uniquement (`register`/`send`/`tenant_of`), jamais politique. Causalité via
  **canal TCB `Message::caused`**, **aucun `CauseHandle` minté** (il serait du code mort — le check
  handle n'existe que dans le chemin guest `agent_add_cause`, non sollicité par le modèle Router ;
  voir [[L133]]). **Mono-tenant strict** : garde `tenant_of` = seule frontière inter-tenant
  (analogue `Supervisor::authorize`). Test `inv_router_mono_tenant_no_cross_fanin` (oracle inversé +
  **miroir positif** obligatoire, vrai cycle WASM) + 2 tests purs de routage + garde de provenance.
  Constat majeur : le choix B-fort/B-light est **neutre** pour le code du driver (ADR-0063 §D3 ter).
- [x] **Incrément 2a — `FanInRouter` (label par membre) + `wait_result` + refacto `incident_runner`
  vérifié sous Ollama** *(2026-06-07)* : `FanInRouter::new(agg, cause, labels)` émet
  `REPORT:<label>:<text>` (format = mécanisme révisable, ADR-0063 §D6) — résout le constat de cadrage
  (l'agrégateur code en dur `REPORT:infra:`/`db:`/`security:`). `FleetDriver::wait_result` ajouté
  (tue le `wait_action_result` bespoke). `incident_runner` refactoré : `Scheduler` + `FleetDriver` +
  `FanInRouter`, **plus aucun `tokio::spawn(run_loop)` ni poll manuel**. **Run réel sous Ollama
  PASS** : DAG `incident → [infra, db, security] → rapport` produit, rapport cohérent. 141/141 tests
  lib PASS (test fan-in mis à jour). Différence assumée (à raffiner en 2b) : pas d'agrégation
  PARTIELLE sur deadline.
- [x] **Incrément 2b — complétion partielle sur deadline + refacto `consensus_runner`** *(2026-06-07)* :
  le `FleetDriver::run` émet `FleetEvent::Deadline` aux membres **muets** au deadline de collecte ;
  `FanInRouter`/`QuorumRouter` finalisent partiellement (`finalize()`/`tally()`, idempotents) —
  restaure le comportement partiel-sur-timeout des runners pré-fleet. `consensus_runner` refactoré
  (`Scheduler`+`FleetDriver`+`QuorumRouter`, plus aucun `spawn(run_loop)`/poll manuel ;
  `QuorumRouter` compatible `tally_secretary` via `VOTE:<text>`/`starts_with`). **Runs réels sous
  Ollama PASS** : incident → rapport ; consensus 3 votants → `DECISION:REJECTED … N:3`, recompute
  cohérent. 7/7 tests fleet (3 nouveaux : partiel fan-in/quorum + no-op sans résultat), lib verte.
- [x] **Incrément 2c — `PipelineRouter`/`RefineRouter`/`SuperviseRouter` + `await_event` + how-to**
  *(2026-06-07)* : 3 Routers (familles 1/5/6) ; `FleetEvent::Emit` + `FleetDriver::watch_emit` +
  remontée dans `poll_events` réalisent la primitive **`await_event`** (§6 bis) sur les emits typés
  (≠ ActionResult) pour la supervision. Le *transform de payload applicatif* (motif récurrent) est
  matérialisé par des `fn` : `PipelineRouter::with_transform(fn(usize,&str)->Vec<u8>)` (prompt
  par-saut) et `RefineRouter` `accepted: fn(&str)->bool` (convergence). How-to :
  `docs/guides/howto-fleet-routers.md` (commandes copier-coller, frontière P-faible explicite).
  **149/149 tests lib PASS** (12 fleet, dont 5 nouveaux : pipeline forward/transform, refine
  loop/max_iter, supervise emit typé). Frontière assumée tracée dans le how-to : REPL interactifs
  (`chain`/`pipeline`) et draft↔critique 2-agents (`iterative`) restent bespoke ; famille 4 (routage
  par contenu d'emit LLM) hors scope. **Bibliothèque de Routers complète pour les familles 1/2/3/5/6.**
- [ ] **Reliquats fleet** *(non engagés)* : 5ᵉ primitive `spawn` (famille 4) ; optim curseur
  incrémental sur le log (ADR-0063 D7, poll O(N×M) actuel) ; closure de formatage unifiée (vs `fn`).
- [ ] **Driver de flotte cross-tenant — DORMANT, ne pas instruire** *(ADR-0063 §D4)* : le driver
  est mono-tenant strict. **Trigger objectif de réveil** : première PR introduisant une flotte dont
  les membres portent ≥2 `TenantId` distincts. Au réveil seulement : exiger un témoin
  `SupervisionAuthority::Orchestrator` (jamais inféré, gabarit ADR-0059 §D3) + audit via `0x15`
  (pas `0x14`, payload figé inadapté). Ne pas pré-câbler de chemin cross-tenant « au cas où » : YAGNI.
- [ ] **RFC future — routage piloté par contenu LLM sans Router Rust** *(famille 4, conditionnel
  priorité)* : le seul problème dur isolé par la clôture RFC-0001. Réveillerait D4 d'ADR-0062. Non ouvert.

---

## Chantier démos — couverture complète (ouvert 2026-06-06)

Combler les trois manques de démo identifiés en revue d'état : aucune démo « agent qui
accomplit une mission », aucun multi-agent vivant, seL4 absent du narratif. Chaque brique :
code exécutable + script + narratif + md. Périmètre **use case** (aucun cœur touché). Verdict
architect intégré (revendications/régimes par scène). Branche `chantier-demos`.

- [x] **Lot A — Scènes `mission-resume` + `incident`** *(2026-06-06)* : sélecteur `--scene` dans
  `demo_tui.rs`. `mission-resume` (réutilise `long_task_runner`) : tâche 4 étapes, interruption
  *simulée*, reprise sans recompute — **P3 (traçabilité), PAS P1a ni P6** (correction architect ;
  cf. [[L124]]). `incident` (réutilise `incident_runner`) : fan-out 3 → fan-in, DAG B-light
  (ADR-0036). `SeqBackend` (file rejeu par agent). Validé sous pty. Use case.
- [x] **Lot B — Scène `swarm`** *(2026-06-06)* : admission bornée C2 (`IoAdmissionQueue`,
  in-flight ≤ cap garanti sémaphore) + éviction/réveil densité (`Scheduler` evict/wake, S11/S12).
  Compteurs réels uniquement. Garde-fous architecte en dur (R2 non mesuré, N≠soutenables, aucun
  ~100 agents/s). Aucun accesseur cœur ajouté → use case. Validé sous pty (C2≤4 ; 3 évictions → 3 dormants).
- [x] **Lot C — seL4 jouable** *(2026-06-06)* : `poc/sel4-hello/demo-isolation.sh` (build+boot
  QEMU). W^X matériel C.10 : écriture page RX → `vm fault` seL4 (`C10_NEG_PASS`). Transcript réel
  capturé `docs/demo/sel4-transcripts/c10-wx-phaseA.txt` (`make test` exit 0). Garde-fous : verdict
  d'isolation pas de perf (latence non recevable QEMU, ADR-0046) ; D7. Artefacts build nettoyés.
- [x] **Lot D — Narratif + md** *(2026-06-06)* : `docs/demo/demo-tui-script.md` (sections par scène,
  format accroche/drill/régime + checklist anti-survente étendue), `docs/demo/demo-tui-guide.md`
  (commandes `--scene` + touches par scène), TODO + [[L124]]. Revue marketing/andragogue = suivi
  recommandé (non bloquant).

---

## Démonstrateur live + agents de rôle (ouvert 2026-06-06)

Démonstrateur TUI présentable (public mixte) + capitalisation de deux compétences en agents de référence. Périmètre **use case** : aucun fichier du cœur runtime touché.

- [x] **Agents de rôle `andragogue` + `marketing`** *(2026-06-06)* : deux agents de rôle, câblés sur les garde-fous épistémiques (régimes R1/R2, substrat vs applicative, frontière LLM, F1/L68 — jamais survendre ni simplifier au prix de la fausseté). Livrables produits : `docs/demo/demo-tui-script.md` (script + checklist anti-survente, marketing) et `docs/onboarding-parcours.md` (parcours 6 étapes, andragogue).
- [x] **Démonstrateur TUI `demo-tui` (lots 1–3)** *(2026-06-06)* : `poc/runtime/src/bin/demo_tui.rs` (feature `demo-tui`, ratatui). Lot 1 : pipeline reviewer→judge en rejeu (`CannedBackend` keyé par agent_id), DAG causal live depuis le vrai log. Lot 2 : `[t]` falsification tamper-evident (recalcul `action_id` réel → stored≠recalc + juge orphelin), `[r]` rollback P2 (`Message::Rollback` sur `multi_turn` vivant), `[x]` intrus P4 (`data_accessor`, cap `reports/` → `confidential/` refusé, 0x14). Lot 3 : `[d]` couche preuve (hashes complets + `EmitEnvelope` décodé), `--live` (enum `DemoBackend` → Ollama). Vérifié par drive pseudo-TTY (L122).
- [x] **`--live` exercé (Ollama réel)** *(2026-06-06)* : démo confirmée live (texte LLM anglais généré, en-tête `mode: LIVE`). Latence mesurée `llama3.2:3b` sur CPU : ~5 s/appel chaud, ~13 s à froid. Constat clé : seules 2 des 6 interactions touchent le LLM ; `[t]/[r]/[x]` sont identiques rejeu/live (agissent sur log+caps, pas l'inférence) → « live ≈ rejeu » est la **thèse** de la démo (contrôle des effets indépendant du LLM), pas un raté. Guide `docs/demo/demo-tui-guide.md`.
- [x] **Scénario « vérificateur tiers » (falsification crédible, dé-téléphonée)** *(2026-06-06 — verdict architect + agent rocksdb)* : sépare écrivain/attaquant/auditeur en 3 process. **Cœur** `causal-log` : `open_existing` (create_if_missing=false, anti faux-négatif), `iter_default_raw` (mécanisme, pas de `.flatten()`), `corrupt_value_at` (test-utils, `db.put` même-clé + flush). **Runtime** `os_poc_runtime::integrity::verify_content_addressing` (politique : `clé==SHA256(octets bruts)` + parents pendants). **Bins** `log-verify` (exit 0/1/2) + `log-tamper` (feature `demo-tamper`). `demo-tui` → chemin stable `demo-work/` (`.gitignore`). E2E vérifié (32 entrées intègres → corruption → 1 mismatch pointant la bonne `action_id`, exit 1) + test unitaire round-trip `integrity::tests`. Portée P3a R1 ; hors-portée graves documentés (re-keying feuille, bit-rot=CRC RocksDB, troncature). Voir L123, `docs/demo/demo-tui-guide.md §5 bis`.

> ⚠️ Périmètre : les lots 1–3 sont **use case** (aucun cœur touché). Le scénario vérificateur tiers **ajoute du mécanisme au cœur** `causal-log` (3 méthodes, dont 1 test-only) + un module runtime `integrity` — pur mécanisme/politique d'audit, sans ADR (P3a déjà actée S32/SEF-13/ADR-0036, verdict architect).

- [x] **Trois variantes « anti-téléphoné »** *(2026-06-06)* : (1) `demo-tui --code <fichier>` — le reviewer relit le code du public (à coupler `--live` ; en rejeu la conserve ignore le contenu) ; fichier absent → exit 2. (2) `log-tamper --blind` — entrée+octet tirés à l'horloge et **cachés** ; l'auditeur retrouve seul (vérifié : cible cachée `1a934a47…` détectée). (3) `demo-tui --llm-wrong` — rejeu d'un LLM qui rate la faille (juge APPROVE code vulnérable, en-tête `scénario: LLM faillible`, narration « le système ne corrige PAS, il trace/attribue » — frontière LLM = non-objectif). Tous trois pilotés/vérifiés sous pty. Doc `docs/demo/demo-tui-guide.md §5 ter`. Périmètre **use case** (demo_tui.rs + log_tamper.rs uniquement, pas de nouveau cœur).

---

## Phase 13 — Métrique orphelins + décisions différées (ouvert 2026-06-02)

Validation empirique du déclencheur GC (ADR-0055 §D4) et fermeture des débats Pulley/Cranelift et spec.

- [x] **ADR-0055 §D4 — Réserve empirique levée** *(2026-06-02 — PASS)* : `orphan_fabricator` + `orphan_metric_sampler` écrits. Trois runs K∈{0,200,5000} → écart=0 sur store frais. `estimate-num-keys` exact en régime frais. Réserve résiduelle : compaction active (tombstones, L0). Voir `poc/scenarios/orphan-metric/VERDICT.md`. Infrastructure GC (itérateurs D5 + sampler) livrée ; `gc_orphans.rs` sur HOLD (D7 — déclencheur D4 statique+dynamique non atteint).
- [x] **ADR-0056 — Pulley vs Cranelift AOT : différé** *(2026-06-02)* : W^X soldé (C.10), latence neutre (I/O domine), avantage signature conditionnel (PKI non atteinte), migration propre pour C.12+ non instruite. Motif historique ADR-0037 §132 (instabilité) déclaré caduc. Conditions de réveil R1 (second substrat), R2 (PKI/multi-producteur), R3 (JIT réintroduit sur cible). Voir `decisions/0056-pulley-vs-cranelift-aot.md`.
- [x] **spec/09 + spec/04 — Intégration ADR-0055/0056 + Phase 10** *(2026-06-02)* : §5.2 GC orphelins raccordé à ADR-0055 + VERDICT ; §5.4 Pulley ADR-0056 inscrit. H-inférence-coût enrichi (OllamaBackend réel, médiane 12,5 s, p99 18 s, non-transférable GPU). H-densité-active annotée (P10-S3/S5 mécaniques validées, comparaison Docker T6-actif pendante).
- [x] **Bundle exportable (Chantier 4)** *(2026-06-02)* : bundle de 7 fichiers — 00 (tout public), 01 (technique), 02 (use-cases régime-taggés), 03 (red team — stub structuré), 04 (direction seL4), 05 (glossaire), 06 (références). Chantiers 1 et 2 du briefing déjà couverts (catalogue régime-taggé depuis `a092354`, S15 depuis `89dc011`).

**Chantiers ouverts (sans déclencheur hardware ni trigger manquant)** :
- [x] **Chantier 3A — Campagne red team propriétés** *(2026-06-03 — 6/6 PASS ou LIMITE DOCUMENTÉE)* : 6 vecteurs A-1–A-6, oracles Rust déterministes exécutés. A-1/S17 : P2×P4 PASS (cascade révocation depth=2). A-2/S16 : P2×C1 PASS (slot zombie fermé, séquence log correcte). A-3/S28 : P2 PASS (self-rollback post-emit refusé, code -3). A-4/S19 : P6 PASS (orphelin 0x11 détecté, ContentStore intact). A-5/S32 : P3 LIMITE DOCUMENTÉE (B-light mono-tenant, intentionnel, ADR-0036). A-6/S31 : P4-audit LIMITE DOCUMENTÉE (sentinel F2, isolation tenue, borne anti-DoS ADR-0051 §D2). Voir `red-team/campagne-A-proprietes/FINDING-*.md` + `poc/scenarios/S16..S32/VERDICT.md`.
- [x] **Chantier 3B — Campagne red team substrat Linux** *(2026-06-03 ; B-1 corrigé le même jour post-`cargo audit`)* : 6 findings. B-1 (CVE Wasmtime — **15 advisories ACTIFS sur v25** remontés par `cargo audit`, dont 2 critiques CVSS 9.0 sandbox escape ; RUSTSEC-2026-0096 touche la cible aarch64/seL4 ; **la version initiale affirmait à tort « aucun CVE actif v25 » — corrigée par audit live**, structurelle + dette upgrade). B-1b (dépendance morte `wasmtime-wasi` **retirée** : 16→15 advisories, RUSTSEC-2026-0149 éliminé). B-2 (bounds check inconsistant agent_check_cap/agent_add_cause — correctible, **patch appliqué**). B-3 (N agents dans 1 processus Linux — post-évasion = tout le processus, structurelle). B-4 (W^X mprotect() logiciel — revocable par kernel exploit, structurelle). B-5 (TCB Linux ~30 MLOC non prouvé — LPEs classe active, structurelle). 4 limites structurelles → argument seL4 formalisé dans `red-team/campagne-B-substrat/SYNTHESE.md`.

**Déclencheurs dormants (ne pas instruire avant)** :
- `gc_orphans.rs` → D4 statique + dynamique observés sur cycles reopen (ADR-0055 D7)
- Pulley → R1 (second substrat), R2 (PKI/multi-producteur), R3 (JIT sur cible) — ADR-0056
- #7b commit cross-store atomique → suit GC
- D-P3a latence seL4 → board physique
- Power-loss/β → board physique + NVMe passthrough
- **Upgrade wasmtime ≥36.0.7/≥42.0.2/≥43.0.1** → activation `wasm_memory64` (RUSTSEC-2026-0096 N/A par config tant que memory64 off ; garde fail-closed `memory64_reste_desactive` dans `poc/runtime/src/lib.rs`) OU upgrade requis pour autre cause. Pas d'ADR (arbitrage architect 2026-06-03). Voir B-1 / ADR-0049 D3(c). *Résidu medium non N/A : RUSTSEC-2026-0087 (f64x2.splat x86-64) — atténué par convention (agents WAT n'émettent pas ce SIMD), à revérifier si SIMD introduit.*

---

## Retouche documentaire (ouvert 2026-05-30)

Campagne expérimentale close (ADR-0049, ADR-0051). Corpus rendu lisible de l'extérieur. Aucun code, aucun nouveau benchmark, aucune réouverture d'item différé. Audience cible : à valider DOC-5.

- [x] **DOC-1 — Réconcilier thèse ↔ atterrissage (`spec/01-vision.md`)** *(2026-05-30)* : §3.1 état réel de validation par propriété (validé / substrat / scénario) ; marquer P1-quantifié-vs-Linux et D-P3a comme non aboutis + pourquoi. §3.2 : annoter le verdict réel à côté des critères (sans réécrire la cible). §3.3 : corrigé « exercice de spécification, pas de code » → PoC E2E + stack seL4 produits.
- [x] **DOC-2 — Note de revendication honnête (`README.md §Périmètre des revendications`)** *(2026-05-30)* : démontré (P2/P4/P5/P6 + intégration seL4 C.1–C.11) ; hors scope + pourquoi (quantitatif-vs-Linux, D-P3a, power-loss, séparation CAS/index) ; abandon-pour-non-transférabilité cadré comme résultat.
- [x] **DOC-3 — Rafraîchir `README.md`** *(2026-05-30)* : ADR → 0053, L-series → L92, spec → 10 fichiers, phases 10–12 ajoutées, table de statut exacte, spec/02 corrections soldées, INDEX.md ADR-0053 ajouté.
- [x] **DOC-4 — Cohérence transversale** *(2026-05-30)* : aucun doc top-level ne présente D-P3a / quantitatif-vs-Linux comme acquis ou imminent. « Corrections spec/02 à corriger » → soldées. Distinction mesuré/planifié/différé respectée.
- [x] **DOC-5 — Document public `README-PUBLIC.md`** *(2026-05-30)* : audience = décideur / manager technique (connaît l'IA, pas seL4 ni les capabilities) ; format = `README-PUBLIC.md` à la racine, autonome, pointe vers le corpus technique ; langue = anglais. Structure : thèse en une phrase → pourquoi maintenant → ce qui a été construit → ce qui est démontré → honnêteté sur les limites → où aller.

---

## Harness de durabilité — en cours (ouvert 2026-05-30 — ADR-0051 §Amendement)

Séquencement décidé par architect (2026-05-30) : spec/10 → oracle I-CSR pérenne → harness substrat-agnostique. Armement du blocage le plus convergent (#7b / #8 / D-P3a). Voir `spec/10-modele-durabilite.md` et ADR-0051 §Amendement.

- [x] **spec/10 — Modèle de durabilité** *(2026-05-30)* : consolidation ADR-0027/0038/0045/0046/0049/0051 + SEF-10. Tableau D1–D4 × substrat × régime. Invariant I-CSR formalisé. Règle de symétrie fsync. Voir `spec/10-modele-durabilite.md`.
- [x] **Oracle I-CSR pérenne dans `poc/runtime/src/durability.rs`** *(2026-05-30)* : `write_commits()` + `verify_icsr()` + `IcsrWitness` JSON. Vérifie `∀ log_entry ∈ journal : log_entry.snapshot_hash ∈ store` (I-CSR). Distincts : `LogEntryMissing` (admis, no-force), `SnapshotMissing` (violation I-CSR), `DataBlockMissing`. 96/96 tests runtime intacts.
- [x] **Harness substrat-agnostique : `icsr-writer` + `icsr-verifier`** *(2026-05-30)* : deux binaires dans `poc/runtime/src/bin/`. Modes : `drop` (arrêt coopératif, exit=0) et `exit` (SIGKILL simulé, exit=1). Smoke test : 30 commits mode drop PASS + 50 commits mode exit PASS. Modes `drop_caches` / `kill-QEMU` stubbés (déclencheurs spec/10 §6).
- [x] **Run ICSR-drop-caches — verdict I-CSR sous cache froid** *(2026-05-30 — PASS)* : 100 commits, régime SIGKILL + sync + drop_caches sur AMD Ryzen 5 PRO 4650U + WD SN530. `snapshot_missing=0`, `log_missing=0`. Confirme D2 (page cache → disque via WAL) atteint. Note : `sync` avant `drop_caches` garantit le flush des dirty pages — ce test ne matérialise pas la fenêtre cross-store SEF-10 (qui requiert power-loss sans sync). Voir `poc/scenarios/ICSR-drop-caches/VERDICT.md`.

**Déclencheurs dormants (ne pas instruire avant)** :
- `drop_caches` → accès root ou VM → débloque mode 2 du harness + V3.1/V3.2 adversarial P3
- Board physique / NVMe passthrough → débloque mode 3 (kill-QEMU ou power-loss réel) + D-P3a + β-seL4 (#8)
- GC orphelins → déclenche #7b (commit cross-store atomique) + re-séparation CAS/index

---

## Phase 12 — Campagne adversariale P2/P3/P5 (ouvert 2026-05-30 — ADR-0053)

Méthode ADR-0050 : gate de soundness bloquant → axes dans l'ordre P2 ≻ P3 ≻ P5 (ADR-0001). Substrat Linux PoC, garde-fou non-transférabilité. Trois questions bloquantes à résoudre au gate avant tout harness.

- [x] **G — Gate de soundness** *(2026-05-30)* : Q1 : V2.1 tombe (`session_max_actions=10K` → depth=100 → 3,4 ms << 100 ms ; spec ≤100 ms pour depth=100 seulement). Q2 : V3.4 non-constructible par design (append-only + SHA-256 + existence-check = pas de fixed-point). Q3/G-P5 : Branche NON (défaut) — A-P5 clos sans code (ADR-0051 §D5, dette #3 dormante).
- [x] **A-P2 — rollback adversarial** *(2026-05-30 — SEF-12 PASS)* : V2.2 (rollback², jonction nouvelle-branche→chaîne-originale) PASS — P-α₂/β₂/γ₂/δ₁/δ₂ vérifiés. V2.3 (flood immédiat) PASS. V2.4 (liveness, 80 msgs) PASS. V2.1 tombe (gate). Voir `poc/scenarios/SEF-12-rollback-adversarial/VERDICT.md`.
- [x] **A-P3 — traçabilité adversariale (intégrité)** *(2026-05-30 — SEF-13 PASS)* : V3.3a (10 000 forgeries → 0 faux-positif) PASS. V3.3b (1001 entrées, intégrité content-addressed) PASS. V3.4 non-constructible par design — finding positif. Voir `poc/scenarios/SEF-13-causality-adversarial/VERDICT.md`.
  - **V3.1/V3.2 (latence P3 sous concurrence) — FERMÉ sans run** *(2026-05-30)* : root disponible mais résultat prévisible — ADR-0032 D2 établit déjà que p99 dépasse 10 ms sous compaction L0 active ; repopulation 10⁸ (plusieurs heures + 41 GB) pour confirmer un résultat connu sur substrat clos (ADR-0049). Coût > valeur. Note dans TODO suffit : P3a-latence sous write concurrent = borne 10 ms non garantie sous compaction active (ADR-0032). **Non transférable seL4.**
- [x] **A-P5** *(2026-05-30)* : **CLOS sans code** — Branche NON (défaut ADR-0053 §D-P5). La sortie LLM n'entre pas dans `state_bytes` ; P5 tenu trivialement et correctement. Dette #3 dormante inchangée (ADR-0051 §D5).

---

## Phase 11 — Remontée spec (ouvert 2026-05-30)

PoC clos (ADR-0049). Direction post-clôture : intégrer les acquis de la campagne adversariale et du PoC seL4 dans les documents de spec. Aucun nouveau code sans déclencheur explicite.

- [x] **T1 — Évaluer sous-axe B ADR-0052** *(2026-05-30)* : sous-axe B **NON déclenché** — la sortie LLM n'entre pas dans la préimage du hash d'état (`state_bytes = [agent_id|seq|zéros]`) ; le hash de transition reste déterministe ; SEF-6-bis non ouvert. Correction imprécision TODO : `kv_store` n'est pas non plus sérialisé dans `state_bytes` — le vrai déclencheur de #3 est une modification de `commit_barrier` élargissant la préimage, pas le stockage en kv_store. Voir `decisions/0052-scope-phase-10-inference-reelle.md §Clôture sous-axe B`.
- [x] **T2 — Amender spec/02** *(2026-05-30 — fait dans commit f55c938 ADR-0051)* : §P2 (O(depth)), §P4 (audit qualifié + rate-limit par resource), §P6 (asymétrie orphelin/référence pendante + dette SEF-10). Voir `spec/02-properties.md`.
- [x] **T3 — Vérifier spec/08** *(2026-05-30)* : T12 (confused-deputy SEF-9) ajouté — flood CapabilityDenied → masquage audit, correctif #6 (agrégation par resource ≤32) + régression-test PASS. §2/T1 mis à jour. §5 cartographie mis à jour. Version 1.5. Voir `spec/08-modele-menace.md`.
- [x] **T4 — Mettre à jour TODO.md** *(2026-05-30)* : T1–T3 cochés. Déclencheurs dormants déjà inscrits ci-dessous.

**Déclencheurs dormants (ne pas instruire avant)** :
- GC des orphelins → déclenche re-séparation CAS/index (ADR-0049 §D3a)
- ~~B-fort multi-tenant → déclenche au 2ᵉ `TenantId` distinct (ADR-0036)~~ **RÉVEILLÉ puis RÉSOLU le 2026-06-07** : MT-1 a introduit ≥2 tenants, BF-0→BF-3 ont livré B-fort complet (CauseHandle obligatoire, 137/137 tests). Voir chantier en tête de fichier + ADR-0058/0059/0060.
- #7b commit cross-store atomique → suit GC (F4 REVIEW-2026-05-30)
- D-P3a seL4 sur hardware réel → bloqué infra (board physique / NVMe passthrough)
- ADR-0050 §D6 C2/hardware représentatif → bloqué infra

---

## Phase 10 — Inférence réelle + E1/E3/P1b (ouvert 2026-05-30 — ADR-0052)

`OllamaBackend` existant (`poc/runtime/src/inference/mod.rs`), gestionnaire de slots entièrement implémenté (ADR-0022/0023/0030/0031). Phase 10 = branchement + mesure, aucun design. Garde-fou : verdicts non transférables au hardware cible (GPU 24 GB, spec/07 §2). Condition ADR-0050 §D6 partiellement levée (moitié inférence réelle) ; moitié C2/hardware reste ouverte.

- [x] **Axe A — Runners P10-S3 + P10-S5 sous OllamaBackend** *(2026-05-30 — binaires compilés, smoke test PASS)* : `poc/runtime/src/bin/p10_s3_runner.rs` + `p10_s5_runner.rs`. Assertions P-α/P-β/P-γ (S3) + A-priorité/A-E3/A-E1 via `QueueTrace` (S5). Garde-fou non-transférabilité câblé. `queue_traces()` ajouté sur `InferencePool`. Smoke test llama3.2:3b : 2 workers, cap=1, **t_infer≈13,7 s sur CPU**, PASS.
  - **Run complet PASS** *(2026-05-30)* : P10-S3 (6 workers, cap=2) PASS — overhead≈0, t_infer médiane 12,5 s p99 18 s. P10-S5 (3 fg + 1 sv, cap=1) PASS — A-priorité, A-E3, A-E1 FIFO vérifiés. Verdicts → `poc/scenarios/P10-S3/VERDICT.md` + `P10-S5/VERDICT.md`. t_infer CPU (13–18 s) non transférable hardware cible (ADR-0052 §D2). Coordination C1↔C2 : aucune anomalie détectée (total_promoted=0, overhead scheduler≈0).
- [x] **Sous-axe B — Oracle P5 #3 (conditionnel)** *(2026-05-30 — Phase 11 T1)* : sous-axe B NON déclenché. Voir `decisions/0052-scope-phase-10-inference-reelle.md §Clôture sous-axe B`.

**Non-objectifs** : C2 recalibré hardware (mur infra), #7b commit cross-store, #8/D-P3a/β-seL4, B-fort multi-tenant, C.12+ seL4, GC orphelins, campagne P2/P3/P5 (**SOLDÉE Phase 12 / ADR-0053** : A-P2 SEF-12 PASS, A-P3 SEF-13 PASS, A-P5 clos Branche NON — résidu unique : dette oracle #3 dormante, déclencheur non atteint, ADR-0053 §Non-objectifs).

---

## Mise à l'épreuve adversariale (ouvert 2026-05-30 — ADR-0050)

Le système a été validé propriété par propriété (SEF-1→7, S1–S14) sous faute unique contrôlée. La campagne **attaque** au lieu de valider. Substrat = PoC Linux (garde-fou : non-transférable seL4, isolation testée = logicielle). Ordre ADR-0050 §D5 : gate → axe 1 → axe 3.

- [x] **Gate soundness (préalable bloquant)** *(2026-05-30)* — `poc/scenarios/SEF-8-soundness-gate/VERDICT.md`. Audit des oracles de P1–P6 + SEF-7 vs invariants spec/02. **Bilan : 5 INSTANCIÉE, 5 PROXY, 1 SUR-GARANTIE.** Recadrage : axe 1b (P4-audit = PROXY sous flood, F2) et axe 3 (P6-Linux INSTANCIÉE *seulement* en crash-processus, SEF-4 ne scanne pas les orphelins ContentStore) **confirmés pertinents** ; axe 1a (P4-isolation = INSTANCIÉE) a une base saine. **Gate clos** (domaine fini, verdict total). Voir LESSONS L87.
  - **Findings hors axes → corrections spec (décision architect avant édition spec/02)** : (1) P2 « O(log N) » contredit l'impl `rollback_path` O(depth) ; (2) P4 « 100% loggé » non tenu sous rate-limit ; (3) P5 validé sur agent trivialement déterministe (mécanisme S6 non exercé) ; (4) P6 « état = ContentStore » (spec/02) vs oracle observant le log (ADR-0027). Le gate les *constate*, ne les tranche pas.

- [x] **Axe 1 — interactions entre défenses (P4 + fidélité d'audit)** *(2026-05-30)* — `SEF-9` (`poc/scenarios/SEF-9-confused-deputy-audit/VERDICT.md`, test `sef9_audit_masking_under_flood`). **Finding confirmé : confused-deputy rate-limit ↔ audit.** Agent inonde 101 refus bénins → son refus malveillant `"secret"` (count 102) silencé, **non attribuable au log**. **1a (isolation) INTACTE** (`get` refusé -1, cap jamais accordée) ; **1b (fidélité audit) ÉCHOUE** (témoin hors-bande capte `"secret"`, log non). Démontre que le mécanisme anti-flood-log désarme la complétude d'audit. Instrumentation : `AgentState::cap_denied_witness` (None en prod). Voir LESSONS L88.
  - **→ Conséquences (architect)** : (1) correction spec/02 §P4 « 100% loggé » (faux sous flood) — déclencheur axe 1b atteint, à grouper avec les corrections du gate ; (2) décision design : agréger le rate-limit **par resource** (ensemble borné) lèverait le masquage sans réintroduire le DoS de log — ou accepter+documenter.

- [~] **Axe 3 — crash concurrent à invalidation de cache (P6)** *(2026-05-30 — partie constructible PASS, durabilité différée)* — `SEF-10` (`poc/scenarios/SEF-10-cross-store-crash/VERDICT.md`, test `sef10_cross_store_dangling_snapshot`).
  - **Mur d'infra acté** : `drop_caches` inaccessible (pas root), pas de VM → verdict durabilité power-loss **recevable non exécutable ici** (mur identique β seL4, ADR-0046). Pas de simulation maison (piège L32). **Durabilité différée** à root/VM/matériel.
  - **Finding design (rigoureux)** : ContentStore + CausalLog = 2 instances RocksDB séparées, commit store-puis-log sans fsync ni atomicité cross-DB → **fenêtre de référence pendante** (log en avance sur store sous cache-loss) = état déchiré que le no-force n'autorise pas.
  - **Finding sévérité (testé)** : état déchiré construit → (a) `restore_from_evicted` l'adopte **sans détection** ; (b) rollback P2 cassé `MissingBlock` (symptôme tardif) ; (c) pas de panic. Voir LESSONS L89.
  - **→ Conséquences (architect)** : (1) dette soundness P6 cross-store (candidat : commit atomique / WAL commun, lié à la re-séparation CAS/index ADR-0049 §D3a) ; (2) correctif local peu coûteux : vérifier `last_snapshot ∈ store` au restore (fail-safe).

- **Hors scope (ADR-0050 §D6)** : axe 2 plafonds (inférence stubbée F1, cap C2 non recevable) ; frontière LLM (spec/08 §0.1, knowledge non garanti) ; P1/P2/P3/P5 (non-cibles d'axe, campagne dédiée différée).

- [x] **Tri des findings + vague de correctifs** *(ADR-0051 — 2026-05-30)* : 8 items triés (revue architect). **Amendements spec/02** : §P2 (« O(log N) » → O(depth), revendication retirée), §P4 (audit qualifié « jusqu'au rate-limit » + rehaussé par #6), §P6 (asymétrie orphelin toléré / référence pendante + trou cross-store inscrit). **Correctifs code (régression-tests verts)** : #6 agrégation rate-limit `0x14` **par resource** bornée → masquage levé (SEF-9 `masked={}`) ; #7a `restore_from_evicted` défend `last_snapshot ∈ store` → référence pendante détectée tôt (SEF-10 Err). Voir ADR-0051.
  - **Différés tracés** : **#3** (P5 dette d'**oracle** — SEF-6 teste un agent trivialement déterministe ; déclencheur : campagne P2/P3/P5 dédiée ou agent consommant une primitive non-déterministe) ; **#7b** (commit cross-store atomique — déclencheur : chantier GC / re-séparation CAS-index, ADR-0049 §D3a requalifié) ; **#8** (verdict durabilité power-loss — déclencheur : substrat média réel, **groupé avec D-P3a et β seL4**).

---

## Phase 7 — Qualification + Scheduler unifié + SEF (2026-05-18)

Trois axes issus de l'état post-Phase 6. Ordre de priorité : Axe 1 d'abord (C2 bloque le dimensionnement de tout le reste), Axe 2 ensuite, Axe 3 en parallèle avec Axe 2.

### Décisions spec 2026-05-23 (suite revue architect)

- [x] **H-wake-latence — T7 MESURÉ** *(2026-05-25)* : **T_wake = 311 µs (p99)**, p50 = 204 µs. N=50 agents, N_dormant=20, CAP_IO=3, K=3 runs, 60 samples, 0 erreur. État AGENT_WAT (64 KiB, cache chaud). Option A (admission prédictive) non nécessaire dans ce régime : 311 µs << cycle 5 s. Limites : état minimal + cache chaud ; W1 + charge réelle attendu 3–10× plus élevé. Critère PASS suggéré : p99 < 10 ms (×32 marge). Voir `results/T7/wake/SYNTHESE.md`.

- [x] **Note portée hardware — spec/01 §3.1 + spec/07 §3.3** *(2026-05-23)* : clarifie que 14 agents/s × cycle 5 s = **~70 agents simultanément actifs en steady-state** sur hardware consumer (pas "des centaines"). Distingue densité hébergée (P1a, ~3 M idle sur 16 GB RAM, validée) de densité active (P1b, 70 agents, mesurée). La projection ~100 agents/s reste conditionnelle à hardware serveur PCIe Gen4 non encore qualifié.

### Axe 1 — Qualification hardware (préalable)

- [x] **C2 — fio sur NVMe PCIe (classe 2)** : mesuré 2026-05-18 via T5-qualif classe 2. Seq QD=1 : 1 290–1 321 MB/s ; rand QD=1 : 9 039–10 865 IOPS ; rand QD=32 : 125 000–130 000 IOPS. Cap C2 classe 2 : 25 agents/s. Cap retenu conservateur toutes classes : **14 agents/s** (borne basse AWS). Spec/07 §3.3 mis à jour. **Reste** : qualifier un NVMe serveur PCIe Gen4 dédié pour relever le cap vers ~100 agents/s.

- [x] **T5-qualif** : K=3 runs conformants sur AMD Ryzen 5 PRO 4650U + WD SN530 NVMe PCIe (classe 2). **P3a → validé** (ADR-0026, 2026-05-18) : 2 classes hardware, K≥3 chacune, régime cache-mixte contraint entériné. p99 pire cas toutes classes : 4 855 µs (×2 sous cible). IOPS rand QD=1 (10K) et QD=32 (130K) produites pour la première fois. Voir `results/T5/SYNTHESE.md`.

- [x] **T5-bis — P3b** *(résolu 2026-05-18)* : K=3 runs N=10⁸ sur AMD Ryzen 5 PRO 4650U / WD SN530 NVMe (classe 2). **P3b partiellement validé (1 classe)** — les 3 runs passent la borne 20 ms, mais avec progression thermique marquée : p99 = 3 972 µs (RB1) → 12 294 µs (RB2) → 19 644 µs (RB3, marge 356 µs). Risque TODO déclenché : p99 > 15 ms sur RB2 et RB3. Voir `results/T5-bis/SYNTHESE.md`.
  - **Signal thermique** : p50/p95 stables (≈ 900/1 550 µs) — régime normal solide. La dégradation p99 est un tail event SLC→TLC du SN530 amplifié par runs consécutifs sans refroidissement. En production (workload non-consécutif), p99 attendu ≈ RB1 (3 972 µs, ×5 sous cible).
  - **Décision** : la borne 20 ms est tenue mais **inconfortable sur NVMe consumer sous charge soutenue**. L'ADR P3b n'est pas révisé (la borne reste 20 ms), mais SEF-5 doit être annoté « recommandation validation C2 hardware (PCIe Gen4 server) avant traitement P3b comme hypothèse de travail ».
  - **Découplé de SEF-4** : ADR-0027 (2026-05-18) clarifie que SEF-4 vise le régime SIGKILL/panic (WAL OS-buffered survit), pas power-loss.
  - **[x] T5-bis-thermal — dissociation causale p50/p95 vs p99 (falsifiabilité empirique)** *(résolu 2026-05-23 — RÉFUTÉ)* : refaire une série de runs avec surveillance de la température NVMe/CPU. Protocole : (A) 3 runs consécutifs sans pause (reproduit la progression thermique RB1→RB3) puis (B) 3 runs avec pause jusqu'à retour à un seuil de température cible. Verdict : Spearman(rank(p99), rank(T_max)) > 0.7 (phase A) **et** régression p99 vs run_index b≈0 (phase B).
    - **Implémenté 2026-05-23** : `benchmarks/t5-bis-bundle/run-thermal.sh` — capteurs sysfs sans root (`/sys/class/hwmon/hwmon{3,4}/temp1_input`), monitor thermique 1 Hz, verdict Python3 (Spearman + OLS). Sorties : `results/T5-bis-thermal/<TS>/thermal.jsonl` + `verdict.json` + `summary.md` + `A<n>.log` / `B<n>.log`.
    - **Bugs corrigés 2026-05-23** : (1) `start_thermal_monitor` deadlock : le `$(...)` bloquait indéfiniment car le `()&` héritait du write-end du pipe `$()`. Fix : `>/dev/null 2>/dev/null &`. (2) Parsing metrics : `run_one` cherchait `"p50_us":` dans le log de run.sh qui n'émet pas ce format. Fix : run.sh émet maintenant `T5BIS_THERMAL: p50_us=X p95_us=Y p99_us=Z ...` sur stdout ; run_one parse ce tag. (3) sudo bloquant : SKIP_INSTALL=1 ajouté. (4) tmpfs : T5BIS_BENCH_DIR pointé sur ext4 ; répertoire partagé pour économiser le fichier fio 4 GB ; cleanup des `t5bis-causal-*` entre runs (db N=10⁸ ≈ 10–15 GB, 52 GB libres).
    - **Smoke test 2026-05-23** (N=100 K, Phase A seulement) : plomberie validée. 3 runs complétés. p99 = 1 603 / 1 887 / 1 794 µs, T_max_NVMe = 49.9 / 55.9 / 57.9°C. Spearman(p99, T_max) = 0.50 (seuil > 0.7 non atteint — signal thermique trop faible à N=100 K, DB tient en cache). p50/p95 stables.
    - **[x] Résolu 2026-05-23** : run complet N=10⁸ (Phase A + Phase B). **Verdict : RÉFUTÉ** (les deux critères échouent).
      - Phase A : Spearman(rank(p99), rank(T_max)) = −0.50 (seuil > 0.70) → ✗. A3 a le p99 le plus bas (2 553 µs) malgré la T_NVMe la plus haute (60.85°C) — réchauffement du page cache OS après 2 runs identiques annule l'effet thermique sur les lectures NVMe.
      - Phase B : |b/se_b|(p99) = 3.06 (seuil < 1.0) → ✗. p99 augmente malgré les pauses thermiques (B1=3 757 µs, B2=6 479 µs, B3=16 282 µs) alors que T_NVMe diminue (55.9→52.9→50.9°C) — corrélation inverse, pas de causalité thermique.
      - p50/p95 stables sur tous les runs (≈ 880–1 063 / 1 428–1 709 µs) → régime normal invariant.
      - **Interprétation** : la variance p99 est causée par la fenêtre de compaction L0 RocksDB frappant aléatoirement dans le run — même cause que le pattern dents-de-scie T6-soak. Le modèle thermique est falsifié. Voir `results/T5-bis-thermal/2026-05-23T095915Z/verdict.json`. **ADR-0032** (réfutation formalisée). **Dette CLOSED.**

- **[x] T5-ter — isolation p99 vs compaction RocksDB** *(résolu 2026-05-24, ADR-0032 §D4, L57)*
  - **Mode A** (disable_auto_compactions + compact_all avant mesure) : K=3 runs, N=10⁸. p50/p95 stables (580–620 / 1 200–1 350 µs). p99 volatile (1 688 / 5 420 / 2 125 µs, ratio 3×). **Verdict FAIL (±20%)** — source des spikes = OS/NVMe burst I/O, corrélation compaction = 0 % sur les 3 runs. P3b-intrinsèque = **p99 ≈ 1 700–2 200 µs** (plancher OS/NVMe sans RocksDB).
  - **Mode B** (config normale + polling propriétés RocksDB par cycle) : K=3 runs, N=10⁸. Corrélation moyenne **100 %** (95/95, 207/207, 157/157 spikes avec signal compaction). p99 = 4 000 / 19 198 / 17 531 µs. `running_compact>0` sur 100 % des cycles. L0 = 23 files avant mesure. **Verdict CONFIRMED.** La compaction RocksDB est la cause mesurable et exclusive des spikes p99 ≥ 5 ms en régime nominal.
  - **Décomposition P3b** : P3b-intrinsèque ≈ 2 ms (plancher NVMe), P3b-with-LSM ≈ 4–19 ms (borne 20 ms tenue en médiane). Critère ±20% à p99/N=10K trop sensible au bruit OS — réviser vers N=100K ou p95 pour future qualification.
  - **Cap ~100 agents/s** : la compaction est responsable de 3–17 ms de variance à p99 ; en régime stabilisé (compactions non-bloquantes) le cap reste valide. À surveiller si `files_l0` approche `level0_stop_writes_trigger`.
  - **Voir** : `results/T5-ter/SYNTHESE.md`, `results/T5-ter/a/2026-05-23T174223Z/`, `results/T5-ter/b/2026-05-23T205112Z/`. **Dette CLOSED.**

- [x] **T6-qualif** *(2026-05-22)* : K=3×3 runs Wasmtime (N=100/500/1000) + baseline Docker Python LLM réaliste (N=100 containers). Ratio Wasmtime/Docker : 4539–7375×, cible ≥ 5× satisfaite. **H-densité → partiellement validé**. Voir `results/T6/phase-a/2026-05-22T134309Z/verdict.json`.
  - [x] **T6-scaling — loi de scaling overhead/agent** *(résolu 2026-05-22)* : K=3 runs pour N ∈ {100, 300, 1000, 3000}. Fit 4 modèles (constante/log/sqrt/1/N) — **meilleur : `overhead(N) = 9.65 − 54/N` KB, R²=0.988**. Le terme super-linéaire (+9 %/décade) signalé sur 2 points est un artefact : overhead fixe partagé (~54 KB WASM+Tokio) mal amorti à N=100. Courbe sature à N=300 ; **overhead = O(1) par agent** pour N≥300. Prédiction N=10 000 : 9.64 KB/agent. Voir `results/T6/SYNTHESE.md §T6-scaling` et `results/T6/phase-a/2026-05-22T155530Z/`.
  - **[x] T6-soak — absence de fuite mémoire** : N fixe (ex. 500), K=1 run de plusieurs heures, courbe RSS. Orthogonal au scaling en N.
    - **Implémenté 2026-05-23** : mode `t6-soak` ajouté dans `poc/benchmarks/src/main.rs`. Usage : `cargo run -p os-poc-benchmarks --release -- t6-soak [N [HOURS]]`. (1) Spawn N agents, (2) boucle de HOURS heures — `Message::data(...)` par agent toutes les 1 000 ms via `try_send`, (3) RSS échantillonné toutes les 60 s → `results/T6/soak/<timestamp>/rss.jsonl` + `verdict.json`. Régression OLS sur points post-warmup (skip 5 premiers). Test non-linéaire : si slope(2ème moitié) > 2× slope(1ère moitié) → warn compaction/fragmentation.
    - **Critère PASS** *(corrigé, dérivé de H-profil-B)* : pente `b < N × overhead_per_agent / 60` KB/min. Dérivation : au taux PASS, growth(1h) ≤ overhead_initial — compatible avec H-profil-B durée min 1h. À N=500, overhead ≈ 9.64 KB/agent → seuil ≈ 80 KB/min (vs 100 KB/min arbitraire du plan initial). Seuil calculé dynamiquement au runtime depuis la mesure de spawn réelle.
    - **Hors périmètre** (documenté dans le code) : cycle evict/wake (scheduler.dormant) — couvert par S11/S12. Métriques RocksDB internes (num-files-at-level0) non exposées par l'API ContentStore — à ajouter si un FAIL est observé et que la cause est ambiguë.
    - **Correction 2026-05-23** : TempDir (/tmp → tmpfs RAM) masquait les fuites réelles — la RSS croissait avec le volume d'écriture BdD, pas les allocations WASM. Corrigé : store + log maintenant sur chemin disque réel (`poc/results/T6/soak/<ts>/data/`). Avec NVMe, les SST RocksDB vont sur disque et le block cache (256 MB) absorbe les lectures → RSS stabilisé après warmup + compaction.
    - **[x] Résolu 2026-05-23** : `cargo run -p os-poc-benchmarks --release -- t6-soak 500 4` (4 h). **Verdict : FAIL** (critère OLS).
      - Pente mesurée : **1 067.8 KB/min** vs seuil 80.6 KB/min (13.2×). R²=0.66. Ratio 2ème/1ère moitié : 0.41× (la pente diminue en 2ème moitié — signe de stabilisation post-compactions).
      - Messages envoyés : 7 147 500 (= 500 agents × 4h). Overhead/agent mesuré : 9.7 KB (cohérent avec T6-scaling).
      - **Pas de fuite WASM.** Le pattern est un sawtooth RocksDB classique : write buffers (memtables) remplis à ~26 MB/min, flush en SST → compaction → chute RSS ~150–230 MB, toutes les 30–50 min. 10–17 compactions sur 4h. Les baselines post-compaction restent dans le même ordre de grandeur sur 4h.
      - **Le critère OLS global est inadapté** pour un workload write-intensif avec LSM tree. Il mesure le rythme de remplissage des write buffers, pas une fuite applicative. Voir `poc/results/T6/soak/1779528184/verdict.json` et `lab/LESSONS.md §L55`.
      - **Révision du critère requise (ADR-0033)** : critère retenu = OLS sur `RSS − rocksdb.cur-size-all-mem-tables` (Option b). Expose `ContentStore::get_rocksdb_int_property`. **Verdict courant : INVALIDE (critère inadapté).** Re-run T6-soak requis pour verdict valide. Voir `decisions/0033-critere-fuite-memoire-lsm.md`.
    - **[x] T6-soak v2 — re-run avec critère ADR-0033** *(résolu 2026-05-24, ADR-0034)* : implémenté `CausalLog::total_memtable_bytes()` + correction formule rss_adj (ContentStore + CausalLog). Runs de diagnostic : 1 agent × 1h (R²=1.00 → bug memtable identifié ; après fix R²=0.74, ratio=0.42), 500 agents × 30 min (R²=0.24, OLS structurellement inutilisable — spikes allocateur post-flush). **H-fuite-mémoire infirmée.** RSS borné : memtables ~256 MB + block caches ~512 MB + overhead agents ~5 MB ≈ 793 MB total. Critère OLS retiré. Voir `decisions/0034-refutation-fuite-memoire-t6-soak.md`.

### Axe 2 — Scheduler unifié C1+C2

Pièce manquante la plus structurante. `InferenceQueue` (C1) est en place (ADR-0022/0023) ; C2 (I/O admission control) reste « design proposé, non implémenté » (spec/07 §3.3). La spec est explicite : les deux doivent être coordonnés par un scheduler unique — précharger l'état d'un agent sans slot d'inférence disponible gaspille du cache.

- [x] **ADR-0030 — Scheduler unifié C1+C2** *(2026-05-22)* : `IoAdmissionQueue` (C2) + pipeline C2→C1. Interface : `io_queue.acquire(agent_id, priority, last_active)` → précharge ContentStore, puis `InferencePool::submit` (C1). Priorité sémantique + affinité cache. `Scheduler::reap()` câblé dans `register()`. ADR-0030. Scénario S10 : 3/3 pass.

- [x] **I/O Admission Control** *(2026-05-22)* : `IoAdmissionQueue` dans `poc/runtime/src/io_queue.rs`. Cap paramétrable (`cap_actif`), 3 files VecDeque (Supervisor/Foreground/Batch), dispatcher Tokio, permit RAII, affinité de cache (cache_score). 5 tests unitaires : borne dure, priorité, affinité cache, FIFO intra-classe, intégration bound.

- [x] **Câbler C1 × C2** *(2026-05-22)* : pipeline séquentiel C2→C1 implémenté dans `s10_runner.rs`. Scénario S10 : N=8 agents, cap_io=3, k_infer=2, K=3 runs. P-α max_io ≤ cap_io (invariant dur, 3/3), P-γ n_completed=8 (3/3). P-δ (proxy latence) : 1/3 runs favorable, proxy bruité avec N=2/classe — voir dette P-δ-invariant.
  - [x] **Câblage préparatoire C1→C2** *(2026-05-22)* : `slot_freed_notify()` + `new_with_c1_hint()` + `select!` dans io_dispatcher. Précondition pour le SchedulerCoordinator futur. Test `t_c1_hint_wires_without_deadlock` prouve l'absence de deadlock, pas un bénéfice de latence. ADR-0030 §Câblage préparatoire.
  - [x] **P-δ-invariant — remplacer le proxy latence par l'invariant d'ordre d'admission** *(résolu 2026-05-22)* : `IoQueueState.pop_best()` instrumente deux compteurs `pop_with_sup_present` / `sup_chosen_when_present` ; exposés dans `IoQueueStats`. P-δ dans S10 devient `sup_chosen_when_present == pop_with_sup_present` — déterministe, sans timing, sans N≥5. S10 3/3 pass. P-δ maintenant dans le verdict global (plus seulement observatoire).
  - [x] **Cycle evict/wake (ADR-0030 §FutureWork débloqué)** *(résolu 2026-05-22)* : `Message::Evict { reply }` + `EvictedState` dans `actor.rs` ; `Scheduler::dormant` + `evict_agent()` + `wake_agent()` + `ActorInstance::restore_from_evicted()`. Scénario S11 (`poc/scenarios/S11-evict-wake/`) : 3/3 pass — P-α (table dormant), P-β (seq+snapshot préservés), P-γ (chaîne ContentStore intacte), P-δ (Suspended logué). 90/90 tests lib.
  - [x] **SchedulerCoordinator — réveil à la demande (S12)** *(résolu 2026-05-23 — ADR-0031)* : Option B (lazy wakeup) implémentée. `Scheduler::deliver` + `DeliverError` + `EvictedState.evicted_at` (capturé dans `evict_agent`). Scénario S12 (`poc/scenarios/S12-scheduler-coordinator/`) : 3/3 pass — P-α (dormants réveillés), P-β (cap_io respecté), P-γ (actifs bypassent C2). 92/92 tests lib. Admission prédictive (Option A) en FutureWork : critère = latence p99 deliver > budget documenté sous charge réelle (H-wake-latence à définir).

- [x] **Scheduler::reap() — appel périodique** *(2026-05-22, ADR-0015 dette soldée)* : `reap()` appelé au début de `Scheduler::register()`. Nettoie handles/senders des agents terminés à chaque nouvel enregistrement.

### Axe 3 — SEF (vérification des propriétés système)

La spec définit SEF-1 à SEF-6 ; tous sont maintenant implémentés. Sans SEF, P2–P6 sont garanties « by construction » (le code est correct) mais jamais « by observation » (aucun scénario ne les a exercées de bout en bout).

- [x] **SEF-1 — Persistance d'état après redémarrage** *(résolu 2026-05-26)* : binaire `sef1-runner` + scénario `poc/scenarios/S13-persistence-restart/`. Phase 1 : N=100 actions → shutdown propre (drop tx + await handle + drop Arcs RocksDB). Phase 2 : réouverture des mêmes chemins → 4 propriétés : P-α (SnapshotHeader intact), P-β (log intact, entrées ≥ pré-shutdown), P-γ (bloc 64 octets bit-à-bit identique), P-δ (ActorInstance restauré via `restore_from_evicted` continue la chaîne causale — hash_before == H_before). **Verdict : 5/5 pass.** Voir `poc/scenarios/S13-persistence-restart/report.json`.

- [x] **SEF-5 — P3a Traçabilité causale lookup** *(résolu 2026-05-26)* : binaire `sef5-runner` + scénario `poc/scenarios/S14-causal-lookup/`. DB unique N=10⁸ entrées (CausalLog RocksDB), 1 passe de population + K=3 passes de mesure indépendantes. P-α : p99 ≤ 10 ms sur 10 000 get(action_id) ; P-β : 1 000 entrées vérifiées bit-à-bit (action_id()==clé, agent_id, hash_before, hash_after, emit_payload). **Verdict : 3/3 pass.** p99 par passe : 1 368 / 1 727 / 1 850 µs (×5–7 sous cible). Voir `poc/scenarios/S14-causal-lookup/report.json`. Note technique : `_exit()` via FFI requis pour éviter le SIGSEGV RocksDB atexit sur DB 10⁸ entrées (compaction threads en vol au moment de la fermeture).

- [x] **SEF-4 — P6 Atomicité crash** *(résolu 2026-05-18)* : agent en transaction → `SIGKILL` (`process::exit(1)`) à un point armé → recovery → état observable via log comparé aux états admissibles de P6.
  - **Implémentation** :
    - 4 nouveaux `CrashPoint` ajoutés à `poc/runtime/src/crash_point.rs` (feature `crash-injection`) : `CommitBarrierPrePutBlock`, `CommitBarrierBetweenPutBlockAndPutSnapshot`, `CommitBarrierPostPutSnapshotPreLogAppend`, `CommitBarrierPostLogAppend`. Mécanisme `armed::arm/disarm/current` (AtomicU8) pour activer un point précis depuis le binaire victim.
    - 2 binaires `poc/runtime/src/bin/{sef4_victim, sef4_verify}.rs` (`required-features = ["crash-injection"]` pour victim ; verify compile sans la feature).
    - Scénario `poc/scenarios/S6-crash-atomicity/` : README + `run.sh` orchestrant 4 kill_points × 2 kill_actions × K=5 = **40 runs**.
  - **Verdict** : 40/40 pass. Distribution des cas observés :
    - 25 runs : `observed = pre[k]` (action k non committed côté log) — case 1 strict
    - 8 runs : `observed = pre[k-1]` ou `pre[k-2]` (perte d'actions terminales avant le kill) — case 1 préfixe étendu, autorisé sous SIGKILL/no-fsync
    - 7 runs : `observed = post-action-i` avec `i ∈ {k-1, k}` — case 2 (action committed, hash non précalculable)
  - **Découverte importante** : RocksDB en `WriteOptions::default()` buffer applicativement le WAL au-delà du page cache OS. Plusieurs actions terminées peuvent être perdues sous `process::exit(1)`. **P6 (atomicité par action) tient strictement** — aucun état observé n'est partiel. Mais la formulation ADR-0027 D3 « toutes les écritures depuis l'ouverture survivent » est trop forte ; clarification ajoutée à ADR-0027 §Observation post-décision.
  - **Hors scope explicite (ADR-0027)** : pas de test power-loss. SEF-4 vise SIGKILL/panic.

- [x] **SEF-2 — P2 Rollback transactionnel** *(résolu 2026-05-18)* : après 1 000 actions, rollback à l'action n°500, vérifié sur 5 runs.
  - **Implémentation** :
    - Binaire `poc/runtime/src/bin/sef2_runner.rs` (sans crash-injection — pas de panne testée).
    - Scénario `poc/scenarios/S7-rollback-equivalence/` : README + `run.sh` orchestrant K=5 runs avec N=1000 actions, k_target=500, budget rollback 100 ms.
  - **Cinq propriétés vérifiées par run** (pas une seule — la formulation initiale « hash identique » est triviale sous content-addressed) :
    - P-α : `SchedulerRollback.hash_after == hash_at_k` (exerce `rollback_path` sur 500 sauts)
    - P-β : `SnapshotHeader(hash_at_k).seq == k - 1` (indexation seq cohérente)
    - P-γ : payload `SchedulerRollback.target_seq == k - 1` (Scheduler::rollback envoie bien la cible demandée)
    - P-δ : action post-rollback `hash_before == hash_at_k` (vraie cohérence — l'agent reprend depuis l'état restauré)
    - P-ε : durée rollback ≤ budget (borne de performance P2)
  - **Verdict** : 5/5 pass. Durées observées (release, AMD Ryzen 5 PRO 4650U + WD SN530) : 17, 17, 17, 20, 19 ms (budget 100 ms — large marge).
  - **Note méthodologique** : la durée mesurée inclut un overhead de polling (sleep 5 ms entre lectures du log). Pour mesurer P2 strict, voir `poc/store/benches/rollback_latency.rs` (H-rollback-latence — p95 = 99 µs sur N=10⁶). SEF-2 vise la **propriété d'équivalence d'état**, pas la borne de latence fine.

- [x] **SEF-3 — P4 Isolation capabilities** *(résolu 2026-05-18)* : les trois critères P4 vérifiés — (a) 100 % des accès couverts réussissent, (b) 100 % des accès non couverts échouent, (c) 100 % des refus sont dans le log causal (`CapabilityDenied 0x14`). Scénario S9 (`poc/scenarios/S9-capability-isolation/`) : 1 parent + 10 sous-agents, 84 tests verts. Décision : ADR-0029 (`scope_covers` préfixe + émission côté runtime). **Hors périmètre SEF-3** : propagation récursive des révocations — traitée séparément dans le plan H-revoke (`spec/04-hypotheses.md`).

- [x] **SEF-6 — P5 Déterminisme de transition** *(résolu 2026-05-18)* : deux instances initialisées avec un état identique, même séquence de 1 000 messages → hash d'état finaux identiques.
    - **Précondition S6 implémentée (ADR-0028)** : audit 2026-05-18 a relevé 24 call-sites `SystemTime::now()` dans `poc/runtime/src/` ; 11 d'entre eux inscrivaient un timestamp dans une structure hashée (`SnapshotHeader.ts_us`, `LogEntry.ts_ms`, `EmitEnvelope.ts_us`). Sans substitution, deux runs identiques produisent des chaînes content-addressed bytewise différentes. La primitive `crate::clock::Clock` (`SystemClock` prod, `LogicalClock` replay) substitue tous les call-sites concernés ; les call-sites hors SEF-6 (scheduler.rs::emit_compensation_*, agent_infer durée Instant) restent intacts et sont documentés dans le périmètre.
    - **Mécanisme** : `AgentState.clock: Arc<dyn Clock>` ; tous les host functions (commit_barrier, emit, log_lifecycle, log_session_boundary, log_agent_crash, record_validation_response, agent_self_rollback, agent_request_validation, Message::Rollback handler, session bounds check, agent_infer ts_ms) lisent `state.clock.now_*()`. Constructeurs préexistants rétro-compatibles via `SystemClock` par défaut ; nouveau `ActorInstance::new_precompiled_with_clock` pour le replay.
    - Binaire `poc/runtime/src/bin/sef6_runner.rs` : lance deux instances avec `LogicalClock(start)` identique, envoie 1 000 messages, compare last_snapshot + séquence d'action_ids. 3 propriétés vérifiées (P-α last_snapshot identique, P-β séquence bytewise identique, P-γ hash agrégé identique).
    - Scénario `poc/scenarios/S8-determinism/` avec `run.sh` K=5 runs (release).
    - **Verdict** : 5/5 pass (AMD Ryzen 5 PRO 4650U). N=1 000 messages par run, 2 instances par run → 10 traces totales toutes bit-à-bit identiques entre A et B sur le même run. Rapport : `poc/scenarios/S8-determinism/report.json`.
    - Tests runtime : 79/79 pass (`cargo test -p os-poc-runtime --lib`, avec et sans crash-injection). Aucune régression. Tests `clock` ajoutés (3 unitaires : logical_clock_is_deterministic, logical_clock_ms_us_share_counter, system_clock_returns_nonzero).
    - **Hors périmètre SEF-6 (documenté README S8)** : agent_infer (backend stochastique + Instant::now duration_ms), Scheduler::rollback (compensation 0x11/0x12 émis avec agent_id scheduler), déterminisme d'exécution complet (spec/05-non-goals §3.3).
    - Décision : ADR-0028.

---

## Dettes ADR-0015 — implémentation restante (2026-05-17)

Issues de la décision ADR-0015 acceptée le 2026-05-17.

- [x] **D15.2-a — Émission `AgentCrash (0x13)` dans `actor.rs`** : résolu 2026-05-18. Implémenté avec variante architecturale : `log_agent_crash` fixe `lifecycle=Terminated` atomiquement (un seul append RocksDB, D-Q-V2.2). Il n'y a **plus de `Lifecycle::Terminated` séparé** après un crash — `AgentCrash` est le terminal event. Conséquence : ADR-0015 P-D15-1 doit être amendé (voir dette ci-dessous).

- [x] **D15.2-b — `parent_agent_id` dans `AgentState`** : résolu antérieurement (commit 298a5e9).

- [x] **D15.2-c — `os-poc-reconstruct` payload `0x13`** : décodage payload AgentCrash (cause/parent/last_action) déjà présent. Ajouté 2026-05-18 : synthèse de `Lifecycle::Terminated [synthétisé — ADR-0015]` immédiatement après chaque AgentCrash dans la sortie reconstruct.

- [x] **ADR-0015 P-D15-1 — amendement spec** : reformulé 2026-05-18. P-D15-1 : « `AgentCrash` est l'événement terminal — `Lifecycle::Terminated` est synthétisé à la lecture par os-poc-reconstruct ». ADR-0015 §D15.2, §Propriétés, §Ce qui ne change pas mis à jour.

- [x] **Scheduler::reap() — appel périodique** *(2026-05-22)* : câblé dans `Scheduler::register()` — see Axe 2 ci-dessus.

---

## Dettes Phase 6 — RÉSOLUES (2026-05-17)

### [x] D9 — Watchdog WASM : calibration fine par profil (ADR-0025)
**Résolu Phase 6 :** `EPOCH_TICK_MS_BASE = 10 ms` (au lieu de 100 ms), `AgentProfile` enum (Algo/LlmShort/LlmLong/Batch), `max_ticks` par profil. `run_loop` réarme via `agent_profile.max_ticks()`. Profil émis dans payload `Spawned (0x01)` (byte additionnel rétro-compatible). `watchdog.rs` + `AgentProfile` dans `agent-sdk`. 3 tests dédiés (t_algo_profile_traps_at_100ms, t_llm_long_profile_allows_30s, t_profile_emitted_in_spawned). Décision : ADR-0025.

### [x] D-Q-V2.2 — Atomicité crash journal de compensation (ADR-0024)
**Résolu Phase 6 :** Stratégie J (journal de compensation) : `CompensationOpen (0x11)` émis avant cancel, `CompensationClose (0x12)` après rollback appliqué. `CrashPoint` feature-gated (`crash-injection`). `SCHEDULER_AGENT_ID = [0xFF;16]`. `os-poc-reconstruct` détecte les 0x11 orphelins et affiche `[INCOMPLETE COMPENSATION: agent_id=...]`. Test `t_no_crash_clean_path_emits_full_quartet` valide le quartet 0x11/0x0E/0x0B/0x12. Décision : ADR-0024.

### [x] D-Q-V2.6 — InferenceQueue bornée avec priorité et équité (ADR-0022 + ADR-0023)
**Résolu Phase 6 :** `InferenceQueue` : 3 files FIFO (Supervisor/Foreground/Batch), capacité totale bornée, dispatcher Tokio, promotion famine (Batch→Foreground après max_starvation_ms). `NoSlot (3)` maintenant actif. Payload `0x0C` enrichi avec `priority_class`, `queue_depth_at_admission`, `promoted_from`. 5 tests unitaires (t_queue_bounded_emits_no_slot, t_queue_priority_supervisor_passes_batch, t_queue_starvation_promotion, t_queue_evicts_batch_for_supervisor, t_queue_fifo_within_class). Scénario S5 (fairness-priority) : 8 Foreground + 2 Supervisor, cap=2, stable 10/10. Décisions : ADR-0022, ADR-0023.

---

## Préconditions — à trancher avant T5-qualif réplication et Phase 6

Issues de la critique architecturale post-T5 (2026-05-15). Pas de code, pas d'ADR — décisions de spec.

- [x] **Q1 — Portée de la borne 10 ms (P3)** : **borne officielle liée exclusivement à P3a** (lookup point `get(action_id)` sur DB statique). P3b (end-to-end emit→fsync→get) borne distincte ≤ 20 ms ; P3c (multi-agent concurrent) bornes ≥ 50 ms réservées. T5 mesure correctement P3a et uniquement P3a. Décision tracée dans `spec/02-properties.md §P3`, `spec/04-hypotheses.md §H-causal-latence`, `benchmarks/test-protocol.md §6.1` (2026-05-16). **Conséquence T5** : la classification « partiellement validé » du SYNTHESE T5 porte sur P3a et reste valide ; pas de requalification rétroactive nécessaire. T5-bis (P3b) et T5-multi-tenant (P3c) restent à produire pour leurs portées respectives.

- [x] **Q2 — Modèle de working set par agent** : **Modèle B (recency-biased) adopté comme convention de référence** ferme pour dimensionnement (C2, P3b/c, scheduler Phase 6). Paramètres : K=128 actions récentes, recouvrement inter-agent = 10 %. Modèle A (no-locality) conservé comme borne supérieure ; le cap C2 publié reste 14 agents/s (Modèle A, conservateur) jusqu'à mesure T5-bis cache-friendly. Décision tracée dans `benchmarks/reference-workload.md §W1-access`, manifest `workload.json.access_pattern` rendu obligatoire dans `benchmarks/test-protocol.md §3.4` (2026-05-16). **Conséquence T5** : T5 actuel utilise implicitement Modèle A (`populate_synthetic` uniforme) — note de portée ajoutée dans test-protocol §6.1, pas de requalification mais étiquetage explicite désormais requis.

- [x] **Q3 — `emit_payload_size_distribution`** : **distribution composite de convention adoptée comme référence ferme** (p50=256 B, p90=4 KB, p95=8 KB, p99=32 KB, max=64 KB). `min_blob_size = 4 KB` d'ADR-0017 §3bis confirmé (cohérent avec p90). Critère de réévaluation explicite : écart > 2× sur p90 entre W2 réel mesuré et convention déclenche amendement transparent (pas de migration de schéma — ADR-0017 §3bis). Décision tracée dans `benchmarks/reference-workload.md §emit-payload-distribution`, note ajoutée dans ADR-0017 (2026-05-16). **Conséquence T5** : T5 utilise des entrées 100 B fixes — note ajoutée dans test-protocol §6.1 indiquant que T5 ne couvre pas la distribution Q3 ; un benchmark T5-payload ou W2 dédié sera produit en Phase 6.

- [x] **C2 — Hardware serveur à qualifier** — **ABANDONNÉ 2026-05-27 (décision architect)** : les latences absolues mesurées sur Linux/NVMe ne sont pas portables sur seL4. Qualifier un PCIe Gen4 server-grade ne prédirait rien sur la stack de stockage seL4-native à venir. Borne conservative 14 agents/s (Modèle A, classe 2) retenue comme référence jusqu'à prototype seL4.

---

## Dettes stack technique — revue doc 2026-05-24

Revue de conformité à la documentation officielle conduite par 4 agents spécialisés (RocksDB, Wasmtime, Tokio, allocateur) après la découverte que plusieurs comportements memoriels étaient documentés mais inconnus. Toutes les dettes ci-dessous auraient pu être évitées par lecture préalable de la doc.

### P1 — CRITIQUE · RocksDB : incohérence `optimize_level_style_compaction` + `set_write_buffer_size`

- **Fichier** : `poc/causal-log/src/lib.rs` (même pattern dans `open_no_autocompact`)
- **Problème** : `optimize_level_style_compaction(512 MB)` configure un ensemble cohérent de paramètres interdépendants (dont `max_bytes_for_level_base`) pour une memtable de 128 MB. L'appel suivant `set_write_buffer_size(64 MB)` écrase `write_buffer_size` seul, laissant `max_bytes_for_level_base` dimensionné pour 128 MB. L1 est 2× surdimensionné → compactions L0→L1 retardées → stalls plus longs.
- **Impact direct** : cause probable des stalls T5-ter Mode B (L0 files ≫ trigger)
- **Fix** : supprimer `set_write_buffer_size(64 MB)` et laisser `optimize_level_style_compaction` configurer l'ensemble. Ou supprimer `optimize_level_style_compaction` et tout configurer manuellement avec des valeurs cohérentes.
- [x] Corriger `poc/causal-log/src/lib.rs` (les deux variantes `open` et `open_no_autocompact`) *(2026-05-24)*
- [x] Ouvrir ADR-0035 documentant les valeurs retenues et leur interdépendance *(2026-05-24)*

### P2 — IMPORTANT · RocksDB : `bytes_per_sync` et `wal_bytes_per_sync` manquants

- **Fichiers** : `poc/causal-log/src/lib.rs`, `poc/store/src/lib.rs`
- **Problème** : sans ces options, l'OS accumule les dirty pages et les flushe en rafale (piloté par `vm.dirty_ratio`). Ces rafales s'additionnent au fsync WAL et gonflent les p99 avec du bruit non attribuable à RocksDB.
- **Impact direct** : mesures T5-bis p99 non fiables (pics OS dirty-flush mélangés aux pics RocksDB)
- **Fix** : ajouter `opts.set_bytes_per_sync(1_048_576)` et `opts.set_wal_bytes_per_sync(1_048_576)` dans les deux bases
- [x] Corriger ContentStore et CausalLog *(2026-05-24)*
- [x] **T5-bis replay** *(résolu 2026-05-25)* — K=3 runs post-fix P1+P2 (ADR-0035). R1 cache froid = 18 059 µs (reboot, DB froide 41 GB — régime non comparable). R2–R3 cache chaud = **5 382 / 4 538 µs** (×4 sous cible 20 ms). La progression p99 RB1→RB3 pré-fix (×4,94) ne se reproduit pas post-fix : P1+P2 ont atténué la variance p99 en régime cache-chaud. Voir `results/T5-bis/SYNTHESE.md §Replay post-fix`.

### P3 — IMPORTANT · Allocateur : glibc par défaut + formule rss_adj incomplète

- **Problème 1** : l'allocateur Rust est glibc ptmalloc2 (aucun `#[global_allocator]`). glibc retient les pages libérées dans ses arenas — RSS ne redescend pas immédiatement après les frees. Ce comportement est documenté et configuré par `M_TRIM_THRESHOLD`.
- **Problème 2** : `rss_adj` ne soustrait toujours pas le block cache du CausalLog (256 MB constant). C'est un plancher RSS structurel qui apparaît comme "croissance" dans les mesures.
- **Fix A** : ajouter `tikv-jemallocator` dans `poc/benchmarks/Cargo.toml` pour les binaires de benchmark (jemalloc retourne les pages via MADV_FREE après decay_time=10s, rendant le RSS Rust prévisible)
- **Fix B** : corriger la formule dans le harness pour inclure le block cache constant : `rss_adj = RSS − memtable_store − memtable_log − block_cache_log (256 MB)`
- [x] Ajouter tikv-jemallocator dans benchmarks *(2026-05-24)*
- [x] Corriger la formule rss_adj dans `poc/benchmarks/src/main.rs` *(2026-05-24)*

### P4 — MOYEN · RocksDB : ContentStore sans bloom filter ni block cache configuré

- **Fichier** : `poc/store/src/lib.rs`
- **Problème** : `ContentStore` était ouvert avec `Options::default()` — pas de bloom filter, block cache par défaut = 8 MB par CF. Chaque point lookup (`rollback_path`) pouvait faire plusieurs I/O inutiles sur données froides.
- **Fix** : ajouter `BlockBasedOptions` avec `set_bloom_filter(10.0, false)` et un block cache explicite (256 MB, partagé avec CausalLog via P7)
- [x] Implémenter `BlockBasedOptions` dans `ContentStore::open` *(2026-05-25)*
- [x] Lié à P7 (cache partagé) *(2026-05-25)*

### P5 — MOYEN · RocksDB : CF `agent_ts` sans bloom filter de préfixe

- **Fichier** : `poc/causal-log/src/lib.rs`
- **Problème** : la CF `agent_ts` avait un `prefix_extractor` de 16 B mais pas de bloom filter de préfixe. Chaque `query_by_agent_range` devait traverser les index de tous les SSTs qui pourraient contenir le préfixe.
- **Fix** : `BlockBasedOptions` avec `set_bloom_filter(10.0, false)` + cache partagé sur la CF `agent_ts`
- [x] Corriger `poc/causal-log/src/lib.rs` *(2026-05-25)*

### P6 — MOYEN · RocksDB : `cache_index_and_filter_blocks_with_high_priority` manquant

- **Fichier** : `poc/causal-log/src/lib.rs`
- **Problème** : `set_cache_index_and_filter_blocks(true)` configuré mais pas `_with_high_priority`.
- **Fix** : N/A — `rocksdb_block_based_options_set_cache_index_and_filter_blocks_with_high_priority` n'est pas exposé dans librocksdb-sys 0.16 (rocksdb 0.22). La valeur par défaut C++ RocksDB 8.10 est déjà `true`. Comportement correct sans code supplémentaire. Commentaire ajouté dans `open()`.
- [x] Vérification doc effectuée *(2026-05-25)* — aucune action requise

### P7 — MOYEN · RocksDB : block caches non partagés entre les deux bases (ADR-0034 D3)

- **Problème** : CausalLog avait un cache LRU de 256 MB, ContentStore le cache par défaut de 8 MB. Deux instances indépendantes se disputant la RAM sans coordination.
- **Fix** : `Cache` (rocksdb 0.22) derive Clone avec `Arc<CacheWrapper>` interne — partage direct sans `Arc<Cache>`. Les deux `open()` acceptent `Option<Cache>` ; callers co-résidents partagent un seul `Cache::new_lru_cache(256 MB)`. Budget RAM inchangé : 256 MB total coordonné vs ~280 MB fragmenté. ADR-0035 amendé.
- [x] Refactorer les deux `open()` pour accepter `Option<Cache>` *(2026-05-25)*
- [x] `pub use rocksdb::Cache` re-exporté depuis `os-poc-store` *(2026-05-25)*
- [x] Tous les call sites mis à jour (shared ou None) *(2026-05-25)*
- [x] ADR-0035 amendé §D5 *(2026-05-25)*

### P8 — FAIBLE · Tokio : commentaire S7 faux dans `scheduler.rs`

- **Fichier** : `poc/runtime/src/scheduler.rs` (ligne ~2)
- **Problème** : commentaire "overhead par acteur ≈ 400 bytes stack minimum". Valeur fausse de 2 ordres de grandeur.
- **Fix** : corrigé — overhead Tokio pur ≈ 64 bytes/tâche ; coût réel dominé par WASM linéaire ≥ 64 KiB. spec/07 §3.3 inchangé (section I/O Admission Control, non lié à ce commentaire).
- [x] Corriger commentaire *(2026-05-25)*

### P9 — FAIBLE · RocksDB : `max_background_jobs` non coordonné entre les deux bases

- **Problème** : CausalLog avait 4 jobs, ContentStore 2 (défaut). Pics I/O superposés lors des benchmarks combinés.
- **Fix** : `db_opts.set_max_background_jobs(4)` ajouté dans `ContentStore::open`
- [x] Aligner `max_background_jobs` dans ContentStore *(2026-05-25)*

### GAP-11 — IMPORTANT · `query_by_agent_range` : erreurs I/O silencieuses (`.flatten()`)

- **Fichier** : `poc/causal-log/src/lib.rs`
- **Problème** : le `.flatten()` sur l'itérateur RocksDB ignorait silencieusement les erreurs I/O. En cas d'erreur, la fonction retournait une liste tronquée sans signal.
- **Fix** : signature `-> Vec<ActionId>` → `-> Result<Vec<ActionId>, LogError>`. Remplacement du `.flatten()` par `item.map_err(LogError::Rocks)?`. 8 callers mis à jour.
- [x] Corriger `poc/causal-log/src/lib.rs` + tous les callers *(2026-05-27)*. Commit `c781974`.

### GAP-12 — IMPORTANT → doc · Seuils write-stall par défaut non documentés

- **Fichier** : `poc/causal-log/src/lib.rs` (les deux `open()`)
- **Problème** : absence de commentaire expliquant pourquoi les seuils `level0_slowdown_writes_trigger=20` et `level0_stop_writes_trigger=36` (défauts RocksDB) sont acceptés.
- **Fix** : commentaire ajouté dans les deux variantes `open()`.
- [x] Documenter *(2026-05-27)*. Commit `257a844`.

### GAP-13 — IMPORTANT · RocksDB : `target_file_size_base` trop grand

- **Fichier** : `poc/causal-log/src/lib.rs` (les deux `open()`)
- **Problème** : `target_file_size_base` = 64 MiB (défaut RocksDB) vs recommandation `max_bytes_for_level_base/10` = 25,6 MiB.
- **Fix** : `set_target_file_size_base(32 * 1024 * 1024)` (32 MiB = 256 MiB / 10, arrondi). ADR-0035 amendé §D8.
- [x] Corriger les deux variantes `open()` *(2026-05-27)*. Commit `257a844`.

### GAP-14 — FAIBLE · `min_write_buffer_number_to_merge` non documenté

- **Fichier** : `poc/causal-log/src/lib.rs` (les deux `open()`)
- **Problème** : valeur par défaut (1) non documentée — aucune fusion pré-flush.
- **Fix** : commentaire ajouté.
- [x] Documenter *(2026-05-27)*. Commit `257a844`.

### GAP-15 — FAIBLE · `pin_l0_filter_and_index_blocks_in_cache` absent sur CF `agent_ts`

- **Fichier** : `poc/causal-log/src/lib.rs` (les deux `open()`)
- **Problème** : option présente sur CF `default` mais absente sur CF `agent_ts`.
- **Fix** : `agent_ts_block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true)` ajouté. ADR-0035 amendé §D9.
- [x] Corriger les deux variantes `open()` *(2026-05-27)*. Commit `257a844`.

### P10 — FAIBLE · RocksDB : compression None sur tous les niveaux

- **Problème** : `DBCompressionType::None` sur tous les niveaux, y compris L2+. Pour des données froides (L2/L3), la compression LZ4 réduirait la taille des SSTs et diminuerait le I/O de compaction, sans coût sur le chemin d'écriture.
- **Fix** : `set_compression_per_level([None, None, Lz4, Lz4, Lz4, Zstd, Zstd])` sur toutes les CFs write-heavy
- **Évaluation** : T5-bis fermé avant activation → baseline H-causal-latence préservée. SSTs existants lisibles sans reconversion (RocksDB décode l'ancien format). L0/L1 restent None → chemin d'écriture inchangé. Futurs benchmarks incluront l'effet compression (bénéfique).
- [x] Activer sur CausalLog (default + agent_ts) et ContentStore (blocks + headers) *(2026-05-25)*

---

## Sécurité — dettes ouvertes (post spec/08 v1.2 + ADR-0036)

- [x] **SEF-7.1 — forgerie causale refusée** *(résolu 2026-05-25)* : test `sef7_1_forged_action_id_rejected` dans `poc/runtime/src/lib.rs`. Agent forge `[0xDE;32]` (absent du log) → `agent_add_cause` retourne -3 (fail-closed), `pending_extra_causes` vide, `parent_ids` du `LogEntry` suivant contient uniquement le parent séquentiel.
- [x] **SEF-7.2 — flood `pending_extra_causes`** *(résolu 2026-05-25)* : test `sef7_2_extra_causes_flood_bounded` dans `poc/runtime/src/lib.rs`. 16 causes valides pré-injectées + 17ᵉ appel WAT → retour -2 (MAX_EXTRA_CAUSES=16 atteint), `parent_ids` contient exactement 16 causes + 1 parent séquentiel = 17, le 17ᵉ est absent.
- [x] **SEF-7.3 — robustesse reconstructeur** *(résolu 2026-05-25)* : `poc/reconstruct/src/main.rs` — boucle sur `entry.parent_ids` avant affichage ; `log.get(parent_id)` → `Ok(None)` émet `WARN: parent_id ... introuvable dans le log (DAG incomplet)` ; jamais de panic.
- [x] **Mesure `log.get()` sous writes concurrents (P3c)** *(résolu 2026-05-25)* : benchmark `t5-p3c` dans `poc/benchmarks/src/main.rs`. N_WRITERS threads concurrent append() + mesure get() sur N_READS lookups. Cible p99 < 200 µs. Usage : `cargo run -p os-poc-benchmarks --release -- t5-p3c [N_PREPOP [N_WRITERS [N_READS]]]`. Résultats → `results/T5-p3c/<ts>/verdict.json`.
- [x] **W1 — `agent_introspect` : panique host sur out_ptr hors bornes *(résolu 2026-05-27)*** : ajout d'un `checked_add` + comparaison `data_size` avant `memory.write`. Retourne -2 si le buffer fourni est trop court. Commit `c621dfb`.
- [x] **W2 — `agent_session_info` : panique host sur out_ptr hors bornes *(résolu 2026-05-27)*** : même correctif que W1 (SESSION_INFO_LEN = 24 bytes). Commit `c621dfb`.
- [x] **W3 — `agent_infer` : `let _ = mem2.write(...)` silencieux → divergence log/état *(résolu 2026-05-27)*** : pré-validation des pointeurs output en Phase 1 (sync, avant l'appel async LLM) via `checked_add`. Toutes les branches (Ok/Cancelled/Error) utilisent `.expect("bounds vérifiées en Phase 1")`. Retourne code 2 (Error) si hors bornes — documenté dans ADR-0019. Commit `37a9efd`.
- [x] **T9 (auto-citation hors session) — DOCUMENTÉ, clôturé *(2026-05-27)*** : un agent peut citer ses propres actions d'une session antérieure via `agent_add_cause`. B-light vérifie l'existence, pas la fenêtre cognitive. **Ce n'est pas un exploit** — c'est une propriété du modèle : le DAG causal implémente *happened-before* (Lamport 1978), pas la connaissance effective (Halpern & Moses 1990). Contrat sémantique ajouté dans `spec/08 §0.1`. Un superviseur ne doit pas inférer "B avait connaissance de A" depuis `parent_ids` seul. Aucun changement de code ni SEF requis.
- [ ] **B-1 (upgrade Wasmtime 25 → ≥36.0.7/≥42.0.2/≥43.0.1) — DORMANT, ne pas instruire.** Décision architecte 2026-06-05 (`red-team/campagne-B-substrat/FINDING-B-1.md` §Décision architecturale). RUSTSEC-2026-0096 (CVSS 9.0, sandbox escape Cranelift aarch64) est **N/A par configuration** : aarch64-only ET memory64-only, et `wasm_memory64` reste désactivé (garde fail-closed `memory64_reste_desactive`, `poc/runtime/src/lib.rs:90`). Les jalons seL4 sont **clos** (ADR-0049 §D1, gel ADR-0047 §D1) → un upgrade ne pourrait cibler que Linux x86-64, où **aucun CVE n'est atteignable** (le critique est aarch64-only ; ne reste que RUSTSEC-2026-0087 CVSS 4.1, conditionnel à `f64x2.splat` non émis). `cargo search` 2026-06-05 : stable upstream = 45.0.0, fix disponible — non bloquant côté upstream, mais **aucun bénéfice** vs coût de migration des API async/epoch/Memory utilisées (`func_wrap_async`, `call_async`, `instantiate_async`, `epoch_interruption`, `into_memory`, `Module::deserialize_file`). **Pas d'ADR** (N/A par configuration ≠ décision, cf. ADR-0049 §D3c). **Triggers objectifs de réveil** : (1) échec du test `memory64_reste_desactive` = `wasm_memory64` activé ; (2) dégel d'un jalon seL4 pour toute autre cause (greffer l'upgrade) ; (3) émission de `f64x2.splat` par un agent (RUSTSEC-2026-0087 atteignable sur Linux). Sujet clos tant qu'aucun trigger n'est armé.
- [x] **B-fort (multi-tenant) — RÉVEILLÉ puis RÉSOLU le 2026-06-07.** Posé DORMANT le 2026-05-26 (architect) avec trigger « première PR introduisant un second `TenantId` distinct ». **Ce trigger a été tiré** : le chantier multi-tenant + B-fort (en tête de fichier) a introduit ≥2 tenants (MT-1) puis livré B-fort complet (BF-0→BF-3 : `CauseHandle` object-capability obligatoire, cycle de vie révocation/rollback, robustesse adversariale ; décomposition Scheduler SD-0→SD-2 ; révocation cross-tenant XR-0/XR-1). ADR engendrés : **0057** (forme multi-tenant), **0058** (modèle B-fort), **0059** (SupervisionAuthority), **0060** (révocation cross-tenant). **137/137 tests lib PASS.** L'ADR-futur prévu ici (Cap<T> typé, ownership cross-agent, tests d'isolation inter-tenant) est réalisé sous une forme révisée (`CauseHandle` sur action_id plutôt que Cap<T> générique — cf. ADR-0058 §D1). La consigne YAGNI « ne pas pré-câbler » a été respectée (aucune structure spéculative).

---

## Phase 8 — Transition seL4 (2026-05-27)

Décision architect du 2026-05-27 : le PoC Linux a validé P1–P6. Le travail restant sur Linux (D9, RocksDB) ne prédit rien sur la stack seL4-native. Pivot vers seL4.

- [x] **spec/09 — Tableau de transfert PoC → seL4** *(2026-05-27)* : 36 ADRs classifiés — A (18, transfert intégral), B (12, concept transféré / implémentation à réécrire), C (4, méthodologie), D (2, non portables : ADR-0011 et ADR-0035 RocksDB-spécifiques). 3 questions bloquantes identifiées : Q-seL4-1 (runtime), Q-seL4-2 (store), Q-seL4-3 (densité).

- [x] **ADR-0037 — Stack runtime seL4** *(2026-05-27)* : Wasmtime `min-platform` (no_std, Cranelift) + executor Rust async minimal maison, sans Genode. N agents dans 1 VSpace seL4 (isolation S1b via sandbox WASM). PoC de fumée validé : RSS +5 MB, latence appel host 0.065 µs, `Module::deserialize()` 0.038 ms. Commit `819ebe6`.

- [x] **ADR-0038 — Store natif seL4** *(2026-05-27)* : Q-seL4-2 tranchée. Interface : ring buffer mémoire partagée + 1 IPC `seL4_Call` de commit. Durabilité niveau (1) — acquittement serveur RAM (≡ ADR-0027 SIGKILL). Atomicité P6 : Q3-C content-addressed + log atomique. Interface `StoreServer` spécifiée. **Phase 8 résolue** : RAM disk pur + HashMap 10⁶ entrées (B2/B3 éliminés du scope Phase 8). **Phase 9 ouverte** : B2 (driver block — voies i/ii/iii) + B3 (moteur index 10⁸ entrées). spec/08 §0.2 ajouté (politique C dans TCB, option α).
  - [x] **B2 Phase 9** *(2026-05-28)* : revue `seL4/rust-sel4` (trouvé `sel4-virtio-blk` 30 LOC no_std dans rev `7a2321f2`), sDDF/blk C (~593 LOC virtio, incompatible microkit), voie (iii) retenue. → ADR-0041.
  - [x] **B3 Phase 9 — Benchmark P3a redb** *(2026-05-28)* : `poc/redb-p3a/` — 10⁸ entrées (clé u64, valeur 100 B) sur NVMe, K=3 passes 10 000 get() aléatoires. **PASS.** p99 : 739 / 581 / 572 µs. p99 pire cas 739 µs, ×13 sous cible 10 ms, ×2 meilleur que RocksDB SEF-5 (1 368–1 850 µs). Taille DB : 23 GB (ratio 2.1× données brutes). Population : 301 s à ~340 000 inserts/s. Voir `poc/redb-p3a/results/redb-p3a/verdict.json`.

- [x] **ADR-0040 — Chemin seL4 : hyperviseur vs substrat natif** *(2026-05-28)* : **Chemin B (substrat natif) retenu** au déclenchement Phase 9 (§5.1.1). Justification §8 : (1) spec/08 §0.2 option α exclut Linux dans TCB OS (~30 MLOC viole cible <5 KLOC C) ; (2) déploiement mono-machine mono-tenant ne justifie pas le coût VM (≥ 64 MB/VM, impact P1) ; (3) si Chemin A était retenu, KVM/Xen seraient préférables à seL4 — rejet A consolide ADR-0037/0038/0039 ; (4) C.3 valide empiriquement la viabilité de B (`add(21,21)=42` sur Wasmtime 25 no_std seL4 AArch64) ; (5) précédent KataOS [Google 2022] confirme le pattern. Conséquence : ADR-0037/0038/0039 deviennent décisions de cible finale, plus seulement de PoC. Variante "sDDF C dans TCB" reste éligible **dans** Chemin B (voie ii de B2).
  - [x] **ADR-0041 — Voie B2 driver block** *(2026-05-28)* : **voie (iii) retenue** — `sel4-virtio-blk` (Rust no_std, 30 LOC, rev `7a2321f2`, `virtio-drivers 0.13.0`). Voie (ii) sDDF rejetée (incompatibilité microkit vs rust-sel4 root task). Voie (i) NVMe rejetée (QEMU = virtio, effort 6–9 mois inutile). Voir `decisions/0041-voie-b2-driver-block.md`.
    - [x] **C.4 — Driver block seL4 PoC** *(2026-05-28)* : `poc/sel4-hello/c4-virtio-blk/` — `sel4-virtio-hal-impl` + `virtio-drivers 0.13.0` (pas de microkit, API root task pure). DMA 64 KB (16 SmallPages) depuis le plus grand Untyped non-device mappé à `0x1000_0000`. MMIO range `0x0a000000..0x0a004000` (4 pages) mappé à `0x2000_0000`. Scan 32 slots : slot 31 (= `0x0a003e00`) = Block device (QEMU virt AArch64 assigne le 1er device virtio au slot 31). `VirtIOBlk` read/write/relecture bloc 0 (round-trip `0xC4 0x04` confirmé). `C4_PASS` reçu. Correction clé : utiliser `max_by_key(size_bits)` (pas `min_by_key`) pour le pool Untyped non-device — le plus petit Untyped ≥ 64 KB est épuisé après les DMA frames, les PTs échouent. Voir `poc/sel4-hello/c4-virtio-blk/`.
  - [x] **ADR-0042 — Voie B3 moteur d'index** *(2026-05-28)* : **redb fork no_std retenu.** P3a validé (p99=739 µs, ×13 sous cible). Voir `decisions/0042-voie-b3-moteur-index.md`.

- [x] **C.5-A — Fork redb no_std** *(résolu 2026-05-29)* : `poc/redb-fork/` — portage redb 4.1.0 vers `#![no_std]` + `extern crate alloc`. Remplacements : `std::io::Error` → `compat::io::Error` (type custom), `std::sync::*` → `spin::*`, `HashMap/HashSet` → `BTreeMap/BTreeSet` alloc (hashbrown 0.14 incompatible avec `-Z build-std=alloc` → E0464), `thread::panicking()` → `false`, `file_backend` supprimé. Compile sur `wasm32-unknown-unknown` ET `aarch64-sel4` (nightly-2026-03-18). Corrections supplémentaires : `.drain()` → `core::mem::take`, `.shrink_to_fit()` supprimés (BTreeMap), `FastHashMapU64/PageNumberHashSet` → type aliases BTreeMap/BTreeSet, `pub use compat::io` pour les implémenteurs externes.

- [x] **C.5-B — `BlockStorage: StorageBackend` sur virtio-blk** *(résolu 2026-05-29)* : `poc/sel4-hello/c5-redb-on-virtio/`. `BlockStorage` : RMW non-aligné (redb fait des accès sous-secteur ex. 320 B pour son header), `sync_data` no-op (durabilité niveau 1 ADR-0038), `unsafe impl Send/Sync` (root task single-threaded), `MmioTransport<'static>` via transmute (MMIO fixe). disk.img 8 MB. heap_size 8 MB. Cache redb 1 MB. **Verdict : `C5_PASS`** — N=1000 insertions + 100 vérifications intégrité. Voir `poc/sel4-hello/c5-redb-on-virtio/`.
  - ⚠️ **Dette identifiée (revue 2026-05-29)** : C.5 a câblé redb store-direct sur virtio-blk → inverse l'invariant ADR-0038 §3 (index reconstructible non-autoritaire) et le modèle no-force d'ADR-0027. C5_PASS = capacité de brique, PAS validation P6. Corrigé en C.6. Voir ADR-0042 §Amendement, LESSONS.md L68.

- [x] **ADR-0043 — Intégration verticale C.6 : topologie 2-processus + validation P6** *(tranché 2026-05-29)* : séparation runtime/serveur de stockage (2 VSpaces, ring partagé, seL4_Call), correction de la topologie C.5. Crash P6 par `tcb_suspend` aux 4 kill_points Q3-C ; oracle dans le serveur survivant (invariants I1/I2/I3) ; portée bornée mono-agent (re-validation P6 obligatoire en C.7). Découpage en 2 jalons. Faits API rust-sel4 vérifiés (pas de helper spawn, `spawn-task` = squelette, `endpoint.call` bloquant). Voir `decisions/0043-integration-verticale-c6.md`.
  - [x] **Prérequis C.6 dérisqués** *(2026-05-29)* : (#1) signatures rust-sel4 confirmées sur clone non-sparse rev `7a2321f2` — bifurcation `tcb_configure` MCS résolue : image `KERNEL_MCS = false` → variante avec `fault_ep`. (#2) footprint serveur mesuré : 121 pages code (~483 KB) + heap configurable (8 MB en C.5 → 2065 pages) ; risque CNode racine maîtrisable. Voir ADR-0043 §Étapes #1/#2, LESSONS.md L69.
  - [x] **C.6 — intégration verticale nominale** *(2026-05-29)* : `poc/sel4-hello/c6-integration/` — 3 crates (supervisor root task, server VSpace B, runtime VSpace A). Ring buffer 1 granule partagé (copie cap), endpoint seL4 call/reply, agent WASM sans mémoire linéaire (évite réservation VA 8 GB Wasmtime). Leçon clé : `seL4_Call` nécessite droit **GrantReply** sur l'endpoint cap (pas seulement Write) — `CapRights::read_write()` insuffisant, `CapRights::all()` requis. **`C6_PASS`** reçu sur UART QEMU. Voir `poc/sel4-hello/c6-integration/`.
  - [x] **C.6-crash — validation P6** *(résolu 2026-05-29)* : `poc/sel4-hello/c6-crash/` — KP1–KP4 instrumentés via `tcb_suspend` self-suspension + `suspend_nfn` signal, oracle dans le serveur survivant (endpoint badgé 0xCAFE), invariants I1/I2/I3 assertés par le superviseur. `parse_kill_point` const fn (contournement : match str en const context non supporté nightly-2026-03-18). **`C6-crash_PASS`** reçu — KP1_PASS, KP2_PASS, KP3_PASS, KP4_PASS. Voir `poc/sel4-hello/c6-crash/`.

- [x] **ADR-0045 — Critère de complétude PoC seL4** *(2026-05-29)* : Q1=B (done = chaîne commit persistante end-to-end + P3a re-validée redb/virtio-blk) ; Q2=α (power-loss hors scope, durabilité niveau 1 only). Matrice P1–P6/I4 par substrat établie. Voir `decisions/0045-critere-completude-poc-sel4.md`.

- [x] **C.8 — Intégration store persistant seL4** *(2026-05-29)* : `poc/sel4-hello/c8-store/` — serveur avec backend redb sur virtio-blk. Chaîne `runtime → ring → seL4_Call → serveur → redb → virtio-blk` end-to-end. Topologie C.7-crash + init IPC (badge=0xC8_0000, passe dma_paddr). P6 re-validée : KP1-4 (I3-N + I4), oracle badge=0xC8FE, seq_(a,b) confirmés. P3a : redb benché sur NVMe (poc/redb-p3a, p99=739µs ×13 sous cible) — la chaîne seL4 valide le fonctionnement end-to-end, pas la latence absolue sur QEMU. **C8_PASS** reçu (KP1-4 tous PASS). Corrections clés : (L77) retry-loop pour PT niveau 3 au lieu de `map_intermediate_translation_tables` ; (L78) DMA en premier pour paddr = ut_paddr. Voir `poc/sel4-hello/c8-store/`, LESSONS L77-L78.

- [x] **ADR-0044 — Intégration verticale C.7 : N agents, dispatch badge, serveur séquentiel, I4** *(tranché 2026-05-29)* : N TCB VSpace partagé (pas executor async, différé) ; badge = agent_id pur (kind dans label MessageInfo) ; serveur séquentiel ; invariants I3-N (par agent) + I4 (non-interférence d'intégrité Biba, pas Goguen-Meseguer) ; amende ADR-0038 §Q4 (badge vs payload Record inchangé) ; précise ADR-0037 §3 (reactor différé, scheduler ADR-0030 hors-scope C.7) ; découpage C.7-A / C.7-crash. Footprint mesuré 2026-05-29 : runtime C.6 = 814 pages (2 MB heap Wasmtime) ; serveur C.6 = 90 pages (256 KB heap) ; 2 runtimes + 1 serveur = ~1718 pages ELF frames < 4096 slots CNode → risque maîtrisable. Voir `decisions/0044-integration-verticale-c7.md`.
  - [x] **C.7-A — intégration nominale N agents** *(résolu 2026-05-29)* : `poc/sel4-hello/c7-integration/` — 2 TCB runtimes (VSpaces séparés, même ELF), 2 rings SPSC, 2 commit-caps badgées (badge=AGENT_A_ID=1 / AGENT_B_ID=2, `CapRights::all()`), serveur badge-dispatch par `(badge == AGENT_A_ID → ring[0], badge == AGENT_B_ID → ring[1])`, index `journal_per_agent: BTreeMap<u64, Vec<[u8;32]>>`. `child_vspace.rs` généralisé : `&[sel4::cap::Granule]` (N rings consécutifs après IPC buffer). Footprint : 2 runtimes = 1628 pages + server = 90 pages < 4096 slots CNode. **`C7-A_PASS`** reçu — agent=1 seq 0→1, agent=2 seq 0→1 (commits distincts, index per-agent correct). Voir `poc/sel4-hello/c7-integration/`.
  - [x] **C.7-crash — validation P6-N + I4** *(résolu 2026-05-29)* : `poc/sel4-hello/c7-crash/` — runtime A instrumenté (KILL_POINT=1-4) + runtime B nominal (KILL_POINT=0), même binary paramétré. Server badge-dispatch + oracle (badge=0xC7FE → seq_a, seq_b). Assertions I3-N (KP1/2/3 → seq_a=0=k-1 ; KP4 → seq_a=1=k) + I4 (seq_b=1 dans tous les cas, non-interférence d'intégrité). **`C7-crash_PASS`** reçu — KP1_PASS (seq_a=0,seq_b=1), KP2_PASS (seq_a=0,seq_b=1), KP3_PASS (seq_a=0,seq_b=1), KP4_PASS (seq_a=1,seq_b=1). Voir `poc/sel4-hello/c7-crash/`.

- [x] **PoC seL4 sur QEMU AArch64** *(semaines 2–3 — ADR-0039, résolu 2026-05-28)* : cible **QEMU `virt` AArch64** (portage x86_64 différé à Phase 9). Séquence en 3 jalons distincts :
  - [x] **C.1 — Hello world officiel** *(2026-05-28)* : `seL4/rust-root-task-demo` + `docker run` + `qemu-system-aarch64`. `TEST_PASS` reçu sur UART QEMU. seL4 15.0.0 + toolchain nightly-2026-03-18 + Docker + QEMU validés. Voir `poc/sel4-hello/c1-hello/run-c1.sh`.
  - [x] **C.2 — Root task custom minimale** *(2026-05-28)* : `poc/sel4-hello/c2-root-task/` — print custom + accès `bootinfo.untyped_list()` (69 régions) + retype 1 Untyped → SmallPage (4 KB, AArch64). `C2_PASS` reçu. Corrections par rapport au squelette : `rustflags = ["-Zunstable-options"]`, `exe-suffix .elf` dans cp, `ObjectBlueprintArch` (pas `ArchObjectBlueprint`), `init_thread::slot::CNODE` (pas `init_thread_cnode()`), `Slot::<Unspecified>::from_index`. Rev rust-sel4 épinglé : `7a2321f2`. Agent `sel4` créé.
  - [x] **C.3 — Intégration Wasmtime** *(2026-05-28)* : `poc/sel4-hello/c3-wasmtime/` — Wasmtime 25.0.3 `runtime` (no_std, sans Cranelift) + module `add(i32,i32)->i32` pré-compilé AOT en `build.rs` (Cranelift x86_64 → cwasm AArch64-unknown-unknown), embarqué via `include_bytes!`, désérialisé par `Module::deserialize`. 13 fonctions plateforme Wasmtime implémentées : pool de 64 pages RWX à 0x1000_0000 (retype Untyped + frame_map/pt_map), bump allocator, mprotect/munmap/mmap_remap no-op, setjmp/longjmp minimal. `add(21, 21) = 42`. `C3_PASS` reçu. Corrections clés : `heap_size = 4 MB` (16 MB → CNode 4096 caps épuisé), page 0x10000000 nécessite 3 PT intermédiaires, cible AOT `aarch64-unknown-unknown` (OS=Unknown = aarch64-sel4 côté runtime). Voir `poc/sel4-hello/c3-wasmtime/`.

---

## Benchmarks en attente

- [x] **T5-qualif hardware serveur** — **ABANDONNÉ 2026-05-27 (décision architect)** : idem C2 — les chiffres p99 RocksDB/Linux ne se transfèrent pas sur seL4. Les deux classes couvertes (AWS i3en.xlarge + AMD/SN530) suffisent à falsifier H-causal-latence pour la cible PoC. Reprendre la qualification performance uniquement sur prototype seL4-natif.

- [x] ~~**T6-qualif** — K≥3 runs sur NVMe + baseline Docker réaliste → "partiellement validé"~~ — **résolu 2026-05-22** (déjà coché dans Axe 1). Entrée conservée pour traçabilité, voir `results/T6/SYNTHESE.md`.

---

## Phase 9 — Consolidation persistance seL4 (ouvert 2026-05-29)

Décision architect du 2026-05-29 (ADR-0046) : Phase 9 = consolidation de la persistance seL4 démontrée. Power-loss (β) renvoyé Phase 10+.

- [x] **C.9 / D-reopen — Smoke test persistance seL4** *(2026-05-29)* : `poc/sel4-hello/c9-reopen/` — deux runs QEMU sur le même `disk.img` (pas de `dd` entre les deux). Phase A : K=100 commits redb/virtio-blk (runtime Rust pur, sans Wasmtime) → `REOPEN_A_PASS`. Phase B : server reopen `create_with_backend` sur disk.img existant → `verify_k_commits()` → `verified=100, seq_a=100, K=100` → **`C9_PASS`**. Correction clé : `BlockStorage::new_reopen` initialise `logical_len = capacity_bytes` (vs 0) pour que redb lise le header existant au lieu de créer une DB fraîche. Voir `poc/sel4-hello/c9-reopen/`, LESSONS.md L81.

- [x] **D-P3a — PASS correction** *(2026-05-30)* : `poc/sel4-hello/d-p3a/` — 3000/3000 lookups corrects (K=3 × M=1000) sur N=10^6 entrées redb/virtio-blk sous seL4. **Correction PASS. Latence N/A** : `CNTVCT_EL0` non accessible EL0 sur seL4 EL2 (CNTHCTL_EL2 non configuré ; seL4_DebugGetClock absent des bindings Rust 7a2321f2). Latence de référence : Linux/NVMe 739 µs p99 (×13 sous cible). Voir `poc/sel4-hello/d-p3a/VERDICT.md`.

- **GC orphelins redb** *(optionnel — déclencher si D-reopen révèle croissance non bornée)* : différé conforme ADR-0038 §Q3.
- **N > 2 agents dynamiques** *(optionnel — déclencher sur besoin concret)* : différé conforme ADR-0044 D1.
- **Power-loss / β** *(Phase 10+ — hors scope Phase 9)* : exige harnais kill-QEMU + modèle de sémantique barrière virtio-blk + ADR dédié. Conforme ADR-0045 Q2=α, ADR-0038, ADR-0027.

### Dettes soundness seL4 — revue C.1→C.9 (2026-05-29)

Revue exhaustive axée soundness seL4 + conformité ADR. C1/C2 (conformité ADR) tracés en amendements (ADR-0038, ADR-0043, ADR-0046) + L82. Dettes techniques ci-dessous :

- [x] **S1 — MOYEN · Violation W^X sur le pool JIT Wasmtime** *(levée par C.10 — 2026-05-29)* : `poc/sel4-hello/c10-wx/runtime/src/platform.rs` — pool JIT sorti du `.bss`, 128 frames dédiées pré-mappées RW+EXECUTE_NEVER par le superviseur, `wasmtime_mprotect` implémenté via frame_unmap+frame_map (W→X via `VmAttributes::EXECUTE_NEVER`). Test négatif : write sur page RX → vm fault seL4 observé. **C10_PASS.**

- [x] **S2 — FAIBLE · `CapRights::all()` sur endpoints (sur-privilège)** *(corrigé sur c9, 2026-05-29)* : les 3 endpoints sur lesquels on fait `.call()` (commit-cap runtime, init_ep, verify_ep dans `c9-reopen/supervisor/src/main.rs`) réduits de `all()` à `CapRightsBuilder::none().grant_reply(true).write(true)` — seL4_Call exige Write+GrantReply, pas Grant ni Read (L70). **C9_PASS revalidé** après changement. Caps **TCB** (self-suspend) conservées en `all()` : invocation TCB non rights-gated comme un endpoint, Grant sans effet, réduction sans bénéfice testable — documenté inline. Non propagé à c6→c8 (gel D1).

- [x] **S3 — FAIBLE · Erreurs avalées dans le chemin de commit** *(corrigé sur c9, 2026-05-29)* : (1) `runtime` lit désormais le `seq` renvoyé par le serveur dans `msg_regs[0]` après `ep.call()` et panique si `seq != i+1` (détection de divergence log/état end-to-end) ; (2) `server` `commit_to_redb` émet un `debug_println` sur payload invalide (`data_len==0 || >4092`) au lieu d'un `return 0` silencieux. **C9_PASS revalidé.** N'a PAS changé le format de protocole figé (ADR-0043 §71) — utilise le `seq` déjà présent dans la réponse. Reste à faire si le protocole évolue (N agents, payloads variables) : propager une `StoreError` typée via le label de réponse. Non propagé à c6→c8 (gel D1).

- [~] **S4 — FAIBLE · Tout trap WASM = panic runtime** : atténuée par C.11 (isolation processus démontrée sous trap réel, ADR-0048 §D3). `wasmtime_longjmp` panique toujours → crash du runtime ; le serveur survit, D-reopen validé. Reste à lever quand N agents / VSpace dynamiques : trap d'un agent ne doit pas tuer un runtime multi-agents. À traiter si architecture multi-agents dans un seul processus runtime.

- [~] **S5 — FAIBLE/doc · Commentaires cap-layout contradictoires** *(wont-fix : jalon figé)* : `c7-crash/runtime/src/main.rs:17` dit `CapRights::all()`, `:36` dit `read_write + grant_reply` pour le même slot 1. c7-crash est un artefact figé (décision D1) → non corrigé. Le commentaire d'en-tête de c9 ne porte pas cette contradiction (pas de mention de rights). Le défaut reste dans c7-crash gelé ; sera évité dès la crate commune (c10+).

- [x] **S6/S7 — INFO · Justifiés, à revalider hors QEMU** : 3× `transmute` MmioTransport→'static (justifié : MMIO à VA fixe résidente) ; DMA mappé cacheable (cohérence déléguée au HAL — à revalider sur HW réel, pas QEMU) ; munmap/mmap_remap/memory_image_* no-op + bump allocator → instanciations WASM bornées par `POOL_PAGES=128`. Pas d'action PoC ; revalidation conditionnée à un substrat matériel réel (même déclencheur que D-P3a).

- [x] **D1 — Duplication ~6× du code seL4** *(levée par C.10 — 2026-05-29)* : crate commune `poc/sel4-hello/sel4-common/` créée (`child_vspace.rs` + `object_allocator.rs`). c9 y migre (`sel4-common = { path = "../../sel4-common" }`). c10 la consomme. Jalons c6→c8 conservés figés (gel ADR-0046). Résidu : `platform.rs` toujours distinct par jalon (architecture de caps différente) — acceptable.

---

## Phase 10 — Durcissement runtime W^X + (futur) power-loss (ouvert 2026-05-29)

Ouverte par ADR-0047 (déclencheur T1 : WASM non confié prévu court terme). Contient aussi le power-loss/β renvoyé ici par ADR-0046.

- [x] **C.10 — W^X du pool JIT Wasmtime (option B)** *(C10_PASS — 2026-05-29)* : jalon vivant `poc/sel4-hello/c10-wx/`. Pool JIT hors `.bss` (128 frames dédiées), `wasmtime_mprotect` unmap+remap (W→X via `VmAttributes::EXECUTE_NEVER`), CNode runtime size_bits=8, crate commune `poc/sel4-hello/sel4-common/`. Test négatif : vm fault à 0x40010000 observé. D-reopen : C10_REOPEN_PASS. Voir ADR-0047, LESSONS L84.

- [x] **C.10-crash — atomicité crash sous remap W→X** *(C10_CRASH_PASS — 2026-05-29)* : kill-point dans la fenêtre de remap RW→RX (page transitoirement ni W ni X entre unmap et remap). Valide qu'un crash dans cette fenêtre laisse un état récupérable. Régime crash-processus α. `poc/sel4-hello/c10-crash/` — pattern suspend_nfn + oracle query (seq_a=1=K). D-reopen : C10_CRASH_REOPEN_PASS. ADR-0047 §D7.

- [x] **C.11 — chargement de module WASM non confié sur JIT durci** *(C11_PASS — 2026-05-29)* : `poc/sel4-hello/c11-untrusted/`. Deux runtimes WASM non confiés exécutés séquentiellement sur le JIT W^X (ADR-0047) : P-α module `unreachable` (commit → trap → crash → fault_ep seL4) ; P-β module boucle infinie (commit → watchdog tcb_suspend externe). D-reopen : verified=2, seq_a=2 → **C11_PASS**. Voir ADR-0048.

- [x] **C.11-prov — axe provenance : `.cwasm` depuis canal non-trusted** *(C11PROV_PASS — 2026-05-30)* : `poc/sel4-hello/c11-prov/`. Sous-jalon ADR-0048 §D1. `provision_bytes_into_vspace` ajouté à `sel4-common` (écriture page par page via scratch free_page_addr). Runtime lit les bytes cwasm depuis `MODULE_VA_BASE=0x5000_0000` provisionné par le superviseur (format `[len: u64 LE][data...]`). P-δ : 32 octets `0xDE` → `Module::deserialize Err` → `ready_nfn` → superviseur observe → **aucun VM fault** (pas d'exécution d'octets arbitraires). Happy path : cwasm valide → commit → oracle seq_a=1. D-reopen : K=1 vérifié. `C11PROV_DELTA_PASS + C11PROV_VALID_PASS + C11PROV_GAMMA_PASS` → **C11PROV_PASS**. Voir LESSONS L86.

- [x] **Clôture du PoC seL4** *(ADR-0049 — 2026-05-30)* : déclencheurs objectifs épuisés (C.10/C.11/C.11-prov soldés ; items restants sans déclencheur atteint). PoC clos au sens ADR-0045 Q1=B + Phase 9 (D-reopen) + durcissements. **Inscription au récit de complétude** : la séparation « CAS autoritaire / index reconstructible » (ADR-0038 §3, ADR-0042) est une **cible non instanciée** — le store réel est un store redb transactionnel monolithique (L82) ; les PASS C.6→C.11-prov attestent isolation 2-processus + P6 crash-processus + I4 + W^X + isolation WASM non confié, **pas** cet invariant. ADR-0045 §Pourquoi-pas-A amendé (retrait argument invariant ; conclusion B inchangée). Voir ADR-0049.

### Déclencheurs dormants seL4 — ne pas instruire sans réveil (ADR-0049 §D3)

- **(a) Instanciation séparation CAS/index** ← déclencheur : implémentation du GC des orphelins, lui-même déclenché par **croissance non bornée du store observée sur cycles reopen** (ADR-0046 §42). Le GC force mécaniquement la re-séparation (L82 corollaire). Sans GC réclamé : aucune propriété observable nouvelle, ne pas coder.
- **(b) Power-loss / β** ← déclencheur : **substrat média réel** (board / NVMe passthrough). Sur QEMU = validation trompeuse (ADR-0027 D3, ADR-0045 §54, ADR-0046 §60). Déclencheur matériel, pas décision de direction.
- **(C.12+) setjmp/longjmp réel, watchdog temporel, fuel-équité, signature** ← déclencheurs ADR-0048 §D6 : ≥ 2 agents par VSpace / SLA temps mur / réseau-PKI ou second producteur de modules. Aucun atteint.
- **N > 2 agents dynamiques** ← besoin concret (ADR-0044 D1). P6-N + I4 déjà validés à N fixe.

### Prochaine direction — remontée spec (ADR-0049 §D4)

- [x] **Consolidation / synthèse de transfert** *(2026-05-30)* : `spec/09` consolidé après clôture. Q-seL4-1/2/3 marquées RÉSOLU (avec ADR + verdict + dette Q-seL4-2 partielle) ; §4 prochaines-étapes marquées réalisées ; nouvelle §5 (réalisation effective C.1→C.11-prov + ce qui n'est PAS instancié, garde-fou D2 + catégorie C étendue L68–L86). Re-cadrage Q-seL4-2 (prémisse « sans WriteBatch » caduque) inscrit. Reste à re-instruire si/quand le GC déclenche la re-séparation.

---

## PoC E2E — complété (2026-05-16)

ADR-0019, 0020, 0021 mergés. 53 tests verts. 4/4 scénarios pass.

- [x] **ADR-0019** — Primitive `agent_infer` : ABI, sémantique sync/async, journalisation (0x0C–0x0F), double timeout, cancellation token, `InferenceBackend` trait
- [x] **ADR-0020** — Toolchain agent SDK : `wasm32-wasip1`, crate `poc/agent-sdk/`, pattern `process()`
- [x] **ADR-0021** — Convention scénarios de test : structure, nommage, format report.json, reproductibilité sémantique
- [x] **B1** — Chargement modules `.wasm` externes (`Module::from_file()`)
- [x] **B2** — Host function `agent_infer` async + code retour `Cancelled (4)`
- [x] **B3** — `InferencePool` sémaphore Tokio + `CancellationToken` par requête
- [x] **B4** — `poc/agent-sdk/` crate : wrappers A1–A4 + `agent_infer`, exemples compilables
- [x] **B5** — Scénario S1 `supervision-algorithmique` : worker LLM + supervisor Rust déterministe
- [x] **B6** — Scénario S2 `self-rollback-incoherence` : composition A1+A2 sur décision LLM
- [x] **B7** — Scénario S3 `inference-cap` : borne dure pool k=4, état `WaitingInference` observable
- [x] **B8** — Scénario S4 `scheduler-rollback` : rollback scheduler + révocation caps D5+D8
- [x] **B9** — Harness `scenarios/run-all.sh` + rapport JSON
- [x] **B10** — READMEs par scénario + README global
- [x] **LESSONS L46–L49** — Surprises semaines 1–4 capitalisées

---

## Lab historique — non prioritaires (phases 1–4 Python/Docker)

Ces dettes concernent le lab Python (`lab/`), désormais référence historique. Non bloquantes pour la suite.

- **D1** — `caused_by` scalaire en REST : `caused_by_list` devrait être primaire (ADR-0003). Impact : incohérence si usage multi-agents. Correctif : `client/client.py` + daemon.
- **D2** — Smoke test sur DB persistante : pas d'isolation entre runs. Correctif : flag `--fresh` ou namespace horodaté.
- **D3** — Rollback dangling reference : un rollback supprime des clés sans notifier les agents qui les référencent. À designer avant toute phase distribuée.
- **D4** — Causalité concurrente ambiguë : fourches non détectées avec plusieurs clients. Résolu dans le PoC Rust (ADR-0008), pas dans le lab Python.

---

## Archive — complété (phases 1–5)

<details>
<summary>33 items cochés (phases 1–5, T5, primitives A1–A4, session management…)</summary>

- [x] D2 lab — Smoke test `--fresh` + `POST /reset`
- [x] D1 lab — REST `caused_by_list` primaire, alias scalaire conservé
- [x] D3 lab — Rollback dangling : caps post-snapshot révoquées (ADR-0007)
- [x] D4 lab — Causalité concurrente : session exclusive + locking optimiste 409 (ADR-0008)
- [x] Phase 5 causal-log — RocksDB Layer 0, p99 = 11 µs sur N=10⁶ (L19)
- [x] Phase 5 store — ContentStore Merkle DAG, H-rollback-latence p95 = 99 µs (L20)
- [x] T5 officiel — K=4 runs 2026-05-15 NVMe AWS, p99 371–502 µs (résultats `results/T5/`)
- [x] poc/capabilities/ — H-revoke : check() p99=361 ns, revoke() O(N) documenté (L21)
- [x] poc/runtime/ run_loop — Wasmtime + commit barrier, H-cb-overhead 11 µs/cycle (L22)
- [x] ADR-0010 — Contrat `emit()` : EmitEnvelope MessagePack, EmitType, PendingCommit
- [x] ADR-0011 — Options RocksDB Layer 0 (bloom filter, block cache 256 MB, compression off)
- [x] ADR-0012 — Mémoire sémantique : sessions bornées 24h/10K actions, SessionBoundary (0x0A)
- [x] ADR-0013 — Protocole supervision Phase 2 : Request→AwaitingValidation→Response→Active
- [x] ADR-0014 — Politique supervision : timeout fixe 30s, pas de retry, observable
- [x] ADR-0006 amendé — Scope restreint log causal, couplage P3↔A corrigé
- [x] T6 dev — Ratio Wasmtime/Docker-Python = 8 670×, H-densité ≥ 5× (L27/L28)
- [x] H-densité reformulée — baseline Docker-Python réaliste, spec/04 + spec/02 mis à jour
- [x] W1 révisé — benchmarks/reference-workload.md : état 2 MB WASM / 50 MB ContentStore
- [x] Spec gap — H-commit-barrier + H-densité, §2.4 portée épistémique LLM (spec/01)
- [x] A1 `agent_introspect` — 73 bytes, EmitType 0x06, 3 tests (L29)
- [x] A4 cycle de vie — LifecycleState, checkpoint/terminate, Spawned/Active/Terminated (L30)
- [x] A2 self-rollback — `agent_self_rollback(depth)` MAX=3, SelfRollback 0x07 (L31)
- [x] A3 validation — `agent_request_validation` + `agent_get_verdict`, 0x08/0x09 (L32)
- [x] spec/07 plafonds architecturaux — C1 mur inférence, C2 Thundering Herd, C3 épuisement épistémique
- [x] Session management — ADR-0012 impl, session_id, SessionBoundary, SessionResume (L35)
- [x] DAG cross-agents — `agent_add_cause()`, nœuds merge N parents (L38)
- [x] Causalité implicite — `Message::Data { cause }`, `send_caused_by()` (L39)
- [x] Scheduler::spawn_child — spawn avec lien causal parent→enfant (L40)
- [x] AwaitingValidation + timeout ADR-0014 — `tokio::time::timeout_at`, 26 tests (L41)
- [x] Index secondaire `agent_ts` — CF RocksDB, query_by_agent_range O(k), WriteBatch (L42)
- [x] D5 — Message::Rollback câblé : rollback_path, SchedulerRollback 0x0B, 30 tests (L43)
- [x] D6 — BlobDB (ADR-0017) + os-poc-reconstruct (ADR-0018), poc/reconstruct/ (L43)
- [x] D7 — session_max_duration_ms configurable, constructeur exposé, 32 tests (L44)
- [x] D8 — Capability.issued_at_ms, revoke_owned_after, bras Rollback révoque caps, 33 tests (L45)
- [x] Protocole §8 — Extension thermique : modèle de menace, seuils d'invalidation, Spearman

</details>
