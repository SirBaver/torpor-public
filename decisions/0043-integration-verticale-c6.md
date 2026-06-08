# ADR-0043 — Intégration verticale C.6 : topologie 2-processus + validation P6

**Date :** 2026-05-29  
**Statut :** Acceptée

## Contexte

Le jalon C.5 (`C5_PASS`) a validé une capacité de brique : redb no_std fonctionne sur virtio-blk en environnement seL4 no_std. Mais C.5 a câblé redb DIRECTEMENT sur le block device comme store durable transactionnel (mono-root-task, pas de journal content-addressed, pas de séparation de processus). Cela inverse l'invariant ADR-0038 §3 (« l'index est un cache reconstructible, jamais autoritaire ; le journal append-only content-addressed est la source de vérité ») et adopte le modèle d'atomicité transactionnelle qu'ADR-0027 a explicitement rejeté (no-force/recovery, pas force-at-commit). Voir ADR-0042 §Amendement (2026-05-29) et LESSONS.md L68.

C.6 corrige cette topologie et réalise la première intégration verticale : un agent WASM émet une action → host function → store → driver, dans la topologie conforme ADR-0038 (serveur de stockage = processus seL4 SÉPARÉ du runtime, communication par ring buffer en mémoire partagée + 1 seL4_Call de commit).

Les jalons précédents (C.2 retype, C.3 Wasmtime deserialize, C.4 virtio-blk, C.5 redb) tournaient tous dans UN SEUL root task mono-processus. C.6 introduit pour la première fois la séparation 2-processus.

## Décision

**C.6 réalise l'intégration verticale mono-agent avec séparation 2-processus (runtime / serveur de stockage), et valide P6 (atomicité crash) par injection de crash aux frontières Q3-C. C.6 est découpé en DEUX jalons : C.6 (intégration nominale) puis C.6-crash (validation P6). Le critère de sortie de la phase = C.6-crash PASS.**

## Topologie 2-processus

- **Root task = SUPERVISEUR.** Elle spawn deux fils : le runtime (VSpace A) et le serveur de stockage (VSpace B). Elle ne s'auto-suspend pas avant la fin du scénario. Elle détient les caps TCB des deux fils.

- **Périmètre C.6** : 1 agent, 1 ring SPSC, 1 endpoint, mono-agent synchrone, SANS executor async (l'executor est repoussé au jalon C.7).

- **Communication** : ring buffer en Frame partagée mappée dans les deux VSpaces (1 frame physique, 2 caps copiées, 2 frame_map, attributs VmAttributes::default() = Normal WriteBack Inner-Shareable, compteurs head/tail atomiques alignés). Commit = 1 seL4_Call synchrone bloquant sur Endpoint partagé.

- **Atomicité Q3-C** (rappel ADR-0038 §44-65) : écrire blobs content-addressed (clé SHA-256) → SnapshotHeader → log_entry ; seul l'append du log_entry est l'opération atomique de commit.

- **L'index redb reste un cache reconstructible non-autoritaire** (ADR-0038 §3), instancié EN AVAL du journal content-addressed, jamais comme store direct (correction de C.5).

## Mécanisme de crash (validation P6)

**Verdict :** `tcb_suspend()` sur le propre TCB du runtime, à des kill_points instrumentés, est le mécanisme PRIMAIRE et SUFFISANT (gate). 

**Justification :** P6 affirme une propriété DISCRÈTE — la fenêtre {blobs → header → log_entry} n'a qu'un seul point de commit atomique. Les kill_points ne sont pas un échantillon d'un continuum, ils sont l'énumération exhaustive des frontières de transition que Q3-C définit. Un crash aléatoire « entre » deux frontières retombe dans un état déjà couvert par la frontière précédente. C'est le découpage S6 transposé (ADR-0027 §207, « 4 kill_points × 2 »).

**Les 4 kill_points obligatoires :**

1. **KP1 :** après push des blobs, avant push du SnapshotHeader.
2. **KP2 :** après push du header, avant push du log_entry.
3. **KP3 :** après écriture du log_entry côté ring mais AVANT retour du seL4_Call (fenêtre « IPC pas retournée », ADR-0038 §62).
4. **KP4 :** après retour du seL4_Call (ADR-0038 §63).

**Contrainte d'implémentation :** `tcb_suspend()` est un GEL, pas une mort. Pour que le test soit honnête, le superviseur ne doit JAMAIS reprendre le runtime suspendu ; idéalement il révoque/delete la cap TCB du runtime après suspension pour rendre l'irréversibilité explicite.

**Mécanisme secondaire OPTIONNEL non-gate :** faute mémoire routée vers un fault endpoint (crash non instrumenté, point non maîtrisé) — confirme que le serveur survit à une mort non-coopérative. À ajouter si le budget le permet, sinon repoussé. Le critère de sortie C.6 NE dépend PAS de ce mécanisme.

**Mécanismes rejetés :** 
- (b) superviseur qui suspend sur observation du ring (introduit une course et un 3e acteur à prouver, sans bénéfice sur le mécanisme primaire) ;
- (d) revoke d'une cap critique (teste la réaction du serveur à la perte d'un lien, orthogonal à P6).

## Oracle de recovery

L'oracle vit **DANS le serveur de stockage** (qui survit au crash du runtime), interrogé après suspension du runtime via une invocation de lecture sur l'endpoint existant. Pas de 3e composant.

**Nature du test** (différence avec Linux) : sur Linux, S6 testait la survie du log sur disque après mort du process puis rejouait via os-poc-reconstruct. Sur seL4, le serveur n'est jamais mort, son journal RAM est l'état autoritaire vivant ; l'oracle n'efface/reconstruit rien, il INTERROGE un composant survivant. Plus fort sur un axe (pas de dépendance disque pour ce régime), plus faible sur un autre (ne teste PAS la survie sur média persistant = power-loss D3, hors-scope). À acter explicitement.

**Invariant exact à asserter** (reproduire tel quel) : soit `seq` la valeur retournée par le serveur après suspension au kill_point KPᵢ, et `k` l'action en cours d'écriture. P6 tient ssi :

- **(I1) Atomicité du dernier log_entry :** le journal contient exactement `n` entrées, `n ∈ {k-1, k}`, aucune entrée partielle (un log_entry est présent en entier ou absent — granularité de l'append-only record, ADR-0038 §54).

- **(I2) Fermeture référentielle descendante :** pour le dernier log_entry présent (seq = n), le SnapshotHeader qu'il pointe par hash est présent, et tous les blobs Bᵢ que ce header pointe par hash sont présents. Formellement : `∀ entry ∈ journal, resolve(entry.header_hash) ≠ ⊥ ∧ ∀ h ∈ resolve(entry.header_hash).blob_hashes, resolve(h) ≠ ⊥`. (« jamais de header sans blobs, jamais de log_entry sans header » — transposition ADR-0027 §226.)

- **(I3) Cohérence état/kill_point :** KP1/KP2 ⇒ n = k-1 (action k non committée, ses blobs/header éventuels sont des orphelins invisibles, licite ADR-0038 §50-52) ; KP3 ⇒ n = k-1 (log_entry pas encore dans le journal serveur, IPC pas retournée) ; KP4 ⇒ n = k.

**Contrainte :** l'oracle ne doit PAS asserter « pas d'orphelin » — Q3-C autorise explicitement les orphelins (blob/header non référencé = invisible et inoffensif). Seule la direction descendante est requise. `resolve(hash)` doit être un lookup direct dans le store content-addressed AUTORITAIRE, jamais via l'index action_id→log_entry (cache non-autoritaire, ADR-0038 §106).

## Portée bornée de la validation P6 (clause obligatoire)

Validité bornée de P6-C.6. Le test crash de C.6 valide P6 **UNIQUEMENT** dans le régime {mono-agent, synchrone, 1 ring SPSC, crash runtime / serveur survivant}. Il ne valide ni le régime concurrent (N actions en vol, introduit en C.7), ni le régime power-loss intégral (serveur mort aussi, ADR-0027 D3). Toute évolution vers l'exécution concurrente (C.7) ou vers la durabilité sur média exige une re-validation P6 dédiée. C6_PASS et C6-crash_PASS n'attestent que du régime ci-dessus.

**Invariant de stabilité d'interface** (condition pour que C.6 reste capitalisable en C.7) : Le protocole de commit unitaire (format Record, unique seL4_Call atomique sur l'append du log_entry, ordre Q3-C garanti par ring SPSC) est figé en C.6 et ne doit pas changer en C.7. C.7 ajoute la composition (N rings → un serveur, ordonnancement inter-actions) au-dessus de ce protocole inchangé. Si C.7 modifie le protocole unitaire, la validation P6-C.6 est invalidée et doit être refaite.

**Note :** l'ordre Q3-C (1)≤(2)<(3) est garanti **PAR ring** (FIFO SPSC trivial, ADR-0038 §65), **PAS entre rings** — l'ordonnancement inter-rings est un mode de défaillance nouveau de C.7, non couvert par C.6.

## Découpage en deux jalons

- **C.6 (intégration nominale) :** `C6_PASS` = le câblage 2-processus fonctionne (runtime VSpace A, serveur VSpace B, ring partagé, endpoint) ; un agent WASM émet, le commit Q3-C atteint le serveur, l'état est lisible. NE valide PAS P6.

- **C.6-crash (validation P6) :** `C6-crash_PASS` = P6 tient aux 4 kill_points Q3-C dans le régime borné (section « Portée bornée de la validation P6 »).

- **Dépendance stricte :** on ne peut pas instrumenter de kill_points dans une séquence d'écriture qui ne fonctionne pas encore. C.6 est un prérequis dur de C.6-crash.

- **C6_PASS ne doit EN AUCUN CAS être présenté comme validant P6** — uniquement l'intégration.

## Faisabilité API seL4 (faits vérifiés rev rust-sel4 7a2321f2)

- **Pas de helper « spawn process »** dans rust-sel4 à ce rev : câblage manuel requis. L'exemple officiel `crates/examples/root-task/spawn-task/` (même rev) est le squelette de référence à copier.

- **Pipeline spawn du serveur** (2 appels, pas 4) : retype VSpace racine (ObjectBlueprint AArch64 = cap_type::VSpace) + asid_pool_assign obligatoire ; PTs intermédiaires via TranslationTableObjectType::from_level sur sel4::vspace_levels::NUM_LEVELS ; Frames image (1 Granule par page ELF) ; IPC buffer (1 Granule) ; CNode fils (allocate_variable_sized, size_bits petit ex. 2) ; TCB fils. Puis `child_tcb.tcb_configure(fault_ep, child_cnode, cnode_cap_data, child_vspace, ipc_buffer_addr, ipc_buffer_cap)` (remplace set_space + set_ipc_buffer), puis `child_tcb.tcb_write_all_registers(true /*resume*/, &mut ctx)` avec `*ctx.pc_mut() = entry` (remplace write_registers + resume).

- **Crash :** `cap::Tcb::tcb_suspend()` attesté (spawn-thread/main.rs:60, le call sur son propre TCB ne retourne jamais → unreachable!()). Cap du propre TCB = `sel4::init_thread::slot::TCB.cap()` (à confirmer nom exact slot). Irréversibilité : superviseur appelle revoke/delete sur la cap TCB du runtime (labels CNodeRevoke=17, CNodeDelete=18 confirmés).

- **Frame partagée :** 1 cap Frame = 1 mapping → pour 2 VSpaces il faut 2 caps copiées (copy/mint) de la même frame physique, puis `frame.frame_map(vspace, vaddr, CapRights::read_write(), VmAttributes::default())` dans chaque VSpace. Attributs default() = Normal WriteBack Inner-Shareable, cohérence cache matérielle assurée entre threads Cortex-A57 ; NE PAS utiliser NON_CACHEABLE (réservé MMIO/DMA). Compteurs head/tail en AtomicU32/U64 alignés avec Ordering explicite (barrières DMB).

- **IPC :** `endpoint.call(msg_info)` côté client (synchrone bloquant, syscall Call=-1 confirmé), `endpoint.recv()`/`reply_recv()` côté serveur, badge minté sur la cap endpoint. seL4_Call BLOQUE le runtime jusqu'au Reply → KP3 (bloqué dans le call) et KP4 (call retourné) sont des points nets et observables.

- **Risque n°1 = dimensionnement CNode/Untyped.** Le CNode racine de la root task = 4096 slots, déjà épuisé en C.3 à heap 16 MB. Avec en plus l'image ELF complète du serveur (1 slot par page de 4 KB, potentiellement plusieurs centaines), + PTs + VSpace + 2 CNodes + endpoint + ring : risque réel. Mesures : le CNode fils a son propre espace (n'encombre pas le superviseur) ; deleter les caps temporaires de construction une fois l'image mappée (SAUF les frames image mappées, dont le delete unmappe) ; prévoir d'itérer sur plusieurs Untyped non-device si un seul gros ne suffit pas ; compter taille_ELF_serveur / 4 KB avant de coder.

- **Note MCS — RÉSOLUE (2026-05-29) :** la config générée du build C.5 donne `KERNEL_MCS = false`. L'image est **non-MCS** : `tcb_configure` prend `fault_ep` en premier argument (variante non-MCS confirmée dans `invocations.rs:194`), et le `seL4_Call` a la sémantique reply bloquante standard (pas de scheduling-context). Pas de surprise MCS à anticiper.

- **Signatures confirmées (2026-05-29)** sur clone non-sparse rev 7a2321f2, crate `sel4` : `Endpoint::{recv, nb_recv, call, reply_recv}` (`syscalls.rs:71-106`), `slot::TCB.cap().tcb_suspend()` (`init_thread.rs:164`), `AbsoluteCPtr::{revoke, delete, copy, mint, move_}` (`invocations.rs:440-493`), `Tcb::{tcb_suspend, tcb_resume, tcb_write_all_registers, tcb_configure}` (`invocations.rs:106-215`). Tous les noms du brief sont exacts.

- **Effort estimé :** 5 à 8 jours (1-2j porter spawn-task + charger ELF serveur ; 1-2j frame ring partagée ; 1-2j endpoint commit + boucle recv/reply ; 1-2j kill_points + reprise superviseur + tests KP1-KP4).

## Conséquences

**Positives :**
- Corrige l'inversion de topologie de C.5.
- Première validation MESURÉE de P6 sur la cible seL4 (jusqu'ici P6 validée uniquement sur Linux via SEF-4).
- Sépare proprement intégration et atomicité (deux PASS distincts).
- Établit le protocole de commit unitaire réutilisable en C.7.

**Négatives :**
- Risque de dimensionnement CNode/Untyped avec 2 images.
- Câblage manuel du spawn (pas de helper).
- P6 validée seulement en régime mono-agent (re-validation obligatoire en C.7).
- Le régime power-loss reste non couvert (hors-scope, ADR-0027 D3).

## Options rejetées

- **Mono-processus en C.6** (repousser la séparation) : rejeté. En mono-processus, « le serveur survit au crash du runtime » est un énoncé vide (tuer le runtime tue le serveur) → P6 non testable, donc le critère de sortie C.6 serait inatteignable. Reproduirait l'inversion de C.5.

- **Crash via superviseur observant le ring** (mécanisme b) : rejeté (race + 3e acteur à prouver).

- **Revoke de cap critique** comme test P6 (mécanisme d) : rejeté (orthogonal à P6).

- **Oracle dans un 3e composant** : rejeté (ajoute VSpace/CNode/preuve pour zéro gain ; l'état autoritaire est déjà chez le serveur).

- **Jalon C.6 unique** (intégration + crash mélangés) : rejeté (ambiguïté du PASS, dépendance stricte non explicite ; le découpage fin a payé en C.2-C.5).

## Étapes suivantes

1. ~~Confirmer dans les sources non-sparse du rev 7a2321f2 : signatures Endpoint::call/recv/reply_recv, init_thread::slot::TCB, AbsoluteCPtr::{copy,revoke,delete}.~~ **Confirmé 2026-05-29** (clone non-sparse rev 7a2321f2, crate `sel4`) : `slot::TCB.cap().tcb_suspend()` exact (`init_thread.rs:135,164` — helper de suspension intégré) ; `Endpoint::{recv(reply_authority)->(MessageInfo,Badge), nb_recv, call(info)->MessageInfo, reply_recv(...)}` (`syscalls.rs:71-106`) ; `AbsoluteCPtr::{revoke(self), delete(self), copy(self,src,rights), mint(self,src,rights,badge), move_(self,src)}` (`invocations.rs:440-493`) ; `Tcb::{tcb_suspend, tcb_resume, tcb_write_all_registers(resume,regs), tcb_configure}` (`invocations.rs:106-215`). **Bifurcation MCS résolue :** `KERNEL_MCS = false` dans la config générée du build C.5 → image **non-MCS** → `tcb_configure(fault_ep, cspace_root, cspace_root_data, vspace_root, ipc_buffer, ipc_buffer_frame)` (variante AVEC `fault_ep`) ; sémantique `seL4_Call` bloquante standard, sans scheduling-context. Tous les « à vérifier » de la section Faisabilité sont levés.

2. ~~Compter la taille de l'ELF du serveur (redb + driver virtio-blk) en pages de 4 KB pour dimensionner CNode/Untyped avant de coder.~~ **Mesuré 2026-05-29** (proxy = binaire C.5, déjà redb + driver sans Wasmtime) : code + rodata + data incompressible = **121 pages (~483 KB)** ; `.bss` = heap statique configurable (8 MB en C.5 → 2065 pages). Total à mapper si heap = 8 MB : 2185 frames. **Conclusion : le code est minuscule (~121 caps) ; le poste dominant est le heap, configurable. Réduire le heap serveur (~2–3 MB plausible : cache redb 1 MB + journal RAM + buffers DMA) ramène le serveur à ~500–750 frames.** Le risque CNode racine (4096 slots) est maîtrisable : réduire les heaps + CNode propre par enfant + deleter uniquement les caps temporaires (les caps de frames d'image doivent rester résidentes — leur delete unmappe).

3. **C.6 :** porter spawn-task, établir runtime+serveur+ring+endpoint, agent WASM émet → commit Q3-C → état lisible. Signal `C6_PASS`.

4. **C.6-crash :** instrumenter KP1-KP4, oracle dans le serveur, asserter I1/I2/I3. Signal `C6-crash_PASS`.

5. **C.7** (ultérieur) : executor async (ADR-0037 §3), composition N rings, re-validation P6 concurrente en SEF-seL4.

## Références

- decisions/0038-store-natif-sel4.md (§3 invariant index, §44-65 atomicité Q3-C, §90 interface StoreServer, §7 topologie 2-processus)
- decisions/0027-durabilite-log-vs-contentstore.md (no-force vs force-at-commit, §205-227 oracle S6, régimes D1/D3)
- decisions/0042-voie-b3-moteur-index.md (§Amendement 2026-05-29 : redb = index reconstructible, jamais store direct)
- decisions/0037-stack-runtime-sel4.md (§3 executor async maison, repoussé C.7)
- decisions/0041-voie-b2-driver-block.md (driver virtio-blk C.4)
- lab/LESSONS.md (L68 : jalon de faisabilité ≠ topologie d'architecture)
- CLAUDE.md (§Conformité aux ADR)
- rust-sel4 rev 7a2321f2 : crates/examples/root-task/spawn-task/ (spawn process VSpace séparé), spawn-thread/ (tcb_suspend, tcb_configure), example-root-task/ (badge/mint/wait)
- seL4 Reference Manual v15.0.0 §5 (IPC), §6 (MessageInfo), §10 (objets VSpace/Frame)

---

## Amendement 2026-05-29 — précision sur l'invariant §3 (renvoi ADR-0038)

§28 affirme « l'index redb reste un cache reconstructible non-autoritaire, instancié EN AVAL du journal content-addressed ». La revue C.6→C.9 a établi que l'implémentation ne réalise PAS cette séparation : `TABLE_JOURNAL_A` (ordre) et l'index vivent dans la même transaction redb que les blobs/headers content-addressed, et l'ordre est donc autoritaire dans redb. Ce que C.6 corrige est l'inversion *topologique* de C.5 (séparation 2-processus runtime/serveur) — réelle et acquise. Ce qui n'est PAS instancié est la séparation *de stockage* §3. Voir ADR-0038 §Amendement 2026-05-29 (Q3) et LESSONS.md L82. P6 tient (atomicité transactionnelle ≥ Q3-C) ; aucune régression de propriété, écart purement sémantique/architectural.
