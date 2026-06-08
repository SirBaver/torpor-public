# Briefing — Phase 6 (propriétés fortes C1 + atomicité crash + watchdog)

**Destinataire :** Claude CLI (agent d'implémentation, session `poc/` — continuation post-PoC E2E)
**Date :** 2026-05-16
**Version :** v1
**Statut :** brief de chantier, à lire en entier avant la première ligne de code
**Durée estimée :** 3 à 4 semaines de travail focalisé
**Pré-requis lecture :** `docs/archive/poc_E2E.md` v3 (brief précédent, en entier), `spec/07-plafonds-architecturaux.md` §C1, `spec/02-properties.md` §P1b/§P3/§P6, ADR-0014 (politique supervision), ADR-0019 (`agent_infer`), ADR-0021 (convention scénarios), `TODO.md §Dettes Phase 6 (actives)`

**Position dans la trajectoire.** Phase 5 = primitives + RocksDB ; PoC E2E = câblage agent LLM réel + 4 scénarios verts ; **Phase 6 = propriétés fortes C1 + résolution des trois dettes nommées (D9, D-Q-V2.2, D-Q-V2.6)**. Phase 7 (substrat cible complet, T10) reste hors scope.

---

## 1. Contexte et cadrage

### 1.1 Pourquoi Phase 6 maintenant

Le PoC E2E a démontré (53 tests verts, 4/4 scénarios pass, ADR-0019/0020/0021 mergés) :

- L'ABI `agent_infer` est figée et exercée sur agent LLM réel.
- Le pool d'inférence borné (cap=4, sémaphore Tokio) sature de façon observable (S3).
- Le rollback scheduler pendant `WaitingInference` produit la séquence canonique `0x0C → 0x0E → 0x0B` (S4).
- La toolchain Rust→WASM via `agent-sdk` est viable, le harness `run-all.sh` produit un rapport JSON consommable.

**Ce que le PoC E2E n'a pas démontré, et qui bloque la suite :**

- L'**équité** du pool d'inférence : S3 vérifie uniquement `tous_les_workers_finissent_par_compléter`. L'ordre de service (FIFO strict vs out-of-order Tokio), la latence d'attente p99, l'absence de famine sous charge soutenue restent invérifiés (`spec/07 §C1.3`).
- La **priorité sémantique** : `supervisor > foreground user > batch` (§C1.3) n'est pas matérialisée. Le sémaphore Tokio actuel n'a pas de notion de priorité — toute requête est traitée FIFO best-effort par la file Tokio non bornée.
- L'**atomicité crash** du couple `(InferenceCancelled 0x0E, SchedulerRollback 0x0B)` : ADR-0019 §Q-V2.2 accepte la dette D-Q-V2.2 (`WriteBatch` cross-composant non fait). Un crash entre les deux émissions laisse un `0x0E` orphelin que `os-poc-reconstruct` sait lire mais qui viole une propriété d'auditabilité.
- La **borne sur la file d'attente** : `Semaphore::acquire().await` non borné = mémoire de file non bornée. Code retour `NoSlot (3)` réservé mais jamais émis (D-Q-V2.6).
- La **calibration fine du watchdog WASM** : D9 résolue mécaniquement (`epoch_interruption` + `MAX_PROCESS_ONE_TICKS = 50`, plafond 5s wall clock) mais les constantes sont des défauts uniformes. Phase 6 doit définir des profils (superviseur algorithmique : 100 ms ; agent LLM à boucle ReAct : 5 s ; batch long : 30 s) et exposer la configuration.

Ces quatre points sont une grappe cohérente — ils touchent tous le scheduler d'inférence + le log causal sous une optique production. Les traiter séparément introduirait des incohérences (par exemple : borner la file sans définir l'équité oblige à choisir une politique de rejet par défaut qu'on devra défaire).

### 1.2 Ce que Phase 6 N'EST PAS

- **Pas un produit.** Même remarque qu'en PoC E2E : pas de packaging, pas d'API de configuration runtime stable, pas de gestion d'erreur exhaustive. Du code de PoC évolué, lisible et reproductible.
- **Pas T5-qualif.** T5-qualif K≥3 sur NVMe dédié reste un chantier hardware séparé (cf. `TODO.md`). Phase 6 ne reproduit pas T5 et n'ajoute pas de benchmark latence — elle peut consommer les résultats T5 quand ils tombent mais ne les attend pas.
- **Pas T6-qualif.** Idem. La densité Wasmtime/Docker mesurée n'est pas un livrable Phase 6.
- **Pas BlobDB en production.** ADR-0017 reste appliqué côté seuil (4 KB), mais l'activation effective de BlobDB sur CF `default` n'est faite que si Q3 (`emit_payload_size_distribution`) le justifie *empiriquement* dans le scope Phase 6. Q3 est tranchée comme convention de référence ; sa réfutation par mesure réelle est un déclencheur hors scope Phase 6.
- **Pas C2 (I/O Admission Control).** C2 reste Phase 3-scheduler du substrat cible. Phase 6 ne lit pas 1 000 agents idle depuis NVMe — elle scheduler des inférences.
- **Pas C3 (mémoire sémantique).** Hors scope absolu.
- **Pas Phase 7.** Le substrat cible complet (T10) reste reporté.
- **Pas de modification d'ABI `agent_infer`.** Le code retour `NoSlot (3)` est *activé* (jusqu'ici réservé), pas réécrit. Aucun nouveau code retour. Aucun nouveau paramètre. Voir §5.

### 1.3 Décisions structurantes déjà prises (non négociables)

Ces décisions sortent du PoC E2E et restent inviolées en Phase 6. Si une décision Phase 6 contredit l'une d'elles, on écrit un **nouvel ADR de remplacement**, on ne modifie pas l'ADR existant.

**D-Ph6-A. ABI `agent_infer` figée (ADR-0019 §Q1).** Six paramètres, cinq codes retour `{0=Ok, 1=Timeout, 2=Error, 3=NoSlot, 4=Cancelled}`. Le code `3 (NoSlot)` était réservé en PoC E2E ; il devient *actif* en Phase 6 (cf. §3.1) — c'est le seul changement, et il est compatible ABI (les agents qui ignorent `3` voient un échec opaque, comportement déjà couvert par la note ADR-0019 §Q6 "Migration future").

**D-Ph6-B. Convention scénarios ADR-0021.** Tout nouveau scénario S5+ suit le patron `S<N>-<slug>/README.md` + test Rust `tests::s<N>_<slug>` dans `poc/runtime/src/lib.rs` + agents dans `poc/agent-sdk/examples/`. Backends de test = `FixedResponseBackend` ou `SleepyBackend` (jamais Ollama en CI). Format `report.json` étendu sans rétro-incompatibilité (clés ajoutées, jamais retirées).

**D-Ph6-C. Seuil BlobDB 4 KB (ADR-0017 §3bis, confirmé par Q3 2026-05-16).** Aucun changement de seuil en Phase 6. Si une mesure Phase 6 produit p90 > 8 KB sur les payloads `emit()` réels (écart > 2× sur la convention), c'est un déclencheur d'amendement transparent — pas un changement de Phase 6.

**D-Ph6-D. Borne P3a 10 ms (Q1 2026-05-16).** Portée P3a exclusivement (lookup point sur DB statique). Phase 6 ne mesure pas P3a. Si une dégradation est observée incidentellement, c'est un signal pour T5-bis, pas un livrable Phase 6.

**D-Ph6-E. Working set K=128, recouvrement 10 % (Q2 2026-05-16).** Modèle B (recency-biased) comme convention de référence pour tout dimensionnement Phase 6. Aucun benchmark Phase 6 ne dépend de Modèle A.

**D-Ph6-F. Les 53 tests existants restent verts.** Au commit près. Tout test rouge à l'issue d'un livrable Phase 6 est soit rollback, soit justifié par un ADR de remplacement.

**D-Ph6-G. Pas de nouvelle primitive WASM exposée à l'agent.** Phase 6 modifie le scheduler d'inférence côté hôte. L'agent voit la même surface : `agent_infer` avec les mêmes codes retour, plus la possibilité réelle de recevoir `NoSlot (3)`. Aucun nouvel `agent_*` n'est ajouté.

**D-Ph6-H. Pas d'humain dans la boucle.** Identique au PoC E2E (§1.2-D-B). Toute orchestration est portée par un agent ou par le harness de test.

---

## 2. État actuel (à utiliser tel quel)

### 2.1 Ce qui est en place

Synthèse — voir `poc/README.md` et `poc/scenarios/README.md` pour le détail.

- 53 tests verts dans `poc/`. 4 scénarios pass (S1–S4).
- Modules : `causal-log/` (EmitType `0x05`–`0x0F`, CF `default`+`agent_ts`), `store/` (Merkle DAG, rollback p95=99 µs sur W2), `capabilities/` (`revoke_owned_after`, lazy chain check), `runtime/` (`actor.rs`, `InferencePool`, état `WaitingInference`, watchdog `epoch_interruption`), `reconstruct/` (`os-poc-reconstruct` lit `0x0C`–`0x0F`), `agent-sdk/` (wrappers Rust→WASM).
- Trait `InferenceBackend` + 3 impls : `OllamaBackend` (production, hors CI), `SleepyBackend` (latence configurable), `FixedResponseBackend` (déterministe, S1/S2).
- Scheduler : `Scheduler::spawn`, `spawn_child`, `respond_validation`, `rollback` (D5+D8 câblés).
- Watchdog Phase 2 : `Config::epoch_interruption(true)` + thread bg `increment_epoch` 100 ms + `Store::set_epoch_deadline(50)` réarmé par `Message::Data` → 5 s wall clock max par `process_one`. Tests : `t2_watchdog_traps_infinite_loop_agent` + équivalents.
- ABI `agent_infer` : 5 codes retour exposés, `NoSlot (3)` jamais émis (sémaphore non borné).

### 2.2 Ce qui manque pour les propriétés fortes C1

Tableau orienté Phase 6.

| Brique | Quoi | Pourquoi | Référence |
|---|---|---|---|
| Ph6-B1 | ADR-0022 — File d'inférence bornée + discipline d'ordonnancement | Définit la sémantique de service (FIFO / priorité / probabiliste) et la politique de rejet. Active le code retour `NoSlot (3)`. | D-Q-V2.6, §3.1 |
| Ph6-B2 | ADR-0023 — Définition formelle d'équité et borne d'attente | Sans définition, "équité" n'est pas mesurable. Choix d'une définition opérationnelle (fair-share sliding window vs FIFO strict vs absence de famine bornée). | §3.2, `spec/07 §C1.3` |
| Ph6-B3 | ADR-0024 — Atomicité crash `(0x0E, 0x0B)` | Choix entre `WriteBatch` cross-composant et journal de compensation. Tranche D-Q-V2.2. | §3.3, ADR-0019 §Q-V2.2 |
| Ph6-B4 | ADR-0025 — Profils de watchdog par classe d'agent | Calibre `EPOCH_TICK_MS` / `MAX_PROCESS_ONE_TICKS` par profil (algorithmique, LLM-court, LLM-long, batch). Résout D9 résiduel. | §3.4, ADR-0019 §Q-V2.1, `TODO.md D9` |
| Ph6-B5 | Implémentation : `InferenceQueue` bornée + politique d'ordonnancement | Substitue le `Semaphore` plat par une file structurée. Code retour `NoSlot (3)` actif. | Ph6-B1 |
| Ph6-B6 | Implémentation : harness d'équité (méta-test instrumentant le scheduler) | Permet de valider l'invariant choisi (S5-fairness) sans dépendre du timing wallclock. | Ph6-B2 |
| Ph6-B7 | Implémentation : atomicité crash via la stratégie tranchée | `WriteBatch` cross-composant ou journal de compensation. Tests crash injectés via `kill -9` ou point d'arrêt instrumenté. | Ph6-B3 |
| Ph6-B8 | Implémentation : profils de watchdog configurables + tests | Configuration par `AgentProfile` enum exposé au scheduler. | Ph6-B4 |
| Ph6-B9 | Scénario S5 `C1-fairness-priorité` | Démontre fair-share + priorité sémantique (supervisor passe devant batch). | §4.3 |
| Ph6-B10 | Scénario S6 `crash-during-cancel` (optionnel — Semaine 4) | Démontre atomicité (0x0E, 0x0B) sous crash injecté. | §4.4 |
| Ph6-B11 | Extension `os-poc-reconstruct` pour rendre lisibles priorité + queue state | Sans rendu, l'observabilité de C1 régresse. | §4.2 |
| Ph6-B12 | LESSONS L50+ et `TODO.md` mis à jour | Capitalisation post-phase. | §4.5 |

---

## 3. Décisions de design à trancher en début de phase (ADR à produire)

Chaque sous-section liste les options retenues, les questions ouvertes, et la décision attendue avant le code. Format : Q-Ph6-* avec arbitrage par ADR-022x.

### 3.1 ADR-0022 — File d'inférence bornée et discipline d'ordonnancement

**Contexte.** ADR-0019 §Q6 (D-Q-V2.6) accepte un sémaphore non borné en Phase 2 par défaut de politique. Phase 6 doit borner la file et choisir une discipline.

**Q-Ph6-1. Discipline de service.** Trois options principales, non exclusives au sein d'une même implémentation à plusieurs files :

| Option | Sémantique | Avantage | Coût | Précédent |
|---|---|---|---|---|
| A. FIFO strict | Insertion en queue, retrait en tête. Une seule file. | Simple, équité ordinale triviale, prévisible. | Pas de priorité — un supervisor attend derrière un batch. Inversion de priorité possible. | Tokio `Semaphore` par défaut + file bornée explicite. |
| B. Priorité multi-niveau strict | N files par classe (e.g. {supervisor, foreground, batch}), service dans l'ordre des classes ; FIFO au sein de chaque classe. | Matérialise §C1.3 directement. | Famine possible des classes basses si la classe haute est saturée. | Linux `SCHED_FIFO` + `nice` ; SGE/Slurm partitions. |
| C. Pondéré / fair-share | Chaque classe a un quota proportionnel sur une fenêtre glissante. À l'intérieur d'une classe, FIFO. | Évite la famine, expose des poids. | Définir la fenêtre, les poids, le rééquilibrage = surface de décision plus large. | Linux CFS ; Stride scheduling [Waldspurger 1995] ; Lottery scheduling [Waldspurger & Weihl 1994]. |

**Recommandation provisoire à confirmer par ADR-0022 :** **B avec garde-fou de famine bornée** — priorité stricte par classe, mais une borne `max_starvation_ms` par classe basse au-delà de laquelle une requête en attente est promue d'un cran. C'est la formulation la plus proche de §C1.3 sans s'engager prématurément sur des paramètres CFS-like dont on n'a pas les données.

**Q-Ph6-2. Politique de rejet quand la file est pleine.** Quatre options. La décision dépend en partie de Q-Ph6-1.

| Option | Sémantique | Avantage | Coût |
|---|---|---|---|
| α. Drop-newest (`try_acquire` → `NoSlot`) | Refuse l'arrivante. Code retour `NoSlot (3)` côté agent. | Stabilité de la file. L'agent décide de retry ou pas. | Charge des arrivants ; les anciens monopolisent. |
| β. Drop-oldest | Évince la plus vieille en attente. | Borne dure sur le temps de séjour. | Trahit l'agent évincé — son `agent_infer` retourne tardivement `NoSlot`. Comportement étrange côté agent. |
| γ. Drop par priorité (basse classe évincée) | Si Q-Ph6-1 = B/C : on évince la plus basse classe en attente avant de refuser une haute. | Cohérent avec priorité sémantique. | Couplage fort avec Q-Ph6-1, complexité de mise en œuvre. |
| δ. Backpressure (l'appel bloque côté hôte) | Pas de borne effective. | Aucun rejet. | Annule la borne — équivalent au sémaphore non borné actuel. |

**Recommandation provisoire :** **α (drop-newest)** par défaut, **γ** si on retient la priorité multi-niveau (B en Q-Ph6-1). Pas δ (annulerait le livrable).

**Q-Ph6-3. Taille de la file.** `queue_capacity` : paramètre de configuration, pas constante. Valeur par défaut à choisir.

- Avec cap pool = 4 (PoC E2E S3) et latence inférence ~2,5 s (qwen2.5:3b) ou 100 ms (`SleepyBackend` test), la file absorbe un burst. À 100 ms d'inférence et 4 slots, le débit en régime est 40 inférences/s. Une file de 32 absorbe un burst de 0,8 s à débit nul.
- Recommandation provisoire : `queue_capacity = 8 × max_concurrent_inferences` (= 32 par défaut). Borne empirique, à confirmer par mesure S5.

**Q-Ph6-4. Visibilité de l'état de file dans le log causal.** Faut-il ajouter un EmitType `InferenceEnqueued` ou enrichir `InferenceRequest (0x0C)` ?

Trois options :
- Aucun changement de log : l'attente est invisible. Régression d'observabilité par rapport au PoC E2E (qui rendait l'attente visible via `LifecycleState::WaitingInference`).
- Enrichir le payload de `0x0C` avec `queue_depth_at_admission u16 LE` et `priority_class u8`. Pas de nouveau EmitType. Cohérent avec Q-V2.4 (clamp observable dans le payload).
- Nouveau EmitType `0x10 InferenceAdmitted` distinct de `0x0C InferenceRequest`, marquant la sortie de la file.

**Recommandation provisoire :** **option 2** — enrichir `0x0C`. Évite une migration de schéma RocksDB et reste rétro-compatible (les anciens decoders ignorent les bytes en queue de payload). Si le besoin de tracer `InferenceAdmitted` distinct apparaît à l'usage, on l'ajoute en option 3 plus tard.

**Livrable ADR-0022.** Décisions Q-Ph6-1 à Q-Ph6-4 tranchées, justifiées, alternatives consignées. Format MADR identique aux ADR existants. Référence explicite à `spec/07 §C1.3`.

### 3.2 ADR-0023 — Définition formelle d'équité et borne d'attente

**Contexte.** "Équité" sans définition est invérifiable. `spec/07 §C1.3` mentionne "priorité sémantique" sans définition opérationnelle. Trois définitions candidates, chacune mesurable.

**Q-Ph6-5. Définition d'équité retenue.**

| Définition | Énoncé opérationnel | Mesure | Précédent |
|---|---|---|---|
| E1. Équité ordinale (FIFO strict intra-classe) | Pour deux requêtes A et B de la même classe, si A entre dans la file avant B, A obtient un slot avant B. | Traçage de l'ordre `admission_ts` → `slot_acquired_ts` par classe ; vérifier qu'il est monotone. | File d'attente standard. |
| E2. Fair-share proportionnel sur fenêtre glissante | Sur une fenêtre de W secondes, chaque classe reçoit au moins `floor(W × weight_class / sum(weights))` slots. | Compteurs par classe sur fenêtre glissante (e.g. W=10 s). | Linux CFS, Stride scheduling. |
| E3. Absence de famine bornée | Toute requête en attente obtient un slot en au plus `max_wait_ms`. Pas de garantie d'ordre, seulement de progression. | Mesure de `wait_time_ms = slot_acquired_ts − admission_ts`. Assert `p100 ≤ max_wait_ms`. | Liveness conditionnelle (`spec/02 §4.3`). |

**Compatibilité avec Q-Ph6-1.**
- B (priorité stricte + garde-fou famine) ⇒ **E1 intra-classe + E3 inter-classes**. Compatible.
- C (fair-share pondéré) ⇒ **E2 + E3**. Le compteur fenêtré *est* la politique.
- A (FIFO global) ⇒ **E1 strictement**. Pas de priorité, donc E3 trivial sous load borné.

**Recommandation provisoire :** **E1 + E3**. C'est la combinaison la plus simple à formaliser et la plus directement liée au test S5. E2 demande de fixer poids et fenêtre — paramètres qu'on n'a pas les données pour calibrer en Phase 6. À reconsidérer en Phase 7 quand on aura des profils d'usage.

**Q-Ph6-6. Valeur de `max_starvation_ms` et `max_wait_ms`.**

- Si E3 est retenu, la borne doit être chiffrée. Sinon non-falsifiable.
- Cap inférence = 4, latence par appel = `t_infer`. Pire cas d'attente FIFO sur N agents en file : `N × t_infer / cap`. À N=32, cap=4, `t_infer=2,5 s` → 20 s pire cas asymptotique. Test S3 actuel n'a pas pris cette mesure.
- Recommandation provisoire : `max_wait_ms = 30_000` (30 s) en valeur défaut. Borne lâche, calibrable par config. Si dépassée, la requête est **promue d'une classe vers le haut** (Q-Ph6-1 B avec garde-fou) ou **refusée** (Q-Ph6-1 A avec drop oldest) — décision liée à Q-Ph6-2.

**Q-Ph6-7. Métrique observable.** Comment mesurer la conformité dans S5 ?

Trois options :
- Instrumenter l'`InferenceQueue` avec compteurs et timestamps (`admission_ts`, `slot_acquired_ts`) ; exposer un snapshot via `Scheduler::queue_stats()`.
- Reconstruire depuis le log causal : `0x0C` enrichi (Q-Ph6-4) contient `queue_depth_at_admission` ; corréler avec `0x0D`/`0x0E`/`0x0F` pour obtenir la latence d'attente.
- Les deux.

**Recommandation provisoire :** **les deux**. L'instrumentation runtime sert au test S5 (assertions de bornes en temps réel) ; le rendu via `os-poc-reconstruct` sert l'observabilité post-hoc.

**Livrable ADR-0023.** Définition retenue (E1 + E3 ou autre), valeurs par défaut chiffrées, métrique observable, exemple de séquence test S5 attendue. Référence à `spec/02 §4.3` (liveness conditionnelle), `spec/07 §C1.3`.

### 3.3 ADR-0024 — Atomicité crash `(0x0E, 0x0B)`

**Contexte.** D-Q-V2.2. Sur crash entre `InferenceCancelled (0x0E)` (émis par `InferencePool`) et `SchedulerRollback (0x0B)` (émis par `Scheduler::rollback`), `os-poc-reconstruct` voit un `0x0E` orphelin. ADR-0019 §Q-V2.2 considère le coût d'un `WriteBatch` unifié disproportionné en Phase 2. Phase 6 doit trancher.

**Q-Ph6-8. Stratégie.** Trois options.

| Option | Sémantique | Avantage | Coût |
|---|---|---|---|
| W. `WriteBatch` cross-composant | `InferencePool::cancel()` ne loggue plus directement — il prépare un `BatchEntry` qui sera consommé par `Scheduler::rollback` et commité atomiquement avec `0x0B`. | Atomicité forte au niveau RocksDB (cross-CF garanti). | Couplage : `InferencePool` doit connaître l'existence du `Scheduler` ou être appelé par lui pour le commit. Fuite d'abstraction. |
| J. Journal de compensation | `0x0E` reste émis directement par `InferencePool`. `Scheduler::rollback` écrit en plus une entrée `CompensationLink (0x11)` au début qui référence le `0x0E` attendu. Au recovery, `os-poc-reconstruct` détecte un `0x11` orphelin (sans `0x0E` postérieur) ou un `0x0E` orphelin (sans `0x11` antérieur) et les marque comme transactions incomplètes. | Découplage préservé. Auditabilité forte. | Le log n'est pas atomique — c'est la reconnaissance des orphelins qui restaure la cohérence. Recovery plus complexe. |
| O. Ordre inversé + idempotence | Émettre `0x0B` *avant* `0x0E`, et concevoir `0x0E` comme idempotent (un `0x0B` sans `0x0E` postérieur signifie "cancellation implicite, à appliquer au recovery"). | Pas de WriteBatch, ordre simple. | Sémantiquement étrange — on annonce le rollback avant de cancel. Casse la lisibilité ADR-0019 §Q-V2.1 (`cancel → send → log 0x0B`). |

**Recommandation provisoire :** **J (journal de compensation)** avec nouveau EmitType `0x11 CompensationOpen` et `0x12 CompensationClose`.
- Avant `token.cancel()`, le `Scheduler` émet `0x11` avec payload `[target_agent_id, expected_inference_event_id]`.
- `InferencePool` émet `0x0E` normalement.
- Après `Message::Rollback` envoyé, `Scheduler` émet `0x0B`.
- Après application du rollback, `Scheduler` émet `0x12 CompensationClose`.
- Au recovery, `os-poc-reconstruct` détecte tout `0x11` sans `0x12` correspondant comme transaction incomplète → applique une politique de réconciliation (`auto-close + warning` ou `manual review` selon configuration).

L'option W est rejetable pour la même raison qu'en ADR-0019 §Q-V2.2 : fuite d'abstraction entre `InferencePool` et `Scheduler`. L'option O viole la lisibilité du log.

**Q-Ph6-9. Coût en latence et en taille de log.**

- 2 nouveaux EmitType par cycle rollback. Payload total : ~80 bytes × 2 = 160 bytes additionnels. Négligeable.
- Latence : 2 `append` RocksDB additionnels (~20 µs par fsync sur NVMe local). À mesurer pendant Semaine 2.

**Q-Ph6-10. Test crash.** Comment forcer le crash entre `0x0E` et `0x0B` ?

- Option a : `std::process::exit(0)` ou `panic!` instrumenté au point exact (test-only, via cfg ou injection de point d'arrêt).
- Option b : `kill -9` du process Tokio depuis un test enfant ; non déterministe sans synchronisation supplémentaire.

**Recommandation :** option a, via un trait d'injection `CrashPoint` qui no-op en production et déclenche `process::exit` en test. Pattern proche du failpoint des SGBD.

**Livrable ADR-0024.** Stratégie tranchée (J recommandée), nouveau EmitType `0x11`, `0x12`, politique de réconciliation au recovery, mécanisme de test (failpoint).

### 3.4 ADR-0025 — Profils de watchdog par classe d'agent

**Contexte.** D9 — watchdog mécanique en place (`epoch_interruption` + `MAX_PROCESS_ONE_TICKS = 50`, plafond 5 s wallclock). Constantes uniformes. Phase 6 doit définir des profils.

**Q-Ph6-11. Classes d'agent.** Quel découpage ?

- C-Algo : agent algorithmique pur (e.g. `supervisor_arith` du S1). Plafond serré (100 ms). Toute boucle de plus de 100 ms est suspecte.
- C-LLM-court : agent à un seul appel `agent_infer` puis decision. Plafond modéré (5 s — équivalent défaut PoC E2E).
- C-LLM-long : agent à boucle ReAct multi-tours, possiblement entre plusieurs `agent_infer`. Plafond large (30 s entre deux yields).
- C-Batch : agent qui traite un fichier ou un dataset, peut être légitimement long. Plafond très large (5 min) ou désactivé.

**Recommandation provisoire :** ces quatre classes, exposées via `AgentProfile` enum dans `agent-sdk`. L'agent déclare son profil à `Scheduler::spawn(profile, ...)`.

**Q-Ph6-12. Mécanisme de déclaration.** Par l'agent (auto-déclaratif, manipulable) ou par le scheduler (basé sur l'identité du module) ?

- Auto-déclaratif : l'agent fournit `AgentProfile` au moment du spawn via le SDK. Simple mais l'agent peut mentir.
- Tiers : le scheduler infère le profil depuis le hash du module ou un manifest signé. Sûr mais lourd.

**Recommandation provisoire :** **auto-déclaratif en Phase 6**, avec note explicite : un agent qui demande C-Batch quand il devrait être C-Algo gaspille des ressources mais ne casse pas le système. Le watchdog reste un *garde-fou* contre les boucles infinies, pas un mécanisme de sécurité. Phase 7+ pourrait introduire un manifest signé si nécessaire.

**Q-Ph6-13. Configuration et observabilité.** Comment exposer les constantes par profil ?

- Fichier de config `runtime.toml` chargé au démarrage du scheduler.
- Constantes Rust compilées dans `runtime/src/watchdog.rs` avec override possible.
- Émettre dans le log causal le profil utilisé au `Spawned (0x01)` pour traçabilité.

**Recommandation provisoire :** constantes compilées par défaut avec override via config, profil émis dans `Spawned` payload.

**Livrable ADR-0025.** Enum `AgentProfile`, valeurs `EPOCH_TICK_MS` / `MAX_PROCESS_ONE_TICKS` par profil, mécanisme de déclaration (auto), traçabilité (profil dans `Spawned`).

---

## 4. Plan d'exécution par semaine

### Semaine 1 — ADR + file bornée + test d'équité basique

**Livrables :**

- **ADR-0022, 0023, 0024, 0025** écrits, revus, mergés. C'est le bloc bloquant pour les semaines 2–4. Pas de code avant que les quatre soient acceptés.
- **Ph6-B5** : remplacement du `Semaphore` plat dans `InferencePool` par `InferenceQueue` selon ADR-0022.
  - Si Q-Ph6-1 = B retenu : trois files de classe `{Supervisor, Foreground, Batch}`, FIFO intra-classe, priorité stricte avec garde-fou famine.
  - Si Q-Ph6-2 = α : `try_acquire` → `NoSlot (3)` actif.
  - Configuration : `queue_capacity` (défaut 32), `max_concurrent_inferences` (défaut 4).
  - `0x0C` enrichi avec `queue_depth_at_admission`, `priority_class` (Q-Ph6-4).
- **Ph6-B6** : harness d'équité minimal (extensions à `Scheduler::queue_stats()`, traçabilité dans le log).
- **Tests unitaires :** au moins :
  - `t_queue_bounded_emits_no_slot` : file saturée à `queue_capacity` → `NoSlot (3)` émis avec EmitType `0x0F` (`error_code = 0x20`).
  - `t_queue_priority_supervisor_passes_batch` : 5 batch + 1 supervisor en file → supervisor servi avant batch.
  - `t_queue_starvation_promotion` : batch en attente > `max_starvation_ms` → promu en foreground.

**Critère de sortie Semaine 1 :**
- Les 4 ADR mergés (statut Accepté).
- File bornée fonctionnelle, code retour `NoSlot (3)` émis dans au moins un test unitaire.
- Les 53 tests existants restent verts (test S3 doit toujours passer — la borne dure k=4 est préservée).
- Au moins 3 nouveaux tests unitaires verts (file bornée + priorité + promotion famine).

**Risques :**
- Désaccord sur Q-Ph6-1 (A vs B vs C) → bloque tout. Mitigation : préparer les arguments avant Semaine 1, viser arbitrage Jour 1–2.
- Tokio `Semaphore` ne supporte pas la priorité nativement. Implémentation manuelle via `Mutex<VecDeque>` + `Notify`. Surface d'attaque pour data races — tests d'intégration ciblés sur la concurrence file.

### Semaine 2 — Atomicité crash `(0x0E, 0x0B)`

**Livrables :**

- **Ph6-B7** : implémentation ADR-0024 stratégie J (recommandée).
  - Ajout EmitType `0x11 CompensationOpen`, `0x12 CompensationClose` à `causal-log/`.
  - `Scheduler::rollback` modifié : émet `0x11` avant `inference_pool.cancel()`, `0x12` après application du rollback.
  - Trait `CrashPoint` pour injection de panique en test, no-op en release.
  - Logique de réconciliation dans `os-poc-reconstruct` : détection des paires `0x11` orphelines.
- **Tests crash injectés :**
  - `t_crash_between_0e_and_0b_recovery_clean` : `0x11` émis, panique injectée avant `0x0B` → recovery détecte l'orphelin, applique politique `auto-close + warning`.
  - `t_crash_after_0b_before_0e` : cas symétrique. Selon ordre d'émission, soit impossible (Q-V2.1 garantit `0x11 → cancel → 0x0E → 0x0B → 0x12`), soit détecté.
  - `t_no_crash_clean_path_emits_full_quartet` : run normal → log contient `0x11, 0x0C, 0x0E, 0x0B, 0x12` dans cet ordre.
- Extension `os-poc-reconstruct` pour rendre lisibles `0x11` et `0x12` (Ph6-B11 partiel).

**Critère de sortie Semaine 2 :**
- Les tests crash passent (déterministes via `CrashPoint`).
- Aucun test S1–S4 régressé.
- `os-poc-reconstruct` rend la séquence `0x11..0x12` comme un bloc transactionnel lisible.

**Risques :**
- Le trait `CrashPoint` doit pouvoir interrompre un appel async Tokio sans corrompre le runtime de test. Mitigation : utiliser `std::process::exit(0)` qui termine le process net (Tokio peut être terminé brutalement, c'est le comportement testé).
- Réplication d'état post-recovery : le test doit relancer un nouveau `Scheduler` sur la même DB RocksDB, ce qui exige un état RocksDB cohérent. Vérifier que le crash injecté n'écrit pas de données half-flushed (s'appuyer sur WAL fsync — ce qui est l'invariant testé).

### Semaine 3 — Watchdog calibration + scénario S5

**Livrables :**

- **Ph6-B8** : implémentation ADR-0025.
  - Enum `AgentProfile { Algo, LlmShort, LlmLong, Batch }` exposé dans `agent-sdk`.
  - `Scheduler::spawn(profile, module, ...)` accepte le profil.
  - `Store::set_epoch_deadline(N)` paramétré par profil au début de chaque `process_one`.
  - Profil émis dans `Spawned (0x01)` payload (ajout d'un byte `profile: u8`).
- **Tests calibration :**
  - `t_algo_profile_traps_at_100ms` : agent `AgentProfile::Algo` qui boucle 200 ms → trap.
  - `t_llm_long_profile_allows_30s_loop` : agent `AgentProfile::LlmLong` qui boucle 10 s → pas de trap (deadline = 30 s).
  - `t_batch_profile_completes_long_task` : agent `AgentProfile::Batch` qui calcule 60 s → pas de trap.
- **Ph6-B9 — Scénario S5 `C1-fairness-priorité`** (`poc/scenarios/S5-fairness-priority/`).
  - Setup : 8 agents `density_worker` configurés en `AgentProfile::Foreground` + 2 agents `supervisor_arith` configurés en `AgentProfile::Supervisor`. Cap=2 slots d'inférence. `SleepyBackend(100ms)`.
  - Tous spawnés ensemble. Les 10 demandent `agent_infer` simultanément.
  - Assertions :
    - **Priorité :** les 2 supervisors obtiennent leur slot dans les 200 ms après spawn (= 2 × latence inférence), alors qu'aucun foreground n'a encore son slot ou seulement un sur huit.
    - **Pas de famine bornée :** chaque foreground obtient son slot en `max_wait_ms` ≤ 30 s (Q-Ph6-6).
    - **Ordre intra-classe :** parmi les 8 foreground, l'ordre de service est FIFO sur `admission_ts`.
    - **Log causal :** chaque `0x0C` porte `priority_class` correct et `queue_depth_at_admission` cohérent avec l'ordre observé.
  - Verdict reproductible (E1 + E3) sans dépendre du sampling Tokio (instrumentation déterministe via `Scheduler::queue_stats()`).
- Extension `os-poc-reconstruct` finale : rendu lisible des classes de priorité dans la chronologie.

**Critère de sortie Semaine 3 :**
- S5 vert, déterministe, reproductible 10 runs sur 10.
- Les 4 profils watchdog validés par tests dédiés.
- Tests S1–S4 toujours verts.

**Risques :**
- Tokio scheduler non déterministe → S5 flaky. Mitigation : pas de mesure wallclock dans les assertions ; tout passe par les instruments runtime (`queue_stats`). Si flaky persiste > 20 %, alourdir les marges (`max_wait_ms` testé = 60 s au lieu de 30 s) et documenter dans LESSONS.
- `AgentProfile` exposé via SDK = changement d'API publique. Mitigation : valeur par défaut `AgentProfile::LlmShort` (comportement actuel) ; agents existants restent compilables sans modification.

### Semaine 4 — Polissage + buffer (optionnel selon avancement)

**Livrables :**

- **Ph6-B10 — Scénario S6 `crash-during-cancel`** (optionnel, recommandé si Semaine 2 a tenu le planning).
  - Démontre atomicité (0x0E, 0x0B) sous crash injecté de bout en bout.
  - 1 worker en `WaitingInference`, scheduler initie rollback, `CrashPoint::Fire` entre `0x11` et `0x12`. Process redémarré, `os-poc-reconstruct` reconstruit la chronologie et applique la réconciliation.
  - Assert : verdict `pass` si réconciliation réussit, `fail` sinon.
- **Ph6-B11 finalisation** : `os-poc-reconstruct` rend toutes les nouvelles entrées (`0x11`, `0x12`, payload enrichi de `0x0C`) avec un format lisible humain.
- **Ph6-B12** : LESSONS L50+ (au moins une entrée par semaine), `TODO.md` mis à jour (D9, D-Q-V2.2, D-Q-V2.6 marquées résolues).
- **README global Phase 6** : `docs/phase6-rapport.md` ou section dans `docs/archive/phase6.md` listant les réussites, les écarts honnêtes, les nouvelles dettes éventuelles.
- **Harness `run-all.sh` mis à jour** pour inclure S5 et S6, rapport JSON étendu.

**Critère de sortie Semaine 4 :**
- Si la Semaine 3 a livré : S5 vert + 2 nouveaux ADR mergés (0022, 0025) suffit.
- Si la Semaine 2 a livré : S5 vert + S6 vert + 4 ADR mergés = succès complet.
- LESSONS L50–L53 minimum.

**Risques :**
- Semaine 4 est explicitement un buffer. Si la Semaine 2 a glissé, Semaine 4 absorbe le retard. Pas de livrable nouveau critique.

---

## 5. Contraintes et invariants

### 5.1 Ce qui ne doit pas changer

- **Les 53 tests existants restent verts**, commit par commit. Tout test rouge est rollback ou justifié par ADR de remplacement (D-Ph6-F).
- **ABI `agent_infer` figée (D-Ph6-A).** Six paramètres, cinq codes retour. Le code `3 (NoSlot)` passe de "réservé" à "actif". Aucun autre changement.
- **Convention scénarios ADR-0021 (D-Ph6-B).** S5 et S6 suivent strictement le patron `S<N>-<slug>/README.md` + test Rust + agents.
- **Pas de modification des ADR acceptés sans ADR de remplacement (D-Ph6-G).** ADR-0019 reste tel quel. Si Phase 6 a besoin d'amender une décision d'ADR-0019, on émet ADR-0019bis ou ADR-0022 (selon le scope).
- **Format `EmitEnvelope` MessagePack inchangé.** Les nouveaux EmitType `0x11`, `0x12` (et possible enrichissement de `0x0C`) suivent la même structure. Pas de migration RocksDB.
- **Wire format de `InferenceRequest (0x0C)` rétro-compatible** : enrichissement par bytes additionnels en fin de payload (decoders existants ignorent le surplus).

### 5.2 Ce qui doit rester transparent

- **Humain hors boucle.** Aucun scénario S5/S6 ne demande d'entrée utilisateur. Toute orchestration est portée par le harness ou par un agent.
- **`os-poc-reconstruct` reste l'unique interface humaine.** Toute nouvelle information (priorité, queue depth, crash recovery) doit y être rendue.
- **Pas d'humain dans le watchdog.** Le profil d'agent est auto-déclaratif (Q-Ph6-12). Pas de mécanisme d'audit humain ; c'est un garde-fou, pas une frontière de sécurité.

### 5.3 Dettes acceptées en Phase 6

- **Pas de manifest signé pour les profils watchdog.** Reporté en Phase 7. À inscrire LESSONS si un cas d'usage adversarial émerge.
- **Pas de calibration empirique des poids fair-share (Q-Ph6-5 E2 non retenue).** Reporté en Phase 7 ou en aval, dépend des données de production.
- **Pas de mesure de performance du nouveau scheduler.** L'overhead d'`InferenceQueue` par rapport au `Semaphore` plat n'est pas chiffré en Phase 6. À mesurer en T5-bis ou benchmark dédié.
- **Pas de récupération après crash multi-noeud.** La réconciliation `0x11/0x12` est mono-process. Phase 7+ pour distribué.
- **Pas de retry automatique sur `NoSlot (3)`.** L'agent voit l'erreur, décide. Cohérent ADR-0014 (pas de retry hôte).

### 5.4 Points de vigilance

- **Priorité = inversion possible si bug.** Un test `t_no_priority_inversion_under_load` est requis : N batch + 1 supervisor, vérifier que le supervisor n'attend jamais derrière un batch (ordre dans le log causal).
- **Garde-fou famine = boucle de promotion.** Si `max_starvation_ms` trop court, un batch promu en foreground peut être à nouveau dépassé puis promu en supervisor — boucle de promotion. Test `t_promotion_is_bounded_one_step` : promotion d'un cran maximum par requête.
- **Trait `CrashPoint` ne doit pas être exécutable en release.** `#[cfg(feature = "crash-injection")]` ou équivalent. Vérifier le binaire release ne contient pas le symbole.
- **`queue_capacity` × `max_concurrent_inferences` × état par requête en attente** = empreinte mémoire. À 32 × 4 × ~1 KB par entry = ~128 KB. Acceptable. Mais si Phase 7 met cap=100, 100 × 100 × 1 KB = 10 MB par scheduler. À garder en tête.

---

## 6. Cohérence avec la spec existante

| Élément de spec | Couverture par Phase 6 | Hors scope Phase 6 |
|---|---|---|
| **P1a Densité hébergée** | Non touchée. Aucun impact attendu (l'`InferenceQueue` est en mémoire mais bornée à 32 entries par défaut). | Mesure T6-qualif reste séparée. |
| **P1b Densité active** | Non mesurée. Phase 6 ne reproduit pas le benchmark débit d'actions/s. L'`InferenceQueue` peut ralentir le débit asymptotique (overhead de priorité) — à mesurer en T-future. | Quantification du ratio R_actif vs Docker. |
| **P2 Rollback** | Préservée. L'atomicité crash `(0x0E, 0x0B)` (ADR-0024) renforce l'invariant sans modifier le mécanisme de rollback. | Mesure borne 100 ms reste de T5-rollback. |
| **P3a Lookup point** | Non touchée. Les nouveaux EmitType (`0x11`, `0x12`) s'insèrent dans la même CF `default` ; volume négligeable. | Mesure p99 ≤ 10 ms reste de T5-qualif. |
| **P3b End-to-end** | Non touchée. | T5-bis. |
| **P3c Multi-agent** | Indirectement touchée : le scénario S5 exerce 10 agents concurrents avec instrumentation. Pas de mesure latence. | Bornes 50/100 ms réservées. |
| **P4 Capabilities** | Préservée. Le `AgentProfile` auto-déclaratif (Q-Ph6-12) n'est pas une cap — c'est un hint runtime. Aucune modification du modèle de capability. | Cap d'inférence (rate-limiting) reportée Phase 7. |
| **P5 Déterminisme transition** | Préservée. Les `InferenceBackend` déterministes (`FixedResponseBackend`) restent utilisés en CI. | Replay complet reste hors scope. |
| **P6 Atomicité crash** | **Partiellement adressée par ADR-0024** pour la transition `(InferenceCancelled, SchedulerRollback)`. SEF-4 général (tout commit barrier) reste à mesurer séparément. | SEF-4 complet. |
| **`spec/07 §C1.1` Mur d'inférence** | Borne dure démontrée par S3 (PoC E2E). Phase 6 préserve. | Recalibrage cap selon GPU réel. |
| **`spec/07 §C1.2` File d'attente avec priorité sémantique** | **Démontrée par S5** via ADR-0022 (B retenu provisoirement) + ADR-0023 (E1+E3). | Calibration empirique des poids (E2). |
| **`spec/07 §C1.3` Priorité sémantique (supervisor > foreground > batch)** | **Matérialisée** : trois classes implémentées, supervisor passe devant batch dans S5. | Hiérarchie plus fine (e.g. par session_id, par criticité). |
| **`spec/07 §C1.4` Absence de famine bornée** | **Adressée** via garde-fou de promotion (Q-Ph6-1 B + Q-Ph6-6 `max_starvation_ms`). | Garantie formelle (preuve) — restera hors scope. |
| **`spec/07 §C1.5` Latence d'attente bornée** | **Adressée** via E3 (`max_wait_ms = 30 s` par défaut). Test S5 vérifie. | Borne plus serrée selon SLA agent (Phase 7). |
| **`spec/07 §C2` Thundering Herd (PCIe)** | **Hors scope Phase 6.** | I/O Admission Control reste Phase-substrat. |
| **`spec/07 §C3` Épuisement épistémique** | **Hors scope absolu.** | Approche A/B/C reste à trancher hors Phase 6. |

---

## 7. Critères de succès du chantier

Le chantier est **pleinement réussi** si à la fin de la Semaine 4 :

1. Les **quatre ADR (0022, 0023, 0024, 0025)** sont écrits, revus, mergés au statut Accepté, et référencés depuis `decisions/INDEX.md`.
2. Le **scénario S5 `C1-fairness-priority`** passe vert en `cargo test --release`, déterministe (10/10 runs identiques sur le verdict).
3. Le **scénario S6 `crash-during-cancel`** passe vert (Phase 6 idéale) OU est explicitement reporté Phase 7 dans LESSONS avec justification (Phase 6 partielle acceptable).
4. Les **trois dettes Phase 6 (D9, D-Q-V2.2, D-Q-V2.6)** sont marquées résolues dans `TODO.md` (ou ramenées à un résiduel chiffré clair).
5. **Les 53 tests existants restent verts**, plus au moins 8 nouveaux tests (3 file bornée + 3 crash + 2+ watchdog profils + S5 + éventuellement S6).
6. `os-poc-reconstruct` rend lisibles : `priority_class` dans `0x0C`, `0x11`/`0x12`, profil watchdog dans `Spawned`.
7. Le harness `scenarios/run-all.sh` inclut S5 (et S6 si livré). Rapport JSON étendu.
8. LESSONS contient au moins quatre nouvelles entrées (L50–L53+), une par livrable bloquant.
9. Aucun ADR accepté antérieur n'est modifié — uniquement ajout d'ADR de remplacement si besoin.

Le chantier est **partiellement réussi** si :

- Les 4 ADR sont mergés ET S5 passe vert, mais S6 est reporté.
- Ou : les 4 ADR sont mergés ET la file bornée + atomicité crash sont livrées, mais S5 est instable (> 20 % de flakiness sur la priorité) et marqué "démonstration qualitative" dans LESSONS. Dans ce cas, `max_starvation_ms` doit être documenté avec sa marge réelle observée.
- Document honnêtement l'écart : quelles propriétés C1 sont démontrées (typiquement E1 ordinal et borne dure), lesquelles restent invérifiées (typiquement E2 fair-share pondéré).

Le chantier est **à reconsidérer** si :

- La Semaine 1 dépasse 10 jours réels : signal que l'arbitrage des 4 ADR (notamment Q-Ph6-1) est bloqué. Action : escalader la décision à l'humain avec les options A/B/C et leurs précédents.
- La file bornée régresse sur l'un des 53 tests existants et ce n'est pas rattrapable en 2 jours. Action : revenir au `Semaphore` plat, livrer ADR-0022 sans implémentation, marquer comme "design retenu, implémentation Phase 7".
- L'atomicité crash s'avère imposer une refonte majeure de l'interface `InferencePool` ↔ `Scheduler` (> 500 LoC modifiées). Action : accepter D-Q-V2.2 comme dette permanente avec mitigation par `os-poc-reconstruct` (déjà partiellement fait), refermer ADR-0024 en *Rejetée* avec justification, conserver les ADR-0022/0023/0025.
- Le watchdog calibration provoque des traps faux-positifs sur les scénarios S1–S4 existants. Action : élargir les profils par défaut, ne pas dégrader les tests existants.

---

## 8. Méthode de travail

- **Granularité de commit.** Un commit par sous-livrable (chaque ADR, chaque brique Ph6-B*, chaque scénario). Messages descriptifs préfixés `phase6:`.
- **TDD obligatoire sur la file bornée.** Tests unitaires d'`InferenceQueue` (Ph6-B5) écrits avant l'implémentation. Au minimum : borne, priorité, FIFO intra-classe, promotion famine.
- **Test S5 prioritaire** une fois la file en place. C'est le critère de succès n°2.
- **CrashPoint isolé par feature flag** (`#[cfg(feature = "crash-injection")]`). Vérification que le binaire release n'embarque pas le symbole.
- **Pas de modifications ABI.** Si un test exige une nouvelle host function, c'est qu'on a déraillé du scope Phase 6. Réfléchir avant de coder.
- **LESSONS au fil de l'eau.** Chaque surprise > demi-journée → entrée LESSONS immédiate. Ne pas attendre la fin de phase.
- **Auto-critique honnête.** Si l'`InferenceQueue` "marche" mais que la priorité est en pratique probabiliste (Tokio scheduler), c'est documenté dans LESSONS même si S5 est vert.

---

## 9. Questions ouvertes pour l'agent CLI

Si l'une de ces questions devient bloquante, ne pas improviser — formuler la question avec contexte et options, demander arbitrage à l'humain.

- **Q-OPEN-Ph6-1.** Si Tokio scheduler rend la priorité non-déterministe au point que S5 est flaky même avec instrumentation runtime, faut-il (a) passer à un scheduler maison (single-threaded Tokio runtime + queue manuelle), (b) accepter un test E1-uniquement (FIFO global) en S5 et reporter la priorité sémantique en Phase 7 ? Recommandation : essayer (a) d'abord (pas de nouveau code, juste `tokio::runtime::Builder::new_current_thread()` pour le test), puis (b) en fallback.
- **Q-OPEN-Ph6-2.** Le `AgentProfile` auto-déclaratif est-il acceptable comme garde-fou en Phase 6, ou faut-il un mécanisme tiers dès maintenant ? Recommandation : auto-déclaratif suffit en Phase 6 (c'est un watchdog, pas une cap), mais à noter explicitement comme dette pour Phase 7 si un cas d'usage adversarial émerge.
- **Q-OPEN-Ph6-3.** Faut-il une nouvelle capability `cap:inference` (rate-limiting LLM par agent) en Phase 6 ? Recommandation provisoire : non. Cohérent avec ADR-0019 §Q5.2 ("Note Phase 6"). Si une nécessité réelle apparaît (e.g. un agent malveillant qui sature le pool), à arbitrer au moment où elle se présente.
- **Q-OPEN-Ph6-4.** L'enrichissement de `0x0C` est-il rétro-compatible avec les decoders MessagePack existants ? Recommandation : vérifier en Semaine 1, première chose. Si non-compatible, créer un nouveau EmitType `0x13 InferenceAdmitted` (option 3 de Q-Ph6-4) au lieu d'enrichir `0x0C`.
- **Q-OPEN-Ph6-5.** Faut-il considérer un mode "test forensique" qui désactive la promotion famine pour valider E1 pur (FIFO strict intra-classe sous toutes conditions) ? Recommandation : oui, mais comme flag de test (`InferenceQueue::with_starvation_disabled()`), pas dans l'API production.

---

## 10. Ressources

- **Tokio sync primitives :** https://docs.rs/tokio/latest/tokio/sync/index.html — `Semaphore`, `Notify`, `Mutex`.
- **Stride scheduling** [Waldspurger 1995] *Stride scheduling: deterministic proportional-share resource management*, MIT/LCS/TM-528.
- **Lottery scheduling** [Waldspurger & Weihl 1994] *Lottery Scheduling: Flexible Proportional-Share Resource Management*, OSDI '94.
- **Linux CFS** : https://www.kernel.org/doc/html/latest/scheduler/sched-design-CFS.html
- **Failpoint pattern (FoundationDB, TiKV) :** https://github.com/tikv/fail-rs — référence pour le trait `CrashPoint`.
- **Spec interne :** `spec/07 §C1` plafond inférence, `spec/02 §P1b/§P6`, ADR-0014 (pas de retry), ADR-0019 (ABI).

---

## Annexe A — Structure cible du repo après Phase 6

```
poc/
├── store/                  (inchangé)
├── causal-log/             (ajout EmitType 0x11 CompensationOpen, 0x12 CompensationClose ; 0x0C enrichi)
├── capabilities/           (inchangé)
├── runtime/
│   ├── src/
│   │   ├── actor.rs              (modifié : profil watchdog par agent)
│   │   ├── inference_queue.rs    (NOUVEAU : remplace inference_pool.rs ou y est ajouté)
│   │   ├── watchdog.rs           (NOUVEAU : constantes par AgentProfile, override config)
│   │   ├── crash_point.rs        (NOUVEAU : feature-gated, no-op en release)
│   │   └── lib.rs                (tests s5_*, s6_*)
│   └── ...
├── reconstruct/            (ajout résumés 0x11, 0x12, priority_class de 0x0C)
├── agent-sdk/
│   ├── src/lib.rs                (ajout enum AgentProfile)
│   └── examples/
│       └── ...                   (agents existants inchangés ; ajout density_worker_supervisor_profile.rs si besoin)
└── scenarios/
    ├── S1-supervision-algorithmique/   (existant)
    ├── S2-self-rollback-incoherence/   (existant)
    ├── S3-inference-cap/               (existant)
    ├── S4-scheduler-rollback/          (existant)
    ├── S5-fairness-priority/           (NOUVEAU)
    │   └── README.md
    ├── S6-crash-during-cancel/         (NOUVEAU — optionnel)
    │   └── README.md
    └── run-all.sh                      (étendu — S5, S6)

decisions/
├── 0022-inference-queue-bornee.md       (NOUVEAU — discipline d'ordonnancement)
├── 0023-equite-borne-attente.md         (NOUVEAU — définition formelle équité)
├── 0024-atomicite-crash-cancel-rollback.md  (NOUVEAU — D-Q-V2.2 résolue)
└── 0025-profils-watchdog-wasm.md        (NOUVEAU — D9 résolue)
```

---

## Annexe B — Glossaire Phase 6

- **`InferenceQueue`** : remplaçant de `InferencePool` (sémaphore plat). File bornée avec discipline d'ordonnancement (FIFO + priorité), capacité paramétrable. Implémente la politique tranchée par ADR-0022.
- **`AgentProfile`** : enum déclaré par l'agent au spawn, calibre le watchdog (`Algo`, `LlmShort`, `LlmLong`, `Batch`). N'est pas une capability.
- **`CrashPoint`** : trait d'injection de panique pour tests, feature-gated (`crash-injection`). No-op en release.
- **Promotion famine** : mécanisme de garde-fou ; une requête en attente plus de `max_starvation_ms` est promue d'un cran de classe (`Batch → Foreground → Supervisor`). Limitée à une promotion par requête (Q-Ph6-1 garde-fou famine borné).
- **`CompensationOpen (0x11)` / `CompensationClose (0x12)`** : EmitType marquant le début et la fin d'une transaction de rollback en présence d'une inférence en vol. Détection des orphelins au recovery (ADR-0024 stratégie J).
- **Réconciliation au recovery** : politique appliquée par `os-poc-reconstruct` quand un `0x11` est observé sans `0x12` correspondant. Auto-close + warning par défaut, configurable.
- **E1 (équité ordinale)** : FIFO strict intra-classe. Mesure : `admission_ts` monotone implique `slot_acquired_ts` monotone (par classe).
- **E3 (absence de famine bornée)** : toute requête obtient un slot en `max_wait_ms`. Mesure : `wait_time_ms = slot_acquired_ts − admission_ts ≤ max_wait_ms` pour 100 % des requêtes.

---

*Fin du briefing Phase 6 v1. Trancher les ADR avant de coder.*
