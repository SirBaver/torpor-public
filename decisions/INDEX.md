# Index des ADRs

Ce fichier répertorie tous les ADRs du projet avec leur statut, leurs dépendances et leurs relations. Source de vérité pour naviguer la chaîne de décisions.

---

## Statuts

| Label | Sens |
|-------|------|
| **Acceptée** | Décision active, applicable |
| **Acceptée (provisoire)** | Décision active, avec condition de réévaluation explicite |
| **Acceptée, amendée** | Décision partiellement révisée par un ADR postérieur — lire aussi l'amendeur |
| **Réservé** | Numéro réservé, décision non encore rédigée — critère de déclenchement documenté |

---

## Table des ADRs

| N° | Titre | Date | Statut | Amendé par | Amende | Liés |
|----|-------|------|--------|------------|--------|------|
| [0001](0001-priorite-proprietes.md) | Ordre de priorité P1–P6 | 2026-05-10 | Acceptée | — | — | 0002, 0006 |
| [0002](0002-choix-substrat.md) | Choix du substrat PoC (Wasmtime + RocksDB) — **amendé 2026-06-05** : §Choix du moteur de stockage Layer 0 (comparatif SQLite/LevelDB/LMDB/RocksDB ; formalise L17 ; renvoi rôle redb seL4) | 2026-05-10 | **Acceptée, amendée** | — | — | 0011, 0010, 0042, 0043 |
| [0003](0003-modele-causal-dag.md) | Modèle causal : `caused_by[]` DAG | 2026-05-12 | Acceptée | — | — | 0008 |
| [0004](0004-schema-memoire.md) | Schéma mémoire : namespaces + clés canoniques | 2026-05-12 | Acceptée | — | — | 0012 |
| [0005](0005-design-capabilities-revoke.md) | Design capabilities et révocation (H-revoke) | 2026-05-13 | Acceptée | — | — | 0007 |
| [0006](0006-modele-supervision.md) | Modèle de représentation du log causal (A/B/C) | 2026-05-13 | **Acceptée, amendée** | 0009 (modèle A→B), 0013 (scope) | — | 0009, 0013, 0014 |
| [0007](0007-rollback-caps-invalidation.md) | Invalidation des capabilities lors d'un rollback | 2026-05-13 | Acceptée | — | — | 0005 |
| [0008](0008-causalite-concurrente.md) | Causalité concurrente : session exclusive + locking optimiste | 2026-05-13 | Acceptée | — | — | 0003, 0007 |
| [0009](0009-profils-acteurs-llm-separation-machine-humain.md) | Profils d'acteurs LLM (T/D) + séparation machine/humain | 2026-05-14 | Acceptée | — | Amende 0006 (modèle A→B) | 0006, 0010 |
| [0010](0010-contrat-emit.md) | Contrat de `emit()` : format, stockage, reconstruction | 2026-05-14 | **Acceptée, amendée** | 0017 (BlobDB CF), 0018 (reconstruct §5) | — | 0009, 0011 |
| [0011](0011-options-rocksdb-layer0.md) | Options RocksDB pour le Layer 0 (log causal) | 2026-05-14 | **Acceptée, amendée** | 0035 (write_buffer, bytes_per_sync) | — | 0002, 0010 |
| [0012](0012-memoire-semantique-sessions-bornees.md) | Mémoire sémantique : sessions bornées + résumé causal | 2026-05-14 | Acceptée | — | — | 0004, 0009, 0010 |
| [0013](0013-architecture-supervision.md) | Architecture supervision : canaux, états d'attente, hiérarchies | 2026-05-14 | **Acceptée, amendée** | — | Clarifie scope de 0006 ; amendée par 0057 (active le trigger D2, différé) puis **0059** (trigger déclenché, décomposition réalisée ; errata réf. « 0014 » → 0059) | 0006, 0010, 0012 |
| [0014](0014-politique-supervision.md) | Politique supervision : timeout, watchdog, retry, escalade | 2026-05-14 | Acceptée | — | — | 0013, 0010, 0012, 0005 |
| [0015](0015-propagation-erreur-cross-agent.md) | Propagation d'erreur cross-agent — `EmitType::AgentCrash 0x13` (cause + parent_agent_id + last_action_id), portée parent direct, pas d'action automatique, complémentaire à ADR-0014 §D14.d | 2026-05-17 | Acceptée | — | Étend 0010 (EmitType 0x13), à étendre 0018 (résumé payload 0x13) | 0010, 0013, 0014, 0018, 0024, 0025 |
| [0016] | Escalade typée et destinataires | — | **Réservé** | — | — | 0014 §D14.d |
| [0017](0017-blobdb-cf-default-amendement-adr0010.md) | BlobDB sur CF `default` — amendement ADR-0010 §Conséquences | 2026-05-15 | Acceptée | — | Amende 0010 (BlobDB CF `emit` → CF `default`, différé Phase 3) | 0010, 0011 |
| [0018](0018-os-poc-reconstruct-minimal.md) | `os-poc-reconstruct` : log-dump minimal Phase 2 | 2026-05-15 | Acceptée | — | Remplace 0010 §5 (algorithme reconstruction) | 0010, 0011, 0009, 0012 |
| [0019](0019-primitive-agent-infer.md) | Primitive `agent_infer` (ABI, async, cancellation, double timeout, Inference* EmitType 0x0C–0x0F) | 2026-05-16 | Acceptée | — | Étend 0010 (4 EmitType), à étendre 0018 (résumés payload 0x0C–0x0F) | 0005, 0007, 0010, 0014, 0017, 0018 |
| [0020](0020-toolchain-agent-sdk.md) | Toolchain agent SDK : cible `wasm32-unknown-unknown`, crate `agent-sdk`, pattern `process()` | 2026-05-16 | Acceptée | — | — | 0019, 0002 |
| [0021](0021-convention-scenarios.md) | Convention de scénarios de test (structure, nommage, format report.json, reproductibilité sémantique) | 2026-05-16 | Acceptée | — | — | 0018, 0019, 0020 |
| [0022](0022-file-inference-bornee.md) | File d'inférence bornée — priorité multi-niveau, drop-newest avec éviction Batch, enrichissement payload `0x0C` (`priority_class`, `queue_depth_at_admission`, `promoted_from`) | 2026-05-16 | Acceptée | — | Active `NoSlot (3)` réservé d'ADR-0019 §Q6 | 0014, 0017, 0019, 0023, 0025 |
| [0023](0023-equite-formelle.md) | Équité formelle E1 (ordinale intra-classe) + E3 (absence de famine bornée, `max_wait_ms = 30 000`, `max_starvation_ms = 10 000`) | 2026-05-16 | Acceptée | — | — | 0014, 0018, 0019, 0021, 0022 |
| [0024](0024-atomicite-crash-inference.md) | Atomicité crash `(0x0E, 0x0B)` — journal de compensation via `CompensationOpen 0x11` / `CompensationClose 0x12`, failpoint `CrashPoint` feature-gated | 2026-05-16 | **Acceptée, amendée** | 0027 (régime durabilité D1) | Résout dette D-Q-V2.2 d'ADR-0019 §Q-V2.2 | 0010, 0014, 0018, 0019, 0022, 0027 |
| [0025](0025-profils-watchdog-wasm.md) | Profils watchdog WASM — enum `AgentProfile { Algo, LlmShort, LlmLong, Batch }`, `EPOCH_TICK_MS_BASE = 10`, profil inscrit dans `Spawned (0x01)` | 2026-05-16 | Acceptée | — | Close résiduel D9 d'ADR-0019 §Q-V2.1 | 0010, 0014, 0018, 0019, 0020, 0022 |
| [0026](0026-regime-cache-reference-p3a.md) | Régime de cache de référence pour P3a — introduit « cache-mixte contraint » (drop_caches + RAM/dataset ≤ 2×) comme régime représentatif ; amende test-protocol §2.3 et §5 | 2026-05-18 | Acceptée | — | Amende `benchmarks/test-protocol.md §2.3, §5` | — |
| [0027](0027-durabilite-log-vs-contentstore.md) | Régime de durabilité log vs ContentStore — `append()` non-durable suffit sous modèle de menace SIGKILL/panic ; `append_durable` réservé à la mesure P3b ; power-loss reporté Phase 7+ ; **clarification post-SEF-4 (2026-05-18)** : RocksDB buffer applicativement, P6 atomique par action confirmée, garantie « append OK ⇒ survit SIGKILL » non garantie | 2026-05-18 | Acceptée | — | Amende ADR-0024 §D1 (régime durabilité), ADR-0010 (contrat append), `spec/02 §P6` (modèle de menace) | 0010, 0011, 0015, 0019, 0024, 0026 |
| [0028](0028-horloge-substituable-clock.md) | Horloge substituable `Clock` (trait + `SystemClock` + `LogicalClock`) — satisfait S6 ; tous les call-sites runtime qui inscrivent un timestamp dans une structure hashée (`SnapshotHeader.ts_us`, `LogEntry.ts_ms`, `EmitEnvelope.ts_us`) passent par `state.clock.now_*()` ; constructeurs préexistants rétro-compatibles via `SystemClock` par défaut ; débloque SEF-6 (P5) | 2026-05-18 | Acceptée | — | Implémente `spec/02b §S6`, débloque `spec/02 §P5` (SEF-6) | 0001, 0010 |
| [0029](0029-sef3-scope-covers-cap-denied.md) | SEF-3 : `scope_covers` préfixe + émission `CapabilityDenied (0x14)` côté runtime — portée P4 (isolation capabilities) | 2026-05-18 | Acceptée | — | — | 0005, 0007, 0010, 0018 |
| [0030](0030-scheduler-unifie-c1-c2.md) | Scheduler unifié C1+C2 — `IoAdmissionQueue` (C2, cap_actif paramétrable, priorité + affinité cache), pipeline C2→C1, `Scheduler::reap()` câblé dans `register()` | 2026-05-22 | **Acceptée, amendée** | 0031 (§FutureWork wake/admission) | — | 0022, 0023, 0011, 0027 |
| [0031](0031-scheduler-coordinator-reveil-a-la-demande.md) | SchedulerCoordinator — réveil à la demande (Option B) comme baseline ; admission prédictive (Option A) reportée tant que pas de mesure de latence justifiant la complexité ; `Scheduler::deliver` + `EvictedState.evicted_at` + S12 à produire | 2026-05-23 | Acceptée | — | Amende 0030 §FutureWork (tranche le design wake/admission) | 0030, 0022, 0011 |
| [0032](0032-refutation-hypothese-thermique-p99.md) | Réfutation hypothèse thermique p99 P3b — Spearman −0.50 / OLS |b/se_b|=3.06, deux critères FAIL ; cause retenue : compaction L0 RocksDB aléatoire ; borne P3b non révisée ; T5-ter comme suite | 2026-05-23 | Acceptée | — | — | 0026, 0033 |
| [0033](0033-critere-fuite-memoire-lsm.md) | Critère de fuite mémoire pour workload LSM — OLS sur `RSS − cur-size-all-mem-tables` (Option b) ; exposition `ContentStore::get_rocksdb_int_property` ; re-run T6-soak requis pour verdict valide | 2026-05-23 | Acceptée, amendée | 0034 | — | 0032, 0002 |
| [0034](0034-refutation-fuite-memoire-t6-soak.md) | Réfutation H-fuite-mémoire — RSS borné par caches RocksDB (memtable 256 MB + block cache 512 MB) ; OLS sur rss_adj retiré (spikes allocateur non restituable) ; budget RSS ~793 MB N=500 ; dette T6-soak CLOSED | 2026-05-24 | Acceptée | — | 0033 | 0033, 0002 |
| [0035](0035-config-rocksdb-explicite.md) | Config RocksDB explicite (P1/P2/P3B) — remplace `optimize_level_style_compaction` par valeurs cohérentes (`write_buffer=64 MB`, `max_write_buffer_number=2`, `max_bytes_for_level_base=256 MB`) ; ajoute `bytes_per_sync=1 MB` sur CausalLog + ContentStore ; `block_cache_usage_bytes()` pour enrichir rss_adj | 2026-05-24 | Acceptée | — | Amende 0011 (write_buffer, bytes_per_sync) | 0011, 0032, 0034 |
| [0036](0036-autorité-causale-agent-add-cause.md) | Autorité causale `agent_add_cause` — B-light (existence O(1) dans CausalLog), `MAX_EXTRA_CAUSES=16`, fail-closed (-3) ; contrat sémantique DAG vs fenêtre cognitive (spec/08 §0.1) | 2026-05-25 | **Acceptée (modèle d'autorité remplacé partiellement par 0058)** | 0057 arme le trigger §66 (log partagé) ; **0058 = B-fort, remplace le modèle d'autorité** | — | 0003, 0008 |
| [0037](0037-stack-runtime-sel4.md) | Stack runtime acteur sur seL4 — Wasmtime `min-platform` (no_std, Cranelift) + executor Rust maison ; N agents dans 1 VSpace seL4 ; isolation S1b sandbox WASM | 2026-05-27 | **Acceptée** | — | — | 0002, 0020, 0030, 0036 |
| [0038](0038-store-natif-sel4.md) | Store natif seL4 — ring buffer mémoire partagée + 1 IPC commit ; durabilité niveau (1) serveur RAM (analogue ADR-0027 SIGKILL) ; atomicité Q3-C content-addressed + log atomique ; store partagé par agent_id ; serveur de stockage in-TCB | 2026-05-27 | **Acceptée** | — | — | 0027, 0037, 0011, 0035 |
| [0039](0039-cible-poc-aarch64.md) | Cible PoC Phase 8 — AArch64 QEMU `virt` (Cortex-A57) ; portage x86_64 différé à Phase 9 ; séquence jalons C.1 (hello world officiel) / C.2 (root task custom) / C.3 (Wasmtime) | 2026-05-27 | **Acceptée** | — | — | 0037, 0038 |
| [0040](0040-chemin-sel4-hyperviseur-vs-natif.md) | Chemin seL4 Phase 9 — substrat natif seL4 (Chemin B) vs Linux invité (Chemin A) ; B retenu : spec/08 §0.2 exclut Linux dans TCB (~30 MLOC), mono-machine ne justifie pas VM, C.3 valide empiriquement viabilité B | 2026-05-28 | **Acceptée** | — | — | 0037, 0038, 0039 |
| [0041](0041-voie-b2-driver-block.md) | Voie B2 driver block seL4 — `sel4-virtio-blk` Rust no_std (30 LOC, rev 7a2321f2) retenu ; voie NVMe rejetée (QEMU = virtio, 6–9 mois) ; voie sDDF rejetée (incompatibilité microkit vs root task) | 2026-05-28 | **Acceptée** | — | — | 0038, 0039, 0040 |
| [0042](0042-voie-b3-moteur-index.md) | Voie B3 moteur d'index seL4 — redb v4 fork no_std retenu ; P3a validé (p99=739 µs, ×13 sous cible 10 ms, ×2 meilleur que RocksDB) ; taille DB 23 GB / 10⁸ entrées (ratio 2.1×) | 2026-05-28 | **Acceptée, amendée** | — | Amendée par 0043 (rétractation « ACID complet » : redb = index reconstructible, jamais store direct) | 0038, 0041 |
| [0043](0043-integration-verticale-c6.md) | Intégration verticale C.6 — topologie 2-processus (runtime/serveur, 2 VSpaces, ring partagé, seL4_Call) corrigeant l'inversion C.5 ; validation P6 par `tcb_suspend` aux 4 kill_points Q3-C ; oracle dans serveur survivant (I1/I2/I3) ; portée bornée mono-agent ; découpage C.6 / C.6-crash | 2026-05-29 | **Acceptée** | — | Amende 0042 (rôle redb) | 0038, 0042, 0027, 0037, 0041 |
| [0044](0044-integration-verticale-c7.md) | Intégration verticale C.7 — N TCB VSpace partagé, dispatch badge=agent_id (kind dans label), serveur séquentiel, I3-N par agent + I4 (non-interférence d'intégrité Biba) ; amende ADR-0038 §Q4 (badge vs payload) ; précise ADR-0037 §3 (executor différé) ; découpage C.7-A / C.7-crash | 2026-05-29 | **Acceptée** | — | Amende 0038 §Q4, précise 0037 §3 | 0043, 0038, 0037, 0030, 0031, 0027 |
| [0045](0045-critere-completude-poc-sel4.md) | Critère de complétude PoC seL4 — Q1=B (done = chaîne commit persistante end-to-end C.8 + P3a re-validée redb/virtio-blk) ; Q2=α (power-loss hors scope, durabilité niveau 1 ack seulement) ; matrice P1–P6/I4 par substrat ; garde-fou : ack `Committed` ≠ durabilité média. **Amendé 2026-05-29** : critère 2 P3a reformulé (fonctionnel seL4 + latence Linux/NVMe, QEMU non recevable) ; persistance reopen non démontrée (dette D-reopen) | 2026-05-29 | **Acceptée (amendée)** | 0049 (retrait argument invariant journal-autoritaire dans §Pourquoi pas A ; conclusion B inchangée) | — | 0038, 0042, 0043, 0044, 0027 |
| [0046](0046-scope-phase-9.md) | Scope Phase 9 — consolidation persistance seL4 ; critère bloquant = D-reopen (smoke test write→arrêt→reopen→read sans wipe) ; GC orphelins + N>2 optionnels conditionnés ; power-loss/β renvoyé Phase 10+ (QEMU non recevable comme substrat de validation power-loss) | 2026-05-29 | **Acceptée** | — | — | 0045, 0038, 0027, 0044 |
| [0047](0047-jalon-c10-wx-jit-sel4.md) | Jalon C.10 — durcissement W^X du pool JIT Wasmtime (dette S1 revue C.1→C.9) ; ouvre C.10 (ne pas dégeler c8) ; portée C.10-minimal (module confié) + test négatif obligatoire ; runtime remappe (cap VSpace+frames, pas de retype délégué) ; CNode runtime size_bits=8 ; smoke P6 (protocole §71 inchangé) ; découpage C.10/C.10-crash ; non-confié → C.11 | 2026-05-29 | **Acceptée (cadrage)** | — | — | 0037, 0040, 0043, 0045, 0036 |
| [0048](0048-jalon-c11-wasm-non-confie.md) | Jalon C.11 — chargement WASM non confié sur JIT durci (déclencheur T1 confirmé) ; « non confié » = option C déclinée (C.11 cœur = contenu adversarial / C.11-prov = canal non-trusted, pas de signature) ; C11_PASS = 4 propriétés P-α (isolation OOB) / P-β (terminaison boucle) / P-γ (store survit) / P-δ (rejet cwasm malformé) ; F1 = pas de JIT à l'exécution (features=runtime sans cranelift) → vecteur Cranelift-hostile inexistant ; trap=panic acceptable (isolation processus seL4, S4 requalifiée → C.12+ quand N agents/VSpace) ; terminaison = watchdog seL4 (tcb_suspend), pas fuel ; limites Store mémoire/stack ; smoke P6 (protocole §71 inchangé) | 2026-05-29 | **Acceptée (cadrage)** | — | — | 0047, 0037, 0043, 0046, 0025, 0036 |
| [0049](0049-cloture-poc-sel4.md) | Clôture du PoC seL4 — déclencheurs objectifs épuisés (C.10/C.11/C.11-prov soldés) ; D1 PoC clos ; D2 la séparation CAS-autoritaire/index-reconstructible (ADR-0038 §3, 0042) est une **cible non instanciée** — store réel = redb transactionnel monolithique (L82), promu au récit de complétude ; D3 déclencheurs dormants (a: GC ← croissance non bornée ; b: power-loss ← matériel réel ; C.12+ ← N agents/VSpace, PKI) ; D4 remontée spec (re-instruire Q-seL4-2) | 2026-05-30 | **Acceptée** | 0051 (§D3a requalifié — re-séparation a un défaut de sûreté attaché) | Amende 0045 §Pourquoi pas A (justification, pas conclusion) | 0045, 0046, 0038, 0042, 0048, 0027, 0036 |
| [0050](0050-campagne-mise-a-lepreuve.md) | Campagne de mise à l'épreuve adversariale — gate soundness (L68/L82, verdict INSTANCIÉE/PROXY/SUR-GARANTIE par propriété) préalable bloquant ; axe 1a isolation P4 (oracle = état caps) + 1b fidélité log d'audit sous flood (oracle hors-bande au point de décision, cible F2 rate-limit `0x14`) ; axe 3 crash machine concurrent (régime power-loss-like ADR-0027 §D3, cache invalidé) ; axe 2 plafonds COUPÉ (inférence stubbée F1) ; LLM non-objectif (spec/08 §0.1) ; P1/P2/P3/P5 hors cibles ; substrat Linux (non-transférable seL4) ; ordre gate→1→3 | 2026-05-30 | **Acceptée (cadrage)** | 0051 (soldé — findings triés) | — | 0001, 0027, 0049, 0021, 0029 |
| [0051](0051-cloture-campagne-tri-findings.md) | Clôture campagne adversariale : tri des 8 findings — amendements spec/02 (§P2 O(log N)→O(depth) ; §P4 audit qualifié+rehaussé ; §P6 asymétrie orphelin/pendant + trou cross-store) ; correctifs code #6 (agrégation rate-limit `0x14` par resource bornée → masquage levé, P4) + #7a (restore défend `last_snapshot ∈ store`, fail-safe P6) ; différés #7b (commit cross-store atomique ← GC) / #8 (durabilité power-loss ← matériel, groupé D-P3a/β-seL4) ; #3 P5 = dette d'oracle ; requalifie ADR-0049 §D3a (re-séparation a désormais un défaut de sûreté attaché) | 2026-05-30 | **Acceptée** | — | Amende 0049 §D3a (dossier re-séparation), solde 0050 | 0050, 0049, 0027, 0001 |
| [0052](0052-scope-phase-10-inference-reelle.md) | Scope Phase 10 : exercer le scheduler d'inférence (`InferenceQueue` ADR-0022/0023, coordination C1↔C2 ADR-0030, evict/wake ADR-0031) sous backend réel (`OllamaBackend/qwen2.5:3b`) — falsifier E1/E3/P1b (oracle `QueueTrace`/`queue_stats()`) ; non-objectifs : C2 recalibré hardware (condition ADR-0050 §D6 partiellement ouverte), #7b, #8/D-P3a, multi-tenant, C.12+ ; garde-fou non-transférabilité (calque D7/ADR-0050) ; scénarios S3+S5 à ré-exécuter sous OllamaBackend ; sous-axe B oracle P5 #3 conditionnel | 2026-05-30 | **Acceptée** | — | Lève partiellement ADR-0050 §D6 (moitié inférence réelle) | 0022, 0023, 0030, 0031, 0050, 0051, 0001 |
| [0053](0053-cadrage-campagne-p2-p3-p5.md) | Campagne adversariale P2/P3/P5 — gate de soundness (Q1 V2.1 tombe depth=100 ; Q2 V3.4 non-constructible ; Q3/G-P5 branche NON par défaut) ; A-P2 SEF-12 PASS (V2.2/V2.3/V2.4 vérifiés) ; A-P3 SEF-13 PASS (V3.3a/V3.3b vérifiés, V3.1/V3.2 fermés sans run) ; A-P5 clos sans code (sortie LLM hors préimage hash — debt #3 dormante inchangée, ADR-0051 §D5) ; substrat Linux PoC, garde-fou non-transférabilité | 2026-05-30 | **Acceptée (cadrage — scénarios clos)** | — | — | 0001, 0050, 0051, 0052 |
| [0055](0055-gc-contentstore-mark-and-sweep.md) | Garbage collection orphelins ContentStore — mark-and-sweep algo (pas de refcount) ; mode offline obligatoire ; métrique Δ = blocks−headers + OLS 10 min pour déclenchement ; itérateurs CF en lecture seule ; module GC dans `poc/runtime/src/` | 2026-06-02 | Acceptée | — | — | 0049, 0051, 0027, 0033 |
| [0056](0056-pulley-vs-cranelift-aot.md) | Interpréteur Pulley vs Cranelift AOT — différé : W^X soldé (C.10/ADR-0047, pas de dette ouverte) ; latence neutre (I/O redb domine) ; avantage signature réel mais conditionnel (PKI non atteinte) ; migration propre pour C.12+ mais non instruite. Réveil sur R1 (second substrat), R2 (PKI/multi-producteur), R3 (JIT réintroduit sur cible). Motif historique ADR-0037 §132 (instabilité) caduc — Pulley stable depuis Wasmtime 25+ | 2026-06-02 | Différé | — | — | 0037, 0046, 0047, 0048, 0049 |
| [0058](0058-modele-autorite-b-fort-causehandle.md) | Modèle d'autorité **B-fort** — `agent_add_cause` exige un `CauseHandle` (object-capability sur **action_id**, pas agent_id) au lieu d'une simple vérification d'existence. Registre `CauseHandleStore` dédié (≠ CapabilityStore, isolé par tenant) ; ABI WASM changée (`handle_id: i32`, `-1` disparaît, `-3` élargi) ; `Message.cause` → `Option<CauseHandle>` ; délégation interdite par défaut (D5) ; révocation à terminaison (`revoke_issued_by`) + rollback (`revoke_issued_after` — **émis**, pas détenu, D7) ; modèle uniforme, mono-tenant = cas dégénéré (auto-grant + mint local, **pas** de branchement sur DEFAULT) ; code `-3` réutilisé (pas d'oracle `-5`). `LogEntry` INCHANGÉ (option tenant_id rejetée : casserait l'action_id content-addressed). Risque n°1 = cohérence cache local ↔ store partagé sous révocation (tester via vrai appel WASM). Jalons BF-1/2/3 ; §D6/D7 **amendés par 0060** (révocation élargie au registre cross-tenant) | 2026-06-07 | Accepté (BF-0→BF-3 livré) | — | Remplace partiellement 0036 (modèle d'autorité) ; complète 0057 ; s'appuie sur 0005/0007 ; amendé par 0060 | 0036, 0057, 0005, 0007, 0003 |
| [0057](0057-forme-multi-tenant-causallog-partage.md) | Forme du multi-tenant — **PoC d'apprentissage assumé** (pas de besoin métier) : CausalLog + ContentStore **partagés** entre ≥2 tenants non-confiants, CapabilityStore **isolé** par tenant ; `TenantId` porté par `AgentState` + `.tenant()` sur builder (default = mono-tenant) ; arme le trigger ADR-0036 §66 (log partagé = vulnérabilité que B-fort fermera). Invariant MT-1 = INV-A (cap cross-tenant refusée) ∧ INV-B (forgerie causale cross-tenant réussit sous B-light → oracle inversé de B-fort). Dettes tracées : canal dédup ContentStore, pas de quota I/O/tenant, Scheduler tenant-blind (§D5 **résolue par 0059**) | 2026-06-07 | Acceptée | — | Complète 0036 (arme §66) ; amende 0013 (active trigger D2, décomposition différée) | 0036, 0013, 0055, 0005, 0030 |
| [0061](0061-referent-partage-par-tenant-garde-cablage.md) | **Référent KV partagé-par-tenant (P4 réel) + garde d'isolation de câblage** (revue sécurité, findings C1+M1). C1 : `AgentState.kv_store` passe de `HashMap` privé-par-agent à `Arc<Mutex<HashMap>>` partageable par tenant (disjoint entre tenants) — la capability garde désormais un référent RÉEL (avant : magasin privé inaccessible → P4 vide). M1 : `Registry::register` refuse (panic fail-fast) qu'un `cap_store` soit partagé par deux tenants distincts (`Arc::ptr_eq` + nettoyage au reap) — l'isolation par tenant passe de convention à invariant runtime. NE porte PAS `TenantId` dans `check()` (ADR-0057 §D2 inchangé). Invariants p4_kv_shared_within_tenant_cap_gated / m1_distinct_tenants_sharing_cap_store_rejected. Pertinent RFC-0001 (flotte déclarative câble les tenants). Limite : KV non persistant (FutureWork éviction) | 2026-06-07 | Accepté | — | Amende 0057 §D2 (isolation = invariant) + 0029 (référent P4) ; garde dans 0059 | 0057, 0029, 0059 |
| [0060](0060-revocation-cross-tenant-causehandle.md) | Révocation **cross-tenant** des `CauseHandle` — `CauseHandleRegistry` (`TenantId → store`) rendant tous les stores de tenant visibles à un point unique. Le drop-guard (terminaison, §D6) et le rollback (§D7) de `run_loop` balaient **tous** les stores (`revoke_issued_by_all` / `revoke_issued_after_all`), fermant le trou BF-2 (un handle émis par A∈T1 au profit d'un grantee de T2 vit dans le store de T2 et survivait à la mort de A). `AgentState` porte `Arc<CauseHandleRegistry>` ; store local DÉRIVÉ via `get_or_create(tenant)` (unique point d'insertion → risque n°1 clos). Révocation reste dans `run_loop`, jamais dans Scheduler (jurisprudence ADR-0014 §D14.b). Coût O(tenants×handles) accepté (index inverse différé). Limite : éviction/réveil → registre frais (FutureWork). Invariants INV-XR-CROSS / INV-XR-ROLLBACK / INV-XR-INTRA | 2026-06-07 | Accepté | — | Amende 0058 §D6/D7 ; s'appuie sur 0014 (jurisprudence run_loop) ; jumeau de 0059 | 0058, 0057, 0014, 0059 |
| [0059](0059-decomposition-registry-supervisor.md) | Décomposition du `Scheduler` en **`Registry`** (mécanisme : annuaire/routage/dormant) + **`Supervisor`** (politique : suspend/rollback/checkpoint/spawn_child). Réalise ADR-0013 §D2 (trigger déclenché — supervision cross-tenant testée) et ferme la dette ADR-0057 §D5. Autorité capability-style `SupervisionAuthority {Orchestrator, Tenant(t)}` : `Orchestrator` (runner trusted) ambiant cross-tenant, `Tenant(t)` refuse le cross-tenant (`CrossTenantDenied`, aucun effet). `Scheduler` = façade stable (INV-SD-NOREG). Audit du refus = **O1** (Err typé seul, pas d'EmitType — 0x14 inadapté, condition de bascule O1→O2 tracée). Errata : « ADR-0014 » de 0013 §D2 → lire **0059**. Invariants INV-SD-NOREG / INV-SD-AUTH / INV-SD-INTRA | 2026-06-07 | Accepté | — | Réalise 0013 §D2 ; ferme 0057 §D5 ; s'appuie sur 0029 (audit O1) | 0013, 0057, 0029, 0014, 0060 |
| [0062](0062-builder-canonique-instanciation-acteur.md) | **`ActorInstanceBuilder` chemin canonique + contrat builder pour le futur loader** (RFC-0001 §7.4, alt. (b)). Constat : la prémisse RFC « 8 chemins parallèles » est périmée — `build()` est déjà l'**unique** chemin (point fail-closed centralisé, ADR-0060/0061) ; les **8** façades `new_precompiled_*` + 2 `restore_from_evicted_*` (10 au total) y délèguent toutes. D1 : `build()` canonique acté. D2 : façades **gelées** (*legacy frozen set*) — migration des ~189 sites **rejetée** (churn à valeur architecturale nulle) ; règle « code nouveau → builder », **interdiction d'une 11ᵉ façade**. D3 : `restore_*` = chemin distinct légitime (build + réhydrate), régularisé pour passer explicitement par le builder ; **réveil = runtime, hors loader**. D4 (prescriptif, non implémenté) : contrat loader = `from_spec` data-driven + résolution `wasm_hash` CAS en amont + résolution **fail-closed** des caps déclarées (confused deputy). Format `InstanceSpec` = décision à part. | 2026-06-07 | Accepté | — | En lien RFC-0001 §7.4 ; préserve 0060 (risque n°1) + 0061 (garde M1) ; clôt périmètre 0031 (réveil) ; réserve contrainte 0005/0007 (mint fail-closed) | 0060, 0061, 0031, 0005, 0007 |
| [0063](0063-bibliotheque-routers-flotte-driver.md) | **Bibliothèque de Routers : `FleetDriver` + trait `Router`** (livrable P-faible de RFC-0001 §6 bis ; la RFC reste ABANDONNÉE). Module `poc/runtime/src/fleet/`. Le driver **référence** le `Scheduler` (jamais ne le possède) et n'appelle que la surface mécanisme (`register`/`send`/`tenant_of`), jamais la politique (`spawn_child`/`rollback`). **Pas de `CauseHandle` minté** (D3) : la causalité de flotte passe par le canal TCB `Message::caused`, dont l'unique alternative consultant le store est le chemin guest `agent_add_cause` — non sollicité par le modèle Router (invariant D3 bis : routage décidé par Router/TCB, jamais par l'agent guest). Un handle minté ici serait du code mort (cf. [[L133]]). **Mono-tenant strict (D4)** : la garde `tenant_of` du driver est la SEULE frontière inter-tenant (analogue `Supervisor::authorize`) ; cross-tenant DORMANT (trigger : flotte à ≥2 `TenantId` + témoin `Orchestrator` + audit 0x15). Le choix B-fort/B-light est **neutre** pour le code du driver (D3 ter). Incrément 1 : `FanInRouter` + `QuorumRouter` + test `inv_router_mono_tenant_no_cross_fanin` (oracle inversé + miroir positif, vrai cycle WASM). Borne P-faible : pas de `from_spec`, D4 d'ADR-0062 reste dormant | 2026-06-07 | Accepté | — | Réifie RFC-0001 §6 bis/§8 (n'en ressuscite pas le loader) ; s'appuie sur 0058/0060 (canal TCB vs chemin guest), 0059 (mécanisme/politique, gabarit autorité), 0062 (builder canonique) ; jurisprudence 0014 §D14.b | 0058, 0059, 0060, 0062, 0014 |

---

## Chaînes de décisions

Les ADRs forment des lignées thématiques. Lire dans l'ordre pour comprendre l'évolution.

### Supervision (modèle + chemin + politique)

```
ADR-0006  Modèle de représentation du log causal (A vs B vs C)
    └── amendé par →  ADR-0009  Profils acteurs (adopte Modèle B, sépare machine/humain)
                          └── contrat →  ADR-0010  Contrat emit()
    └── scope clarif →  ADR-0013  Architecture supervision (canaux, AwaitingValidation, Scheduler)
                          └── politique →  ADR-0014  Timeout, watchdog, retry, escalade
                                              └── propagation →  ADR-0015  Propagation erreur cross-agent
                                                                            (EmitType::AgentCrash 0x13)
                                              └── réservé →      ADR-0016  Escalade typée
```

### Causalité

```
ADR-0003  Modèle causal DAG (caused_by[])
    └── concurrence →  ADR-0008  Session exclusive + locking optimiste
```

### Capabilities

```
ADR-0005  Design capabilities + révocation (H-revoke)
    └── rollback →  ADR-0007  Invalidation caps lors d'un rollback
```

### Stockage Layer 0

```
ADR-0002  Choix substrat (Wasmtime + RocksDB)
    └── §moteur Layer 0 →  comparatif SQLite/LevelDB/LMDB/RocksDB (amendement 2026-06-05, formalise L17)
                              └── renvoi seL4 →  ADR-0042 / ADR-0043 (redb B+tree = index reconstructible, profil read-only)
    └── options →  ADR-0011  Options RocksDB (bloom filter, cache, agent_ts index)
    └── contrat →  ADR-0010  Contrat emit() (LogEntry.emit_payload, BlobDB)
                       └── amende §Conséq. →  ADR-0017  BlobDB sur CF `default` (pas de CF `emit`), différé Phase 3
                       └── remplace §5 →      ADR-0018  os-poc-reconstruct minimal (log-dump par agent)
```

### Mémoire agent

```
ADR-0004  Schéma mémoire (namespaces + clés canoniques)
    └── sessions →  ADR-0012  Sessions bornées + résumé causal (C3 plafond)
```

### Primitive d'inférence (PoC E2E)

```
ADR-0019  Primitive agent_infer (ABI, async, cancellation, double timeout)
    └── étend →    ADR-0010  EmitType 0x0C InferenceRequest, 0x0D InferenceResponse,
                              0x0E InferenceCancelled, 0x0F InferenceFailed
    └── à étendre →ADR-0018  Résumés payload 0x0C–0x0F dans os-poc-reconstruct
    └── invariant →ADR-0005, ADR-0007  Pas de cache de cap (Q5.2) — non impactés
    └── politique →ADR-0014           Pas de retry automatique (cohérent §Q4)
```

### Phase 6 — Propriétés fortes C1 + atomicité crash + watchdog calibré

```
ADR-0019  Primitive agent_infer (ABI figée, dettes D9, D-Q-V2.2, D-Q-V2.6)
    └── borne file →   ADR-0022  File d'inférence bornée (priorité 3 classes,
    │                            drop-newest + éviction Batch, NoSlot (3) activé)
    │                       └── équité →   ADR-0023  E1 + E3 (max_wait_ms = 30 s,
    │                                                max_starvation_ms = 10 s)
    └── atomicité →    ADR-0024  Journal de compensation 0x11 / 0x12 + failpoint
    └── watchdog →     ADR-0025  Profils AgentProfile (Algo, LlmShort, LlmLong, Batch)
                                  + override TOML + profil inscrit dans Spawned
```

### Priorités (arbitre tous les conflits)

```
ADR-0001  Ordre de priorité P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1
    (référence transversale — toute tension entre propriétés se résout par cet ordre)
```

---

## ADRs réservés — critères de déclenchement

### ADR-0016 — Escalade typée et destinataires

**Déclenché par :** (a) besoin de distinguer "escalade de premier ordre" vs "escalade répétée" dans le log, ou (b) introduction d'un `human_supervisor` comme acteur de plein droit nécessitant un AgentId réservé.

**Contexte :** ADR-0014 §D14.d a choisi l'observation passive via `verdict == Timeout` dans le log existant. ADR-0016 s'ouvre si ce filtrage devient insuffisant.

### ADR-futur — B-fort capability cross-agent

**Statut :** Dormant — ne pas instruire en mono-tenant.

**Déclenché par :** Première PR (ou design accepté) introduisant un second `TenantId` distinct dans `Runtime`, ou tout namespace logique partageant le même `CausalLog` entre principaux mutuellement non confiants.

**Contexte :** ADR-0036 §sortie a documenté le design (cap-handles dans `Message`, révocation à mort de l'émetteur, `Cap<T>` typé). B-light suffit pour le modèle de menace mono-tenant (cf. spec/08 §R1). Décision 2026-05-26 (architect) : ne pas pré-câbler de structures `Cap<T>` "au cas où" ; le coût de réveil ultérieur est borné (~2–3 jours, sites d'appel identifiés dans ADR-0036).

---

### Qualification empirique (P3 / stockage)

```
ADR-0026  Régime cache-mixte contraint (P3a)
    └── résultat →  ADR-0032  Réfutation hypothèse thermique p99 (cause retenue : compaction L0)
                       └── critère →  ADR-0033  Critère fuite mémoire LSM (RSS − memtable_usage)
                       │                  └── verdict →  ADR-0034  Réfutation H-fuite-mémoire (RSS borné, pas de fuite)
                       └── dettes →   ADR-0035  Config RocksDB explicite (P1 write_buffer, P2 bytes_per_sync, P3B block_cache)
```

---

### Sécurité — modèle de menace

```
ADR-0003  Cross-agent causality (structure DAG, format parent_ids)
    └── autorité →  ADR-0036  Modèle d'autorité agent_add_cause (B-light : existence O(1), MAX=16)
                       sortie →  [ADR-futur, DORMANT]  B-fort capability cross-agent
                                   Trigger : première PR introduisant un second TenantId dans Runtime.
                                   Tant que le projet est mono-tenant, ne pas instruire (décision 2026-05-26).
                                   Voir TODO.md §Sécurité — dettes ouvertes.
```

### Transition vers seL4

```
ADR-0002  Choix substrat PoC (Wasmtime + RocksDB + Tokio sur Linux)
    └── transfert →  spec/09  Tableau de transfert PoC → seL4 (2026-05-27)
    └── runtime →    ADR-0037  Stack runtime seL4 : Wasmtime min-platform + executor Rust maison
                                (Acceptée — PoC fumée validé 2026-05-27)
                        └── store →  ADR-0038  Store natif seL4 : ring buffer + IPC commit + Q3-C
                        │                      (Acceptée — B2 et B3 tranchés)
                        │               └── B2 driver → ADR-0041  Voie B2 : sel4-virtio-blk (C.4 PASS)
                        │               └── B3 index  → ADR-0042  Voie B3 : redb fork no_std (P3a 739 µs)
                        │                                  └── amendé → ADR-0043 (redb = index, jamais store direct)
                        │               └── intégration → ADR-0043  C.6 : topologie 2-processus + validation P6
                        │                                  └── N agents →     ADR-0044  C.7 : badge dispatch, serveur séquentiel, I3-N + I4
                        │                                  └── critère →      ADR-0045  Critère complétude PoC seL4 (Q1=B, Q2=α)
                        │                                                         └── scope → ADR-0046  Scope Phase 9 (D-reopen bloquant, power-loss Phase 10+)
                        │                                  └── durcissement → ADR-0047  C.10 : W^X pool JIT Wasmtime
                        │                                                         └── non confié → ADR-0048  C.11 : WASM non confié sur JIT durci
                        │                                  └── clôture →      ADR-0049  Clôture PoC seL4 (D1 clos, D2 non instanciée, D3 dormants)
                        └── cible →  ADR-0039  Cible PoC Phase 8 : AArch64 QEMU virt, x86_64 différé
                        └── chemin → ADR-0040  Chemin B natif seL4 (vs hyperviseur) — tranché 2026-05-28
```

### Campagne adversariale

```
ADR-0050  Campagne mise à l'épreuve (gate soundness → axe 1 → axe 3)
    └── gate →    SEF-8  Gate soundness : 5 INSTANCIÉE, 5 PROXY, 1 SUR-GARANTIE sur P1–P6 + SEF-7
    └── axe 1 →  SEF-9  Confused-deputy rate-limit ↔ audit (1a INTACTE, 1b ÉCHOUE sous flood)
    └── axe 3 →  SEF-10 Fenêtre de référence pendante cross-store (durabilité power-loss différée)
    └── clôture → ADR-0051  Tri findings : correctifs #6 (rate-limit par resource) + #7a (restore fail-safe) ;
                             différés #7b (commit atomique cross-store ← GC) / #8 (durabilité ← matériel)
```

---

*Format : [MADR](https://adr.github.io/madr/) — Dernière mise à jour : 2026-06-07*
