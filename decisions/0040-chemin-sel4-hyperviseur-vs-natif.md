# ADR-0040 — Chemin seL4 : hyperviseur vs substrat natif

**Date :** 2026-05-28  
**Statut :** **Acceptée — Chemin B (substrat natif) retenu** (2026-05-28). Voir §7 (Décision) et §8 (Justification).

---

## Contexte

ADR-0002 a tranché le **substrat du PoC** (Wasmtime + RocksDB + Tokio sur Linux). ADR-0037 a tranché la **stack runtime** sur seL4 (Wasmtime `min-platform` + executor Rust maison). ADR-0038 a tranché le **store natif seL4** (ring buffer + IPC commit, in-TCB). ADR-0039 a tranché la **cible PoC Phase 8** (AArch64 QEMU). `spec/09` décrit le tableau de transfert PoC → seL4.

Aucun de ces ADR ne tranche une question plus en amont : **quel rôle joue seL4 dans l'architecture finale ?** Deux chemins sont compatibles avec ce qui a déjà été décidé, et conduisent à des architectures fondamentalement différentes :

- **Chemin A — seL4 comme hyperviseur.** Un Linux minimaliste tourne en VM sur seL4 et fournit les drivers (NVMe, réseau, USB, etc.) et la stack hôte (RocksDB, glibc, fsync). L'OS-pour-IA tourne soit dans une seconde VM (au-dessus de seL4 directement), soit comme processus dans la VM Linux. seL4 joue le rôle d'isolateur de domaines (VMM formellement vérifié) et de TCB minimal sous-jacent.
- **Chemin B — seL4 comme substrat natif.** Tout en Rust/WASM directement sur le micro-noyau. Zéro Linux dans la stack. La plomberie système (driver block, stack réseau si nécessaire, FS, moteur d'index persistant) est réécrite — voir ADR-0038 §6 (B2, B3). RocksDB est remplacé par un store natif Rust no_std.

Le choix n'est pas un détail d'implémentation. Il conditionne :

1. **La surface d'attaque** (spec/08 §0 — TCB). Chemin A inclut un noyau Linux complet (~30 MLOC) dans le TCB de fait pour les drivers. Chemin B garde seL4 (~10 KLOC vérifiés) + runtime Rust (~10–50 KLOC à écrire) + Wasmtime min-platform.
2. **Le périmètre de réécriture** (spec/09 catégories B/D). Chemin A préserve une grande partie de la stack PoC (RocksDB reste, fsync existe, Tokio peut tourner dans la VM Linux). Chemin B impose la réécriture identifiée dans ADR-0038 §6 (driver block, moteur d'index) et des composants Tokio (ADR-0030/0031 catégorie B).
3. **Le sens du tableau S1–S7** (spec/02b §3). L'agent responsable de chaque exigence diffère entre A et B (voir §3 ci-dessous).
4. **Les jalons C.1/C.2/C.3** (ADR-0039) et le scope Phase 8/9. Le travail déjà entamé (root task AArch64 + Wasmtime no_std) est compatible avec les deux chemins, mais leur prolongation diffère.

Ce choix n'a jamais été formalisé. ADR-0037 a écarté l'option "WasmEdge + Guest Linux sur seL4" (§5) comme "hors objectif", ce qui suggère implicitement un alignement sur le Chemin B, sans le formaliser ni examiner le Chemin A comme alternative équivalente. ADR-0038 §B2 voie (ii) sDDF/blk laisse ouverte la possibilité d'un driver block C dans le TCB, ce qui est compatible avec une variante du Chemin A. Cet ADR ouvre le débat sans le trancher.

---

## Statut

**Acceptée — Chemin B (substrat natif) retenu, 2026-05-28.** Déclencheur §5.1.1 atteint (ouverture Phase 9 — choix B2 voie i/ii/iii). Voir §7 (Décision) et §8 (Justification). Les ADR-0037/0038/0039 deviennent ipso facto des décisions de la cible finale (plus seulement de PoC). Les sections §1–§6 sont conservées pour traçabilité du raisonnement et restent l'arbre comparatif source.

---

## 1. Chemin A — seL4 comme hyperviseur

### Description

seL4 tourne en EL2 (AArch64) ou en mode root (x86_64) comme VMM. Une VM Linux minimaliste (kernel seul + initramfs Rust statique, sans userspace POSIX généraliste) fournit :

- Drivers hardware (block NVMe/virtio-blk, NIC, IRQ controllers complexes).
- RocksDB compilé contre glibc (ou musl) — pas de réécriture.
- Stack réseau Linux si nécessaire (TCP/IP, TLS).
- `fsync` et la sémantique POSIX de durabilité.

L'OS-pour-IA s'exécute soit :
- **A.1** dans la VM Linux (processus Rust au-dessus de glibc/musl, accès direct à RocksDB et fsync). seL4 joue le rôle d'isolateur de domaines (VMM), mais le runtime acteur lui-même reste un processus Linux.
- **A.2** dans une seconde VM (paravirtualisée, runtime Rust no_std comme aujourd'hui sur ADR-0037), avec passage de capabilities de bloc/réseau depuis la VM Linux via IPC seL4 (analogue sDDF mais inter-VM).

### Variante voisine

ADR-0038 §B2 voie (ii) — **sDDF/blk dans un composant C séparé** (pas une VM Linux complète, mais un driver C compilé contre `libsel4` + ABI sDDF) — est une variante intermédiaire entre A et B : C dans le TCB, mais pas Linux complet. À traiter explicitement comme *tertium quid* si A et B sont jugés trop polarisés.

### Implications S1–S7

| Exigence | Implication Chemin A |
|----------|----------------------|
| **S1** | S1a inter-VM (MMU + EPT/Stage-2) ; S1b intra-runtime (sandbox WASM, identique ADR-0037) |
| **S2** | RocksDB préservé. Zéro réécriture du store. |
| **S3** | CausalLog + RocksDB préservés (ADR-0011, ADR-0035). |
| **S4** | Double frontière (WASI + syscalls Linux dans la VM). Linux dans le TCB de fait. |
| **S5** | Tokio préservé (ADR-0030/0031 inchangés). |
| **S6** | Clock au-dessus de `/dev/urandom` Linux (ADR-0028). |
| **S7** | Overhead VM ≥ 64 MB (kernel + initramfs). Densité intra-VM inchangée. |

### Coûts / bénéfices

**Bénéfices :**
- Aucune réécriture de RocksDB, driver block, moteur d'index (B2/B3 d'ADR-0038 s'effondrent).
- Le PoC Linux existant transfère directement.
- `fsync` et durabilité power-loss disponibles dès Phase 8.
- Précédents industriels : CAmkES VMM seL4 [Heiser et al. 2020], déploiements automotive.

**Coûts :**
- TCB OS de fait = seL4 (~10 KLOC vérifiés) + Linux kernel complet (~30 MLOC non vérifié). spec/08 §0.2 doit être relue.
- L'argument "seL4 formellement vérifié" perd sa portée si Linux est dans le chemin critique.
- Overhead mémoire VM impacte P1 (densité) en multi-tenant.
- **Question latente :** quel est l'intérêt de seL4 par rapport à KVM/Xen pour le cas A ? Le formalisme du VMM seL4 est-il décisif, ou est-ce un choix d'identité de projet ?

---

## 2. Chemin B — seL4 comme substrat natif

### Description

Aucun Linux dans la stack. Tout en Rust no_std sur seL4 (ADR-0037 + ADR-0038 prolongés à leur conclusion logique). Composants :

- **seL4** : micro-noyau formellement vérifié (~10 KLOC).
- **Runtime Rust** : Wasmtime min-platform + executor async maison (ADR-0037).
- **Serveur de stockage** : ring buffer + IPC commit + moteur d'index Rust natif (ADR-0038).
- **Driver block** : Rust pur (ADR-0038 §B2 voie i) ou Rust + sDDF C minimal (voie ii) ou virtio-blk Rust existant (voie iii).

### Implications S1–S7

| Exigence | Implication Chemin B |
|----------|----------------------|
| **S1** | S1b sandbox WASM intra-runtime (identique A). S1a au niveau processus runtime (1 VSpace seL4 — ADR-0037). |
| **S2** | Store natif à écrire (ADR-0038 §3). Atomicité par log content-addressed. |
| **S3** | CausalLog porté sur le store natif. |
| **S4** | Effets médiés par WASI + capabilities seL4. TCB syscall minimal. |
| **S5** | Executor Rust maison (ADR-0037 §3) remplace Tokio. |
| **S6** | Clock au-dessus de timer seL4 natif (ADR-0028 adapté). |
| **S7** | Overhead par-agent ≈ 10–20 KB (ADR-0037 §4). Pas d'overhead VM. |

### Coûts / bénéfices

**Bénéfices :**
- TCB OS minimal : seL4 (vérifié) + Rust pur. spec/08 §0.2 satisfait en option α ou β.
- Densité optimale. Cohérence avec l'identité du projet ("OS pour IA bare-metal").
- Précédents : KataOS [Google 2022] — seL4 + Rust natif sans Linux.

**Coûts :**
- Réécriture lourde : driver block (6–9 mois solo), moteur d'index (4–6 mois), executor async (3–5 semaines).
- Aucun précédent public de Wasmtime tournant nativement sur seL4 sans Linux (ADR-0037 §4).
- `fsync` n'existe pas. Durabilité power-loss reportée Phase 9+.

---

## 3. Tableau comparatif S1–S7

| Exigence | Chemin A | Chemin B |
|----------|----------|----------|
| **S1a** (MMU) | Inter-VM oui ; inter-agent non (sandbox WASM) | Inter-processus runtime oui ; inter-agent non |
| **S2** (store) | RocksDB préservé (zéro réécriture) | Store Rust natif à écrire (ADR-0038) |
| **S3** (causalité) | Préservé | Porté sur store natif |
| **S4** (effets) | Double frontière (WASI + syscalls Linux) | Frontière unique (WASI + IPC seL4) |
| **S5** (séquentialité) | Tokio préservé | Executor Rust maison |
| **S6** (horloge) | Clock au-dessus de Linux | Clock au-dessus de timer seL4 |
| **S7** (overhead) | ≥ 64 MB overhead VM | 10–20 KB/agent |
| **TCB OS effectif** | seL4 + Linux kernel + runtime | seL4 + runtime Rust (option α/β selon B2) |

---

## 4. Implications sur le tableau de transfert (spec/09)

### Si Chemin A retenu

- ADR-0011, ADR-0035 → catégorie D ("non portable") deviendraient catégorie A ("intégral").
- ADR-0027 → `fsync` reste disponible. Pas de Phase 9+ dédiée au power-loss seL4.
- ADR-0038 → son objet (store natif) devient sans objet, ou se limite à la communication inter-VM.
- ADR-0030/0031 → Tokio reste utilisable intra-VM.

### Si Chemin B retenu

- Tableau spec/09 inchangé. ADR-0037/0038/0039 sont les premiers chaînons d'une longue série.
- ADR-0038 §B2 et §B3 doivent être tranchés (driver block + moteur d'index).
- ADR-réseau-seL4 à ouvrir si le scope dépasse "exécution locale".

---

## 5. Critère de déclenchement

Cet ADR doit être tranché avant l'un des événements suivants :

### 5.1 Déclencheurs externes (force le tranchage)

1. **Ouverture de Phase 9** (driver block + moteur d'index, ADR-0038 §6). Choisir B2 voie (i/ii/iii) suppose A ou B tranché.
2. **Première rédaction de la "ADR-seL4 cible"** mentionnée dans spec/09 ligne 0002 (catégorie B). Cet ADR ne peut être écrit sans avoir tranché A vs B.
3. **Décision d'investissement de 3+ semaines** dans l'executor Rust maison (ADR-0037 §3). Au-delà de ce point, le coût coulé oriente vers B.

### 5.2 Conditions de tranchage (informent la décision)

- **C.3 validé** : Wasmtime no_std exécute un module WASM sur seL4 AArch64. Conditionne la viabilité technique de B.
- **Investigation des précédents** : CAmkES VM [Heiser 2020] et KataOS [Google 2022] lus et leurs trade-offs synthétisés.
- **Position explicite sur spec/08 §0.2** (politique C dans le TCB — α/β/γ). Si γ retenu, A est exclu. Si α/β, A reste candidat.
- **Modèle de déploiement cible clarifié** : mono-machine vs distribué, mono-tenant vs multi-tenant.
- **Bénéfice marginal de seL4 sur le Chemin A** explicité : en quoi seL4 est-il préférable à KVM ou Xen dans ce cas ?

### 5.3 Statut par défaut tant que non tranché

- ADR-0037/0038/0039 restent valides comme décisions de **PoC seL4 Phase 8** sans préjuger du chemin final.
- Les jalons C.1/C.2 (root task + retype Untyped) sont **communs aux deux chemins** : leur travail n'est pas perdu.
- C.3 (Wasmtime dans la root task) est orienté B mais ne ferme pas A : Wasmtime peut aussi tourner dans une VM Linux.

---

## 6. Ce qui n'est PAS tranché par cet ADR

- Le choix A vs B lui-même.
- Le statut d'une variante intermédiaire (sDDF C dans TCB sans Linux complet).
- La question du modèle de déploiement (bare-metal, serveur, embedded, edge).
- L'utilité comparée de seL4 vs KVM/Xen dans le Chemin A.
- La feuille de route post-C.3 (Phase 9+).

---

## 7. Décision

**Chemin B retenu : seL4 comme substrat natif.**

Aucun Linux invité dans la stack cible. Tout en Rust no_std + Wasmtime `min-platform` au-dessus du micro-noyau seL4. Le driver block (B2 d'ADR-0038 §6) et le moteur d'index persistant (B3 d'ADR-0038 §6) sont à écrire en Rust (voies i/iii) ou en C minimal isolé via IPC sDDF (voie ii), selon arbitrage Phase 9.

**La variante "sDDF C dans TCB sans Linux complet"** (mentionnée §1 *Variante voisine*) est **un sous-cas du Chemin B**, pas un tertium quid : elle correspond à la voie (ii) d'ADR-0038 §B2. Elle est compatible avec spec/08 §0.2 option α et reste éligible. La sélection finale entre voies (i)/(ii)/(iii) sera tranchée par un ADR Phase 9 dédié (ADR-0041 prévu, B2 Phase 9 dans `TODO.md` §304-305).

**Conséquences immédiates :**

- ADR-0037 (stack runtime) et ADR-0038 (store natif) cessent d'être conditionnels et deviennent des décisions de cible finale.
- spec/09 reste valide tel quel : catégories A (transfert intégral), B (concept transfère, impl à refaire), C (méthodologie), D (jetées, RocksDB-spécifiques). Aucune ADR ne migre de D vers A.
- spec/08 §0.2 option α est consolidée comme politique TCB cible.
- Phase 9 ouvre B2 + B3 (TODO.md lignes 305–306) sans ambiguïté de chemin.
- Le travail Phase 8 (C.1/C.2/C.3) est intégralement réinvesti dans la cible.

**Ce qui n'est PAS tranché par cette décision (Phase 9+) :**

- Voie B2 driver block — (i) Rust from scratch, (ii) sDDF C minimal isolé, (iii) virtio-blk Rust existant. À évaluer empiriquement (ADR-0041 prévu).
- Voie B3 moteur d'index — `redb` backend custom, `sled`, ou LSM Rust maison. À investiguer (consulter agent `rust`).
- Couverture power-loss (P6 niveau (4) ADR-0038 Q2). Reportée à Phase 10+.
- Statut réseau (stack TCP/IP) — `lions/lwip` ou Rust pur. Hors scope tant que le déploiement reste mono-machine.

---

## 8. Justification de la décision

La décision est tranchée par convergence de cinq critères §5.2, par ordre de force décroissante :

### 8.1 spec/08 §0.2 option α exclut le Chemin A (argument déterminant)

L'option α retenue le 2026-05-27 (spec/08 §0.2 lignes 38–43) admet du C dans le TCB OS **à condition que** :
(a) le composant soit isolé dans un processus seL4 distinct,
(b) son interface soit une ABI IPC typée et documentée,
(c) son LOC soit auditable — cible explicite **< 5 KLOC C**.

Un noyau Linux complet (~30 MLOC C, ABI syscall non strictement typée, surface d'attaque massive) **viole les trois conditions**. Le Chemin A est donc incompatible avec une décision déjà actée et récente. La rétro-révision de spec/08 §0.2 pour admettre Linux dans le TCB OS reviendrait à invalider la cohérence du projet (l'argument "TCB minimal vérifiable" — spec/02-properties.md P4 et spec/08 §0 — perdrait son objet).

La variante "sDDF/blk C isolé" est explicitement admise par α (LOC sDDF cible < 5 KLOC, ABI typée [Heiser et al. 2024 sDDF design notes]) et reste éligible **dans le Chemin B** (voie ii d'ADR-0038 §B2).

### 8.2 Le modèle de déploiement ne justifie pas le coût du Chemin A

Le PoC est mono-machine, mono-tenant (spec/08 §3 R1/R3 confirment l'absence de multi-tenant à court terme). Le pattern industriel du Chemin A (CAmkES VMM, [seL4 Foundation, camkes-vm-examples]) cible des déploiements où Linux invité apporte de la **valeur métier** (legacy applicatif, stack POSIX requise). Dans notre cas, Linux ne serait **qu'une bibliothèque de drivers** — une dépendance trop chère pour ce qu'elle apporte :

- Surface d'attaque : +30 MLOC C non vérifié.
- Empreinte mémoire : ≥ 64 MB/VM (impacte directement P1, densité).
- Frottement opérationnel : 2 modèles d'exécution coexistants (VM + runtime natif).

Ces coûts sont indéfendables si l'unique bénéfice est l'évitement d'un driver block à écrire.

### 8.3 Si on voulait le Chemin A, seL4 ne serait pas le bon choix

Le Chemin A repose sur des hyperviseurs Linux-friendly. KVM/Xen ont 15+ ans de hardening industriel, des écosystèmes virtio/libvirt matures, et un coût opérateur très inférieur à seL4 VMM. Le seul avantage marginal de seL4 comme VMM est la **vérification formelle de l'isolation stage-2** ([seL4 microkernel for virtualization use-cases, MDPI Electronics 2022, arxiv:2210.04328] ; [Klein 2009]) — pertinent en automotive ASIL-D, avionique DAL-A, ou défense MILS. Ce n'est pas notre cas.

**Conséquence logique** : si l'analyse honnête conduit à choisir A, alors **le travail Phase 8 (ADR-0037/0038/0039 + jalons C.1/C.2/C.3) devient sans objet** — il faudrait passer à KVM. C'est un coût de bascule (~6 mois) qui se présenterait *après* avoir investi dans seL4. Le rejet du Chemin A consolide les choix passés.

### 8.4 C.3 (commit fa4ab50) prouve la viabilité technique du Chemin B

Avant le 2026-05-28, l'argument principal contre B était "aucun précédent public de Wasmtime nativement sur seL4 sans Linux" (ADR-0037 §4, ADR-0038 §8). Ce risque est désormais **falsifié empiriquement** :

- Wasmtime 25 `runtime` (sans Cranelift, sans `std`) sur seL4 AArch64 QEMU exécute `add(21, 21) = 42`.
- 13 fonctions plateforme implémentées en Rust pur (memory mapping via `frame_map`/`pt_map`, bump allocator, setjmp/longjmp minimal, mprotect/munmap/mmap_remap no-op).
- Module AOT-compilé hors-ligne (`Cranelift x86_64 → cwasm AArch64`), désérialisé par `Module::deserialize` — pattern reproductible.

Coût empirique mesuré : 13 fonctions plateforme + arbitrage `heap_size = 4 MB` (le 16 MB initial épuisait le CNode 4096 caps). Inférieur à la fourchette haute prévue (3–6 sem ADR-0037).

### 8.5 KataOS [Google 2022] valide le pattern Chemin B

Précédent industriel direct : [Google AmbiML 2022, *Announcing KataOS and Sparrow*]. Architecture quasi-isomorphe à la cible ADR-0037/0038 — seL4 + Rust quasi-entier + rootserver Rust + composants CAmkES statiques, zéro Linux invité. crate `sel4-sys` (réutilisé par notre stack via `rust-sel4` rev `7a2321f2`).

Limites du précédent : (a) activité publique limitée depuis 2022 — on ne sait pas si Google maintient en interne ; (b) cible ML embarquée (OpenTitan/Sparrow) plus simple que la nôtre (pas de Wasmtime, agents ML compilés statiquement). Ce qui est validé : le pattern technique (seL4 + Rust no_std + rootserver Rust) fonctionne et est intégré au cycle officiel `seL4 Foundation`. Ce qui reste à notre charge : Wasmtime + executor async + store natif.

### 8.6 Risque résiduel accepté

- **Coût d'écriture** : 6–9 mois driver block (B2 voie i, fourchette haute), 4–6 mois moteur d'index (B3), 3–5 sem executor async. Mitigation : B2 voie (ii) sDDF/blk ramène à 6–10 sem (le code C reste isolé via IPC, conforme α) ; voie (iii) à 2–4 sem si un driver Rust virtio-blk seL4 utilisable existe (à vérifier en ouverture Phase 9).
- **Power-loss non couvert avant Phase 10+** : assumé par ADR-0038 Q2 niveau (1). Identique à la situation Phase 6 Linux (ADR-0027 D3).
- **Pas de fsync** : assumé. Le serveur de stockage in-TCB tient lieu de garantie de durabilité processus.
- **sDDF/blk non encore formellement vérifié** (au-ts/sDDF v0.6.0, mars 2025 — "block class : preliminary specs and prototype implementations" [au-ts/sDDF GitHub], pas Cogent-verified). Acceptable si voie (ii) retenue : reste compatible α tant que ABI typée et LOC auditable. Si une preuve Cogent ou équivalent émerge plus tard, le composant passe de "C audité" à "C vérifié" sans rupture d'architecture.

---

## Références

- `decisions/0002-choix-substrat.md` — substrat PoC (Wasmtime + RocksDB + Tokio sur Linux)
- `decisions/0037-stack-runtime-sel4.md` — stack runtime seL4 (Wasmtime min-platform)
- `decisions/0038-store-natif-sel4.md` — store natif seL4
- `decisions/0039-cible-poc-aarch64.md` — PoC Phase 8 AArch64 QEMU
- `spec/02b-substrate_requirements.md` §3 (S1–S7), §5.1 (substrats survivants)
- `spec/08-modele-menace.md` §0 (TCB), §0.2 (politique C dans le TCB)
- `spec/09-transfert-poc-sel4.md` — tableau de transfert PoC → seL4
- [Klein 2009] "seL4: Formal Verification of an OS Kernel", SOSP 2009
- [Heiser et al. 2020] CAmkES VMM seL4 — déploiement automotive seL4 + Linux VM
- [Google 2022] "KataOS and Sparrow" — seL4 + Rust natif, sans Linux invité
- [seL4 Foundation 2023] sel4-microkit, lionsos.org, sDDF
- [seL4 Foundation, *CAmkES VMM*] — docs.sel4.systems/projects/camkes-vm/, github.com/seL4/camkes-vm-examples (vm_minimal, vm_cross_connector, vm_multi)
- [Google AmbiML 2022] *Announcing KataOS and Sparrow* — opensource.googleblog.com/2022/10/announcing-kataos-and-sparrow.html, github.com/AmbiML/sparrow-kata-full
- [seL4 microkernel for virtualization use-cases, MDPI Electronics 2022] arxiv:2210.04328 — comparaison VMM seL4 vs KVM/Xen
- [au-ts/sDDF v0.6.0, mars 2025] — github.com/au-ts/sDDF (block class : prototype, non-vérifié)

---

*ADR ouvert le 2026-05-28 suite à discussion externe (neutre entre A et B). **Tranché le 2026-05-28** au moment de l'ouverture Phase 9 (déclencheur §5.1.1). Décision : Chemin B (substrat natif). Justification §8.*
