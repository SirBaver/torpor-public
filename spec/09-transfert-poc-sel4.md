# Spec/09 — Tableau de transfert PoC → cible seL4

**Date :** 2026-05-27 (créé) · 2026-05-30 (consolidé après clôture PoC seL4)
**Statut :** Référence vivante — à mettre à jour à chaque nouveau ADR structurant

> **Note de consolidation (2026-05-30, ADR-0049 §D4).** Ce document a été écrit le 2026-05-27 comme *plan prospectif*, quand seL4 était une cible non encore prototypée. Les §1/§2 (tableau de transfert ADR par ADR, synthèse chiffrée) restent valides comme **analyse de portabilité**. Les §3/§4 (questions ouvertes, prochaines étapes) ont été **réalisés et tranchés** par les jalons C.1→C.11-prov et les ADR 0037–0049 — ils sont marqués RÉSOLU ci-dessous. La §5 (nouvelle) capitalise la réalisation effective et porte les garde-fous de clôture. **Garde-fou central (ADR-0049 §D2) : le store du PoC seL4 n'instancie PAS la séparation « CAS autoritaire / index reconstructible » — c'est une cible non instanciée. Ne pas décrire le store réalisé comme la portant.**

---

## 0. Objet

Ce document répond à une question précise : **qu'est-ce qui transfère du PoC Rust/Wasmtime/RocksDB vers la cible seL4 ?**

Le PoC n'est pas un prototype de la cible. Il est un **falsificateur d'hypothèses architecturales** : il prouve que les propriétés P1–P6 sont satisfaisables en principe, sur au moins un substrat acceptable. Il ne prouve pas qu'elles seront satisfaites sur seL4.

Quatre catégories de transfert :

| Catégorie | Signification |
|-----------|---------------|
| **A — Transfère intégralement** | Modèle abstrait, invariant, ou algorithme indépendant du substrat. Aucune réécriture. |
| **B — Concept transfère, implémentation à refaire** | La décision de design est valide ; le code Rust qui l'implémente est lié à RocksDB/Wasmtime/Tokio et doit être réécrit. |
| **C — Méthodologie transférable** | Savoir-faire de qualification (protocoles, critères, pièges identifiés). Pas de code, mais précieux. |
| **D — Non portable** | Artefacts liés à Linux + RocksDB + glibc. Jetés. |

---

## 1. Tableau ADR par ADR

### Catégorie A — Transfère intégralement

| ADR | Titre | Ce qui transfère | Portabilité |
|-----|-------|------------------|-------------|
| **0001** | Priorité des propriétés | Ordre P4 > P2 > P3 > P6 > P5 > P1. Invariant de décision. | Intégrale |
| **0003** | Modèle causal DAG | `LogEntry` (agent_id, ts_ms, parent_ids, hash_before, hash_after), content-addressing SHA-256, sémantique Lamport *happened-before*. | Intégrale |
| **0004** | Schéma mémoire | `SnapshotHeader` (seq, hash_before, hash_after, ts_us), rollback par chaîne Merkle, idempotence par hash. | Intégrale |
| **0005** | Capabilities + révocation | Modèle scope-prefix, délégation, révocation récursive, confused-deputy résolu. seL4 implémente nativement un sur-ensemble (CNode + Revoke). | Intégrale |
| **0006** | Modèle supervision | Arbre supervisor/worker, restart policy, isolation des fautes. | Intégrale |
| **0007** | Rollback + invalidation caps | Algorithme rollback_path O(depth), invalidation caps au rollback, compensation journal. | Intégrale |
| **0008** | Causalité concurrente | Sémantique des `parent_ids` multiples (DAG vs chaîne linéaire), émission B-light. | Intégrale |
| **0009** | Profils acteurs | Séparation machine/humain, `AgentProfile` (Algo/LlmShort/LlmLong/Batch). | Intégrale |
| **0012** | Mémoire sémantique bornée | Sessions bornées N_max, pas de mémoire cross-session sans citation explicite. | Intégrale |
| **0013** | Architecture supervision | Supervision tree, isolation des espaces de caps, propagation d'erreur hiérarchique. | Intégrale |
| **0014** | Politique supervision | Politiques restart (one_for_one, rest_for_one), seuils d'escalade. | Intégrale |
| **0015** | Propagation erreur | `AgentCrash (0x13)` terminal event, synthèse `Lifecycle::Terminated` à la lecture. | Intégrale |
| **0021** | Convention scénarios | Structure README + run.sh + report.json, reproductibilité sémantique, K runs. | Intégrale |
| **0022** | File inférence bornée | 3 files FIFO (Supervisor/Foreground/Batch), cap bornée, `NoSlot (3)`. | Intégrale |
| **0023** | Équité formelle | Promotion anti-famine (Batch → Foreground après max_starvation_ms), invariants mesurables. | Intégrale |
| **0028** | Horloge substituable | `trait Clock` (SystemClock prod, LogicalClock replay), injection dans `AgentState`. Pattern de testabilité. | Intégrale |
| **0029** | SEF-3 scope | `scope_covers` préfixe, émission `CapabilityDenied (0x14)` côté runtime. | Intégrale |
| **0036** | Autorité causale B-light | `agent_add_cause` vérifie existence dans CausalLog, `MAX_EXTRA_CAUSES=16`, fail-closed (-3). Contrat sémantique DAG vs fenêtre cognitive (§0.1 spec/08). | Intégrale |

### Catégorie B — Concept transfère, implémentation à refaire

| ADR | Titre | Ce qui transfère | Ce qui est à refaire | Effort estimé |
|-----|-------|------------------|----------------------|---------------|
| **0002** | Choix substrat | Critères S1–S7. La grille d'évaluation est réutilisable pour auditer seL4 + couche acteur. | ADR-seL4 à écrire : stack concrète (couche acteur, store, IPC, durabilité), budget mémoire seL4, trade-offs. | Spec (3–5 j) |
| **0010** | Contrat emit | Séquence logique emit → commit_barrier → log_append → store_put, atomicité cross-store, `EmitType` table (0x01–0x14). | Le mécanisme d'atomicité cross-store dépend des primitives de transaction seL4. Sans WriteBatch RocksDB, il faut définir comment garantir P6 (rollback / crash) avec les primitives du store seL4. | Spec + impl |
| **0017** | BlobDB / structure store | Séparation `blocks` (content-addressed) + `headers` (Merkle headers) + `agent_ts` (index secondaire). La structure *logique* est portable. | Le moteur (RocksDB) n'existe pas sur seL4. La structure doit être réimplémentée sur un store-Rust maison ou un serveur de fichiers seL4 (Genode FS, etc.). | Impl (4–8 sem.) |
| **0018** | Reconstruct | Concept : outil de log-dump par agent, reconstruction de la chaîne causale, détection compensations orphelines. | L'implémentation lit RocksDB directement. Sur seL4, il lira le store natif via son API. Même concept, binaire à réécrire. | Impl (1–2 j) |
| **0019** | Primitive `agent_infer` | ABI host function (signature i32, codes retour 0/1/2/3/4), table d'émission 0x0C–0x0F, sémantique cancellation, pre-validation bounds. | L'implémentation host dépend de Wasmtime (`Caller`, `Memory::read/write`) et Tokio (`spawn`, `select!`). Sur seL4, la host function sera implémentée différemment (IPC kernel-mediated, pas de Tokio async nativement). | Impl |
| **0020** | Toolchain wasm32 | Les modules WASM compilés en `wasm32-wasip1` sont portables. L'ABI des host functions A1–A4 + agent_infer est stable. | Le runtime d'exécution (Wasmtime) doit tourner sur seL4. Wasmtime a un port Rust `no_std` expérimental ; à évaluer. Alternative : réécrire l'interpréteur ou utiliser WAMR (plus léger). | Évaluation (1 sem.) |
| **0024** | Atomicité crash + compensation | Journal 0x11/0x12 (CompensationOpen/Close), invariant : pas d'état partiel observable, détection 0x11 orphelins par reconstruct. | `WriteBatch` cross-CF est la primitive d'atomicité RocksDB. Sur seL4, la primitive doit être identifiée dans le store natif (write atomique, WAL équivalent). | Spec |
| **0025** | Watchdog WASM | Concept : budget d'exécution par `AgentProfile`, interruption coopérative via époque. | `engine.increment_epoch()` est une primitive Wasmtime. Sur seL4, il faut un mécanisme équivalent : timer kernel-mediated + trap WASM ou preemption à l'IPC. | Impl |
| **0027** | Durabilité log vs ContentStore | Distinction `append` (WAL OS-buffered) vs `append_durable` (WAL fsync). Notion de "durable au sens SIGKILL" vs "durable au sens power-loss". | `fsync` n'existe pas sur seL4 pur. La primitive de durabilité doit être définie sur le stack seL4 (serveur de bloc + cache de pages du serveur, pas du kernel). Décision à écrire. | Spec |
| **0030** | Scheduler unifié C1+C2 | `IoAdmissionQueue` (3 files, affinité cache), `InferencePool` (sémaphore borné), pipeline C2→C1, dormancy. | Implémentation sur Tokio channels (`mpsc`, `oneshot`). Sur seL4, les channels sont des IPC seL4 (endpoints). L'algorithme est identique, les primitives de synchronisation changent. | Impl |
| **0031** | Scheduler coordinator | Lazy wakeup, `EvictedState`, `deliver` vs `evict`, `DeliverError`. | Idem ADR-0030. | Impl |

### Catégorie C — Méthodologie transférable (savoir-faire, pas code)

| ADR | Connaissance acquise | Application sur seL4 |
|-----|----------------------|----------------------|
| **0026** | Protocole benchmark P3a : drop_caches, K≥3 runs, régime cache froid/chaud documenté, fio QD=1/32, manifests JSON. | Même protocole applicable à tout store seL4, en remplaçant RocksDB par le store natif. Les manifests et le format de verdict sont portables. |
| **0032** | Réfutation thermique : Spearman, OLS, falsification causale. Pièges identifiés (page cache OS masque signal thermique). | Sur seL4, les pièges seront différents (IPC budget, cache de pages du serveur) mais la méthodologie de falsification est identique. |
| **0033/0034** | Critère fuite mémoire LSM : `rss_adj = RSS − memtable`. OLS inutilisable sur workload write-intensif. | Sur seL4, RSS n'existe pas. La métrique équivalente sera différente (pages mappées, budget capability). Mais le piège "workload LSM fausse OLS" ne se reproduira pas — il est documenté. |

### Catégorie D — Non portable

| ADR | Pourquoi non portable |
|-----|-----------------------|
| **0011** | Options RocksDB layer 0 (bloom filter, block cache, prefix extractor). RocksDB n'existe pas sur seL4 pur. |
| **0035** | Config RocksDB explicite (write_buffer_size, target_file_size_base, bytes_per_sync, compression par niveau). Idem. |

---

## 2. Synthèse chiffrée

| Catégorie | Nombre d'ADRs | Proportion |
|-----------|---------------|------------|
| A — Transfère intégralement | 18 | 50 % |
| B — Concept transfère, impl à refaire | 12 | 33 % |
| C — Méthodologie transférable | 4 | 11 % |
| D — Non portable | 2 | 6 % |
| **Total** | **36** | 100 % |

**Conclusion :** 94 % des ADRs ont une valeur de transfert (A + B + C). Les 6 % non portables (0011, 0035) représentent du travail de tuning RocksDB qui est précisément ce qu'il faut arrêter d'accumuler.

---

## 3. Questions ouvertes bloquantes pour l'ADR-seL4 — RÉSOLUES (2026-05-30)

Les trois questions étaient bloquantes en 2026-05-27. Elles ont toutes été tranchées par ADR et exercées sur substrat vivant. Conservées ici pour traçabilité.

### Q-seL4-1 — Stack concrète du runtime acteur — ✅ RÉSOLU (ADR-0037)

**Tranché : option (a)** — Wasmtime `min-platform` no_std + executor Rust maison, sans Genode. Variante précise réalisée : Wasmtime 25 `features=["runtime"]` **sans Cranelift** (pas de JIT à l'exécution ; compilation AOT en `build.rs` côté host → `.cwasm` désérialisé sur cible, cf. ADR-0048 §F1). Validé C.3 (`add(21,21)=42` sur Wasmtime no_std seL4 AArch64). Option (b) Genode rejetée (ADR-0040 §Chemin B). Option (c) WAMR non retenue.

### Q-seL4-2 — Store natif : interface et primitives de durabilité — ⚠️ RÉSOLU PARTIELLEMENT (ADR-0038, 0042, 0045, 0046) + DETTE OUVERTE

- **Interface** : tranchée — ring buffer SPSC + `seL4_Call` (IPC kernel-mediated), commit content-addressed Q3-C (ADR-0038). Backend redb fork no_std sur virtio-blk (ADR-0042).
- **Durabilité** : tranchée — durabilité **niveau 1** (ack serveur RAM + write bufferisé virtio-blk), power-loss hors scope (ADR-0045 Q2=α, ADR-0027). `fsync` réel renvoyé à un déclencheur matériel (ADR-0049 §D3(b)).
- **Atomicité (P6)** : la prémisse « sans `WriteBatch` » de la question d'origine est **CADUQUE** (ADR-0049 §D4). Le code l'a portée **par la transaction redb englobante** — qui *est* un WriteBatch sémantique : `commit_to_redb` ouvre 4 tables (blobs, headers, journal, seq) dans un seul `begin_write()`/`commit()`. P6 tient par cette sur-garantie ACID. **Conséquence : la séparation « journal append-only autoritaire / index reconstructible » d'ADR-0038 §3 n'est PAS instanciée** (L82, ADR-0049 §D2). La question reste donc *partiellement ouverte* : la structure append-only pure reste la cible, déclenchée par l'implémentation du GC des orphelins (ADR-0049 §D3(a)).

### Q-seL4-3 — Budget par acteur sur seL4 — ✅ RÉSOLU (ADR-0037, 0044)

**Tranché** : isolation au **niveau processus seL4** — 1 VSpace = 1 runtime (ADR-0037), N agents validés à N fixe (C.7 : 2 runtimes + 1 serveur ≈ 1718 pages ELF < 4096 slots CNode). La variante « N agents WASM dans 1 VSpace partagé » (isolation S1b software) est différée (ADR-0044 D1) — déclencheur : besoin concret multi-agent par VSpace, qui rouvre aussi S4/fuel-équité (ADR-0048 §D6). P1 (densité) n'a pas été re-mesurée sur seL4 — transitivité Linux documentée (ADR-0045 matrice).

---

## 4. Prochaines étapes recommandées (2026-05-27) — RÉALISÉES

Les trois ADR recommandés ont été écrits **et** exercés sur code :

1. ✅ **ADR-seL4-substrat** → ADR-0037 (Q-seL4-1). Exercé C.3.
2. ✅ **ADR-seL4-store** → ADR-0038 + 0042 + 0045 + 0046 (Q-seL4-2, partiel). Exercé C.4→C.9.
3. ✅ **ADR-seL4-densité** → ADR-0037 + 0044 (Q-seL4-3). Exercé C.6→C.7.

Le PoC seL4 est **clos** (ADR-0049). La suite est la consolidation spec (cette section) — pas de nouveau code seL4 sans réveil d'un déclencheur dormant (ADR-0049 §D3).

---

## 5. Réalisation effective du PoC seL4 (clôture 2026-05-30)

Le transfert prospectif des §1–§4 a été **exécuté**. Cette section capitalise ce qui a réellement été réalisé sur seL4 AArch64 (QEMU virt), pour qu'un futur lecteur n'extrapole pas le plan en acquis.

### 5.1 Jalons C.1 → C.11-prov (ce qui a été démontré)

| Jalon | Démontré | ADR | Verdict |
|-------|----------|-----|---------|
| C.1–C.3 | Toolchain seL4 + root task custom + Wasmtime no_std AOT (`add(21,21)=42`) | 0039, 0037 | PASS |
| C.4 | Driver virtio-blk (DMA + MMIO scan, round-trip bloc 0) | 0041 | PASS |
| C.5 | redb fork no_std sur virtio-blk (N=1000 + intégrité) | 0042 | PASS (capacité de brique, **pas** P6, cf. L68) |
| C.6 / C.6-crash | Intégration 2-processus (runtime/serveur), P6 mono-agent (KP1–4) | 0043 | PASS |
| C.7 / C.7-crash | N agents (badge-dispatch), P6-N + I4 (non-interférence intégrité Biba) | 0044 | PASS |
| C.8 | Chaîne de commit persistante end-to-end runtime→ring→serveur→redb→virtio-blk | 0045 | PASS |
| C.9 | Persistance reopen (D-reopen : write→kill→reopen→read, K=100) | 0046 | PASS |
| C.10 / C.10-crash | W^X du pool JIT Wasmtime + atomicité crash dans la fenêtre de remap | 0047 | PASS |
| C.11 | WASM non confié au **contenu** adversarial (P-α OOB / P-β boucle / P-γ store survit) | 0048 | PASS |
| C.11-prov | WASM non confié par **provenance** (`.cwasm` canal non-trusted, P-δ rejet malformé) | 0048 §D1 | PASS |

**Propriétés validées sur seL4** : P6 (atomicité crash-processus) mono-agent et N, I4 (non-interférence intégrité), W^X intra-VSpace, isolation de processus sous WASM hostile. **Propriétés en transitivité Linux documentée** (non re-mesurées seL4) : P1 (densité), P2 (rollback — smoke seulement post-C.8), P3b, P4, P5. **P3a** : fonctionnelle sur seL4, latence recevable seulement sur Linux/NVMe (QEMU non recevable, ADR-0045 amendt Q1).

### 5.2 Ce qui n'est PAS instancié (garde-fou D2, à ne jamais sur-vendre)

- **Séparation CAS autoritaire / index reconstructible** (ADR-0038 §3, ADR-0042) : store réel = redb transactionnel monolithique (4 tables, 1 transaction). Cible non instanciée (L82, ADR-0049 §D2).
- **Récupération de trap intra-runtime** : trap WASM = panic = mort du VSpace runtime confinée par seL4 (S4, ADR-0048 §D3). L'isolation démontrée est S1-processus seL4, **pas** la résilience intra-runtime.
- **Borne de terminaison temporelle** : P-β validé par watchdog en *tours d'ordonnancement* (`tcb_suspend`), pas par un timer temps mur (ADR-0048 §D4).
- **Power-loss / durabilité média** : hors scope, régime crash-processus α uniquement (ADR-0045 Q2=α).
- **GC des orphelins redb** : différé (ADR-0038 §Q3) — son implémentation forcera la re-séparation CAS/index. L'algorithme mark-and-sweep et la métrique de déclenchement (Δ = blocks−headers + OLS 10 min) sont formalisés dans ADR-0055. La réserve empirique (estimate-num-keys sur store frais) est levée — voir `poc/scenarios/orphan-metric/VERDICT.md`. Le GC reste sur HOLD jusqu'à ce que la croissance non bornée soit observée sur cycles reopen (ADR-0049 §D3a).

### 5.3 Catégorie C étendue — méthodologie seL4 capitalisée (L68–L86)

Savoir-faire transférable à tout futur travail seL4 (pas du code, des pièges identifiés) :

| Leçon | Règle transférable |
|-------|--------------------|
| **L68** | Un jalon de faisabilité (C.5 store-direct) n'instancie pas la topologie d'architecture — vérifier l'invariant, pas le happy path. |
| **L69, L75** | rust-sel4 : vérifier les signatures contre la config `KERNEL_MCS` de l'image ; `absolute_cptr_from_bits_with_depth` exige `u64`. |
| **L70, L73** | `seL4_Call` exige le droit **GrantReply** (pas seulement Write) ; le badge est sur la cap (mint impératif), pas dans le message. |
| **L71, L85** | Wasmtime réserve 8 GB de VA pour toute `(memory ...)` WASM → module sans mémoire linéaire pour un pool JIT fini. |
| **L72** | `match option_env!("VAR") { Some("1") => }` non compilable nightly-2026-03-18 → parser par octets. |
| **L74, L77, L78** | N rings : N caps copiées par frame ; allouer DMA en premier (paddr=ut_paddr) ; retry-loop PT niveau 3 vs `map_intermediate_translation_tables`. |
| **L76** | I4 garantie par sérialisation `seL4_Call` + serveur single-thread, **pas** par preuve formelle. |
| **L79, L80, L81** | QEMU virtio-blk = fonctionnel seulement (jamais latence) ; commit média ≠ persistance (exercer le reopen) ; `StorageBackend::len()=0` → redb crée une DB fraîche, pas un reopen. |
| **L82** | Une transaction ACID unique derrière une interface « journal + index reconstructible » n'instancie pas la séparation. |
| **L83, L84** | Durcissement mémoire suit la détention de caps (VSpace câblé par parent = immuable de l'intérieur) ; `VmAttributes::default()` ≠ EXECUTE_NEVER ; fault_ep résolu dans le CSpace du thread en faute. |
| **L86** | `.cwasm` depuis canal non-trusted : `Module::deserialize` valide le format avant exécution (Err récupérable), isolation de canal portée par la structure de caps. |

### 5.4 Direction post-clôture

Re-instruire **Q-seL4-2** (atomicité — prémisse « sans WriteBatch » caduque) reste à faire si/quand le GC déclenche la re-séparation. Aucun autre travail seL4 n'est instruit sans réveil de déclencheur (ADR-0049 §D3). La valeur restante du projet est au-dessus du substrat (propriétés, mise à l'épreuve adversariale, spec).

**ADR-0056 (2026-06-02) — Pulley différé.** Le choix Cranelift AOT côté hôte + `Module::deserialize` côté cible (Q-seL4-1) est maintenu. Pulley (interpréteur bytecode portable) est différé avec trois conditions de réveil explicites : R1 (second substrat rendant la portabilité d'artefact décisive), R2 (PKI/multi-producteur), R3 (JIT réintroduit sur la cible). Le motif historique de rejet (instabilité, ADR-0037 §132) est caduc — Pulley est stable depuis Wasmtime 25+ ; le différé repose sur l'absence de déclencheur.

---

## Références

- `decisions/0049-cloture-poc-sel4.md` — clôture PoC seL4, garde-fous D2/D4 (source de la consolidation 2026-05-30)
- `decisions/0037` à `0048` — chaîne ADR seL4 (substrat, store, intégration, durcissement)
- `lab/LESSONS.md` L68–L86 — méthodologie seL4 capitalisée (§5.3)
- `spec/02-properties.md` — P1–P6 (invariants portables)
- `spec/02b-substrate_requirements.md §4` — tableau S1–S7, ligne seL4 "Candidat"
- `spec/03-state-of-the-art.md §2.1` — seL4 capabilities, IPC ~0.4µs ARMv7
- `decisions/0002-choix-substrat.md` — template pour ADR-seL4-substrat
- `decisions/0027-durabilite-log-vs-contentstore.md` — template pour Q-seL4-2
- [Klein 2009] "seL4: Formal Verification of an OS Kernel", SOSP 2009
- [Heiser 2020] "seL4 is free, what does that mean for you?" — chiffres IPC
