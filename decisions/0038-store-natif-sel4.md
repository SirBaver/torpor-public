# ADR-0038 — Store natif seL4 (Q-seL4-2)

**Date :** 2026-05-27  
**Statut :** Acceptée — Q1/Q2/Q3/Q4 tranchées. Phase 8 (QEMU RAM disk) résolue. Phase 9 (driver block + moteur index persistant) ouverte.

---

## Contexte

`spec/09 §3 Q-seL4-2` identifie la question du store natif seL4 comme bloquante pour P2, P3 et P6 sur la stack cible. ADR-0037 a choisi Wasmtime `min-platform` + executor Rust minimal dans un VSpace seL4 unique. Le PoC Linux reposait sur RocksDB (ContentStore + CausalLog, ADR-0011/ADR-0035), classifié **D** (non portable) dans spec/09.

Sur seL4, il n'existe pas de filesystem natif, pas de `fsync`, pas de libc. Le store doit être reconstruit à partir des primitives seL4 : IPC, capabilities, Frame objects.

---

## 1. Décision

### Q1 — Interface : ring buffer en mémoire partagée + IPC de commit unique

Le runtime obtient une **Frame Capability seL4** vers une région mémoire partagée avec le serveur de stockage. Pour chaque action committed :

1. Le runtime écrit `block_data`, `snapshot_header`, et `log_entry` dans le ring buffer par accès mémoire direct (pas d'IPC pour les données elles-mêmes).
2. Un unique `seL4_Call` transmet un MessageInfo `"commit seq=k, offsets=[B₁..Bₙ, H, L]"`. Le serveur copie les records dans son journal interne et acquitte.

**Rejet des alternatives :**

| Option | Rejet |
|--------|-------|
| File-like / VFS minimal | Couche d'abstraction inutile. Ouvre les confused deputy classiques des FS POSIX. Pas de libc → pas de VFS natif. |
| 3 IPC séparés (1 par type de données) | 3× la latence IPC (3–9 µs IPC seL4 x86_64) sans bénéfice. Le batch natif seL4 est 1 IPC + mémoire partagée. |

**1 ring par agent** (SPSC) pour la Phase 8 : overhead = 4 KB/agent (taille Frame minimale seL4). À 70–100 agents actifs simultanés (spec/07 §3.3 P1b), le coût est 400 KB — négligeable.

### Q2 — Durabilité : niveau (1) — acquittement serveur RAM

`seL4_Call` retourne quand le serveur de stockage a recopié les données dans son **journal interne en RAM** (niveau 1 sur 4 niveaux : agent→serveur RAM→driver→cache device→média). Pas de flush vers le device sur le chemin chaud.

Cible explicite : **"crash du processus runtime"**, analogue strict d'ADR-0027 D1 (SIGKILL). Le serveur de stockage tourne dans un processus seL4 distinct → il survit au crash du runtime → les écritures déjà acquittées (niveau 1) sont préservées.

**Régimes explicitement non couverts en Phase 8 :**
- **Crash du serveur de stockage** : le serveur est in-TCB (§2). On ne couvre pas ce cas.
- **Power-loss** : identique Phase 6 Linux (ADR-0027 §D3). Sera adressé Phase 9+ via `commit_durable()` (seL4_Call + flush device acquitté par le driver).

### Q3 — Atomicité de {block_data, snapshot_header, log_entry} : Option C (content-addressed + log atomique)

**Invariant d'écriture**, pour une action committed à `seq=k` :

```
1. Écrire tous les blobs nouveaux B₁..Bₙ (content-addressed, clé = SHA-256).
   → Pas d'atomicité requise. Un blob non-référencé est invisible.
2. Écrire SnapshotHeader[k] pointant {B₁..Bₙ} par hash.
   → Pas d'atomicité requise. Un header non-référencé depuis le log est orphelin.
3. Écrire log_entry[k] pointant SnapshotHeader[k] par hash.
   → CETTE écriture est l'unique opération atomique. C'est un append-only record.
```

**Garanties de P6 (atomicité crash) :**

| Crash au moment | État observable | P6 |
|-----------------|-----------------|-----|
| Pendant étapes 1 ou 2 | log_entry[k] absent → état = seq=k-1 | OK |
| Pendant étape 3 (IPC pas retourné) | log_entry[k] absent du journal serveur → état = seq=k-1 | OK |
| Après étape 3 (IPC retourné) | log_entry[k] présent + tous pointés présents (écrits avant) | OK |

**Condition de validité :** l'ordre temporel (1) ≤ (2) < (3) est préservé côté serveur via ring SPSC par agent (FIFO trivial).

**GC des orphelins :** repoussé à ADR-0040. Phase 8 accumule sans GC — acceptable PoC QEMU.

### Q4 — Store partagé entre agents, namespacing logique par agent_id

1 serveur de stockage seL4 pour N agents. Chaque write record encode `agent_id` (16 bytes). L'isolation inter-agents est garantie par Wasmtime sandbox (ADR-0037 S1b). Overhead = O(1) indépendant de N. Compatible P1.

---

## 2. Question bloquante résolue : statut du serveur de stockage (B1)

**Décision B1 : le serveur de stockage est in-TCB.**

Le serveur de stockage fait partie du TCB OS au même titre que seL4 et le runtime Rust. Il est supposé ne pas crasher plus que le kernel. Cette décision est cohérente avec ADR-0037 : on accepte déjà que la sandbox Wasmtime est constituante du TCB.

**Conséquence spec/08 §0.2 :** le TCB OS seL4 comprend : seL4 (kernel vérifié formellement) + runtime Rust (Wasmtime + executor async) + serveur de stockage (Rust, ~2–5 KLOC estimés). Voir reformulation spec/08 §0.2 (règle C dans le TCB, 2026-05-27).

---

## 3. Interface logique `StoreServer`

Pour permettre la substitution RAM Phase 8 → block Phase 9 **sans refactor du runtime**, le serveur de stockage expose une interface logique abstraite :

```rust
// Appel via seL4_Call (ring buffer → MessageInfo)
enum StoreRequest {
    Commit { records: &[Record] },  // blobs + header + log_entry, dans l'ordre Q3
    Get { action_id: ActionId },    // lookup P3a
}

enum StoreReply {
    Committed { seq: u64 },
    Entry(Option<LogEntry>),
    Err(StoreError),
}
```

**Phase 8** : implémentation RAM (`Vec<Record>` comme journal + `HashMap<ActionId, LogEntry>` comme index).  
**Phase 9** : même interface, backend block (B2) + moteur d'index persistant (B3).

**Invariant de l'index :** l'index `action_id → log_entry` est un *cache entièrement reconstructible* depuis le journal append-only. Il n'est jamais autoritaire. Rebuild au démarrage en O(N) : pour 10⁶ entrées × ~200 bytes = 200 MB scan à 2 GB/s → 100 ms. Acceptable.

---

## 4. Phase 8 — Décisions actées (QEMU RAM disk)

| Question | Décision Phase 8 | Coût Phase 8 |
|----------|-----------------|--------------|
| **B2** — Driver block I/O | **RAM disk pur** — journal + index en RAM, pas de driver block réel. Pas de persistance après restart serveur (serveur in-TCB = restart rare). | Éliminé du scope Phase 8 |
| **B3** — Moteur d'indexation P3a | **HashMap full-RAM** — `HashMap<ActionId, LogEntry>`. Cible : 10⁶ entrées (200 MB RAM). | Trivial |
| **Cible P3a** | 10⁶ entrées Phase 8 ; 10⁸ entrées Phase 9+. | — |
| **Reconstructibilité index** | L'index est reconstruit par scan du journal si besoin. Journal = source de vérité unique. | — |

**Conséquence :** Phase 8 peut valider Q1/Q3/P6 sur QEMU sans driver block ni moteur d'index persistant. ADR-0037 §7 étape "PoC seL4 sur QEMU" est débloqué.

---

## 5. Conséquences sur les propriétés

| Propriété | Impact |
|-----------|--------|
| **P2 (rollback ≤ 100 ms)** | Inchangée. Rollback = traversée SnapshotHeader (P3a interne) + restauration racine. Aucun write durable requis sur le chemin de rollback (ADR-0027 D4). |
| **P3a (lookup ≤ 10 ms p99)** | Phase 8 : HashMap en RAM → p99 < 1 µs pour 10⁶ entrées. Phase 9 : nécessite moteur persistant qualifié (B3). |
| **P3b (end-to-end ≤ 20 ms)** | 1 IPC + temps journal serveur (~1–3 µs IPC + ~1 µs copie RAM) ≪ 20 ms. Avec `commit_durable()` Phase 9 : 0.5–3 ms total. |
| **P6 (atomicité crash SIGKILL)** | Tenue par Q3-C + Q2-niveau-(1). Régime de menace : crash processus runtime, serveur survit. Analogue strict ADR-0027 D1/D3. |
| **P6 power-loss** | Non couvert Phase 8 (identique Phase 6). |

---

## 6. Questions ouvertes — Phase 9

### B2 — Driver block I/O

Trois voies, non encore tranchées. Dépend de la résolution de **spec/08 §0.2** (politique C dans le TCB) et d'une **revue de littérature seL4+Rust+block** à mener :

| Voie | Coût estimé (révisé) | TCB |
|------|----------------------|-----|
| **(i) Driver NVMe Rust from scratch** | 6–9 mois solo (driver NVMe + DMA seL4 + IRQHandler + buffering) | Rust pur — spec/08 §0.2 satisfait |
| **(ii) sel4-microkit + sDDF/blk (C)** | 6–10 semaines (intégration + journal+index Rust au-dessus de l'ABI sDDF) | C dans TCB — acceptable si spec/08 §0.2 option α/β |
| **(iii) Driver virtio-blk Rust seL4 existant** | 2–4 semaines si disponible (à vérifier : `seL4/rust-sel4`, `asterinas`) | Dépend de la source |

**Critère de tranchage B2 :**
1. Vérifier l'état de voie (iii) : `seL4/rust-sel4`, `asterinas/asterinas` NVMe, projets 2024–2026.
2. Si spec/08 §0.2 retient option α ou β : voie (ii) sDDF/blk viable.
3. Si spec/08 §0.2 retient option γ (zéro C) : voie (i) obligatoire, 6–9 mois.

### B3 — Moteur d'indexation `action_id → log_entry` (Phase 9, 10⁸ entrées)

**Dépendance :** vérifier si `redb` expose un trait `Storage` abstrait permettant un backend block custom sans `std::fs::File` (seL4 n'a pas de VFS). Si oui, voie redb viable. Si non, LSM custom requis.

| Candidat | Pros | Cons |
|----------|------|------|
| **`redb`** (Rust, B-tree, MIT/Apache) | Simple, Rust pur | Backend Storage abstrait ? À vérifier. Pas qualifié 10⁸. |
| **`sled`** (Rust, LSM-like, MIT) | Plus mature | "Alpha for years". Backend abstrait ? À vérifier. |
| **LSM custom Rust** | Contrôle total | 4–6 mois réalistes, risque bugs élevé. |

**Tâche d'investigation :** consulter l'agent `rust` sur l'API de `redb::Database::create_with_backend()` ou équivalent avant de choisir.

---

## 7. Architecture cible (schéma)

```
┌─────────────────────────────────────────────────────────────┐
│  Processus runtime (1 VSpace seL4 — ADR-0037)               │
│                                                              │
│  Agent₁ [WASM]  Agent₂ [WASM]  ...  AgentN [WASM]          │
│      │               │                    │                  │
│      └───────────────┴────────────────────┘                  │
│                    ring buffer SPSC / agent                  │
│                    (Frame Capability partagée)               │
│                           │                                  │
│              seL4_Call("commit seq=k")                       │
└──────────────────────────────────────────────────────────────┘
                            │
                     IPC endpoint seL4
                            │
┌───────────────────────────▼──────────────────────────────────┐
│  Serveur de stockage (processus seL4 séparé — in-TCB)        │
│                                                              │
│  Phase 8 : journal Vec<Record> + HashMap<ActionId, Entry>    │
│  Phase 9 : journal append-only + moteur d'index persistant   │
│  Interface : StoreRequest::Commit / Get (§3)                 │
└──────────────────────────────────────────────────────────────┘
```

---

## 8. Conséquences négatives

- **3 étapes d'écriture séquentielles** (blobs → header → log_entry) là où RocksDB faisait 1 `WriteBatch`. Coût : 3 ring-pushes + 1 IPC = ~2–5 µs. Acceptable pour P3b.
- **GC des orphelins non implémenté Phase 8** : les blobs non-référencés s'accumulent. RAM disk QEMU = espace borné → à limiter au PoC court.
- **10–18 semaines de bare-metal seL4** (runtime ADR-0037 + store ADR-0038) avant le premier agent sur QEMU. Aucun précédent public. Obstacles inconnus probables.
- **B2/B3 Phase 9 ouverts** : un mauvais choix sur le driver (voie C dans TCB) demanderait de rouvrir spec/08 §0.2. Résoudre B2.1 (politique C) en priorité.

---

## 9. Prochaines étapes

1. **PoC seL4 QEMU** (ADR-0037 §7, Phase 8) : bootstrapper seL4 + sel4-hello-world + runtime Wasmtime + serveur stockage RAM. Valide Q1/Q3/P6 sans driver block.
2. **spec/08 §0.2** — Reformuler la politique C dans le TCB (α/β/γ). Débloque le tranchage B2 Phase 9.
3. **B2 Phase 9** — Revue `seL4/rust-sel4`, `asterinas/asterinas`, et statut sDDF/blk. Décision voie driver block.
4. **B3 Phase 9** — Investigation `redb::Storage` trait (agent `rust`). Benchmark P3a 10⁸ sur Linux. Décision moteur index.

---

## Références

- `decisions/0037-stack-runtime-sel4.md` — architecture runtime, VSpace, TCB
- `decisions/0027-durabilite-log-vs-contentstore.md` — durabilité Linux, régimes SIGKILL/power-loss
- `spec/09-transfert-poc-sel4.md` §3 Q-seL4-2
- `spec/02-properties.md` P2, P3, P6
- `spec/08-modele-menace.md` §0 (TCB), §0.2 (politique C — 2026-05-27)
- [Gray & Reuter 1992] *Transaction Processing*, ch. 9–10 (no-force/steal)
- [Xu & Swanson 2016, FAST] "NOVA: A Log-structured File System for Hybrid Volatile/Non-volatile Main Memories"
- [Dolstra 2006] *The Purely Functional Software Deployment Model* — content-addressing + atomicité par racine
- [O'Connor et al. 2016, ICFP] "Refinement through restraint: bringing down the cost of verification" (BilbyFs, Cogent)
- [Heiser et al. 2024] sDDF design notes — block server seL4 de référence (voie B2-ii)
- seL4 Reference Manual v15.0.0 §5 (IPC), §6 (MessageInfo), §10 (Frame objects)

---

## Amendements

### Amendement 2026-05-29 — Q3 : l'atomicité Q3-C n'est pas instanciée dans le PoC seL4 (délégation à redb)

**Contexte :** Revue de soundness C.6→C.9. L'invariant §3 (« l'index action_id→log_entry est un cache entièrement reconstructible, jamais autoritaire ») et le modèle Q3-C §44-65 (« seul l'append du log_entry est l'opération atomique, sur un journal append-only content-addressed séparé de l'index ») décrivent une séparation stockage qui n'est PAS celle du code.

**Réalité du code** (`poc/sel4-hello/{c8-store,c9-reopen}/server/src/main.rs`, fn `commit_to_redb`) : le serveur porte QUATRE tables dans UNE seule base redb — `TABLE_BLOBS` et `TABLE_HEADERS` (content-addressed, clé SHA-256) MAIS AUSSI `TABLE_JOURNAL_A` (seq→header_hash) et `TABLE_SEQ` (agent→seq). Le commit ouvre les quatre tables dans un unique `begin_write()` et fait un `wtx.commit()` unique.

**Conséquences :**

1. **L'atomicité réellement fournie est celle de la transaction ACID redb englobante**, pas « l'append atomique du log_entry sur un store content-addressed séparé » de Q3-C. C'est plus FORT que Q3-C (la fenêtre {blobs → header → log_entry} est rendue atomique en bloc, pas seulement le dernier append) — P6 (I1/I2/I3) tient et tient même par sur-garantie. Aucune régression de propriété.

2. **`TABLE_JOURNAL_A` encode l'ordre temporel** (seq → header_hash). Il n'est donc PAS reconstructible depuis les seuls blobs/headers content-addressed. L'index d'ordre est de l'**état autoritaire vivant dans redb**, contrairement à l'invariant §3 qui le veut reconstructible et non-autoritaire.

**Distinction essentielle avec C.5 (L68).** L'inversion *topologique* de C.5 (mono-root-task, redb store-direct accédé par le runtime) EST corrigée par C.6 : runtime et serveur sont deux processus seL4 distincts, le runtime ne touche jamais redb, tout passe par ring + `seL4_Call`. Ce qui reste non instancié, c'est la séparation *de stockage interne au serveur* : journal append-only content-addressed autoritaire d'un côté, index reconstructible de l'autre. La valeur architecturale acquise (isolation de processus, survie du serveur au crash du runtime — P6) est réelle ; ce qui n'est pas démontré, c'est la propriété « journal CAS autoritaire + index jetable ».

**Périmètre de l'écart vs ADR-0027.** L'écart porte sur la TOPOLOGIE de l'atomicité (transaction unique vs append séparé), PAS sur le régime de durabilité. `sync_data()` est no-op → durabilité niveau 1 (§Q2, α) → le modèle no-force d'ADR-0027 reste respecté. Ne pas reformuler cet écart comme un retour au « force-at-commit » : ce serait inexact.

**Décision :** pour le PoC seL4, l'atomicité déléguée à la transaction redb unique est ACCEPTÉE. Elle satisfait P6 par sur-garantie et corrige l'inversion topologique de C.5. La séparation Q3-C (journal autoritaire / index reconstructible) reste une **cible non réalisée**, à instancier si et quand : (a) le GC des orphelins est implémenté (un GC suppose des blobs/headers orphelins distincts de l'index — incohérent avec un index dans la même transaction) ; ou (b) un backend où l'index n'est pas transactionnellement couplé aux blobs est introduit. Cette dette est tracée (TODO.md §Phase 9, LESSONS.md L82).

**Ce que les PASS attestent réellement :** C6/C6-crash/C7/C7-crash/C8/C9 attestent (i) la séparation 2-processus, (ii) P6 en régime crash-processus, (iii) le chemin write+reopen. Ils n'attestent PAS l'invariant §3 « index reconstructible non-autoritaire » ni la granularité d'atomicité Q3-C (single log_entry append). Le store du PoC est un store transactionnel redb monolithique encapsulé derrière l'interface §3 StoreServer.

**Référence :** `poc/sel4-hello/c9-reopen/server/src/main.rs` fn `commit_to_redb` ; ADR-0043 §26-28 ; LESSONS.md L68 (faisabilité ≠ architecture) et L82 (cet écart) ; ADR-0027 (no-force, NON concerné côté durabilité).
