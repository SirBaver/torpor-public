# ADR-0047 — Jalon C.10 : durcissement W^X du pool JIT Wasmtime sur seL4

**Date :** 2026-05-29
**Statut :** Acceptée (cadrage — code C.10/C.10-crash en session dédiée)

---

## Contexte

La revue soundness C.1→C.9 (2026-05-29) a relevé **S1** : le pool d'exécution JIT de Wasmtime (`runtime/src/platform.rs`) est un `static mut PAGE_POOL` de 128 pages (512 KB) dans le `.bss` du runtime, mappé **RWX en permanence** (le bit X vient de `add_rights`: PF_X→`grant(true)`). `wasmtime_mprotect` et `wasmtime_mmap_remap` sont des no-op. Cranelift écrit le code JIT puis l'exécute dans la même région writable+executable — aucune transition W→X. Dette assumée tracée ADR-0040 §247.

L'arbitrage S1 (architect, 2026-05-29) a d'abord conclu à un différé (Option C) sous déclencheur `T1∨T2∨T3` (T1 = module WASM non confié sur le JIT ; T2 = multi-tenant ; T3 = évasion sandbox publiée), réveil en **option B** (superviseur pré-mappe les frames JIT et passe les caps VSpace/frames au runtime ; `wasmtime_mprotect` = unmap+remap RX). **T1 a été confirmé déclenché** (exécution de WASM non confié prévue court terme) → bascule sur implémentation.

**Contrainte factuelle déterminante :** le pool JIT n'est exercé par **aucun jalon vivant**. c6→c8 ont le JIT (Wasmtime) mais sont **figés** (décision D1 : un PASS atteste d'un état daté, pas de refactor rétroactif). c9-reopen — seul jalon vivant — est en **Rust pur, sans Wasmtime** (`CHILD_CNODE_SIZE_BITS=2`, pas de `platform.rs`). Durcir et **valider** W^X exige donc un nouveau jalon vivant qui ré-exerce le JIT.

---

## Décision

### D1 — Ouvrir le jalon C.10. Ne pas dégeler c8.

Dégeler c8 pour y injecter W^X détruirait la propriété qui rend `C8_PASS` citable (P3a + P6 re-validées sur la chaîne complète, ADR-0045 §76-79). Le défaut W^X est **orthogonal** à ce que C.8 atteste (intégration store + atomicité crash) — un défaut orthogonal au verdict ne justifie pas de rouvrir le verdict. C.10 instancie le plan déjà acté en TODO §D1 (« crate commune au prochain jalon vivant c10+ ; c9 y migre »).

### D2 — Portée C.10-minimal. Le module non confié est renvoyé à C.11.

C.10 réintègre Wasmtime/JIT sur la base c8 (store redb/virtio-blk + reopen) + W^X option B, en exécutant un **module confié trivial**. W^X est vérifiable sans module non confié : la propriété de protection mémoire est indépendante de la provenance du module. Charger du non-confié (vecteur T1 réel) soulève sa propre grappe de décisions non tranchées (validation de provenance, configuration sandbox, limites de ressources) — c'est **C.11**, jalon distinct avec son modèle de menace.

### D3 — Définition opérationnelle de W^X (critère C10_PASS)

> Pour toute page du pool JIT, à tout instant observable, les bits de permission VSpace ne contiennent jamais simultanément `WRITE` et `EXECUTE`. La transition d'une page de l'état W (Cranelift écrit le code) à l'état X (exécution) passe par un remap explicite via cap VSpace.

**C10_PASS exige les trois :**
1. Le module trivial confié s'exécute end-to-end avec W^X actif (la chaîne JIT fonctionne sous contrainte de protection).
2. **Test négatif obligatoire** : une tentative d'écriture sur une page passée en état X produit un VM fault observable. Sans ce contre-exemple, C10_PASS ne prouve rien (piège L68 : happy path vert ≠ propriété démontrée).
3. La crate commune (D1 de la revue) est née : c9 y migre, c10 la consomme.

### D4 — Le runtime remappe lui-même (cap VSpace + caps frames JIT au runtime)

**Tranché : le runtime détient sa cap VSpace et les caps de ses frames JIT, et exécute lui-même le remap RW→RX.** (Alternative rejetée : remap délégué au superviseur via IPC — coût latence par remap JIT + complexité protocole, sans bénéfice de sûreté réel.)

**Justification au modèle de menace :** ADR-0037 a acté que le runtime est seul dans son VSpace et que l'isolation forte est au niveau **processus seL4** (sandbox WASM pour l'isolation inter-agent, pas la MMU intra-VSpace). Donner au runtime le contrôle de son propre VSpace ne lui confère **aucun pouvoir cross-composant** : le pire qu'un runtime compromis fait est se corrompre lui-même, ce que seL4 confine déjà (le serveur de store survit — P6). On ne délègue **pas** le pouvoir de `retype` (cohérent avec S2 least-privilege) : le superviseur retype les frames JIT et passe seulement les caps de frames + la cap VSpace au runtime.

### D5 — Budget CNode et sortie du pool hors `.bss`

Le pool JIT doit **sortir du `.bss`** : tant qu'il est un `static mut` dans un segment ELF, il est mappé avec les droits du segment (RWX) par `child_vspace`. Il devient un ensemble de **frames dédiées** mappées séparément, dont le runtime détient les caps.

Budget (chiffrage indicatif, à confirmer à la compilation — règle ADR-0043 §97 « compter avant de coder ») :
- Pool JIT = 128 frames → 128 caps dans le CNode runtime.
- + cap VSpace, EP, Notification, TCB → **CNode runtime `size_bits=8` (256 slots)** (vs 2 aujourd'hui).
- **Risque réel = CNode racine du superviseur** (4096 slots, déjà épuisé en C.3 à heap 16 MB — ADR-0043 §97) : les 128 frames JIT transitent par le CNode racine avant mint/copy vers le fils. Mitigations imposées : deleter les caps temporaires après transfert (sauf frames mappées résidentes), itérer sur plusieurs Untyped non-device si besoin, chiffrer le total avant de coder.

### D6 — P6 : smoke test suffit (sous condition)

C.10 réintroduit Wasmtime **au-dessus** de la chaîne de commit sans modifier le protocole de commit unitaire (format Record, `seL4_Call` atomique d'append, ordre Q3-C par ring SPSC). **Invariant ADR-0043 §71 préservé** → re-jouer les 4 kill-points Q3-C est redondant. **Smoke test P6 + smoke D-reopen suffisent.** Condition explicite : si C.10 modifiait le protocole de commit (non prévu), P6-crash complet redeviendrait obligatoire.

### D7 — Découpage C.10 / C.10-crash

| Jalon | Portée | Valide | Régime |
|-------|--------|--------|--------|
| **C.10** | Crate commune (D1) + réintégration Wasmtime/JIT sur base c8 + W^X option B, module confié trivial | W^X (def. D3) : happy path W→X + **test négatif** ; smoke P6 + smoke D-reopen | crash-processus α |
| **C.10-crash** | Idem + kill-point dans la fenêtre de remap RW→RX | L'atomicité crash n'est pas cassée par le remap : un crash dans la fenêtre où une page est ni W ni X (entre unmap et remap) laisse un état récupérable | crash-processus α |
| **C.11** (futur, T1 réel) | Chargement module **non confié** | Modèle de menace explicite : provenance, config sandbox, limites ressources | à cadrer |

C.10-crash n'est pas optionnel : introduire un unmap/remap dans le chemin chaud crée une fenêtre où une page peut être ni W ni X. Un crash dans cette fenêtre doit laisser un état récupérable — même classe de fenêtre que C.6-crash/C.7-crash existent pour fermer (à chiffrer, possiblement léger si la fenêtre est purement locale au VSpace runtime).

---

## Justification

### Pourquoi un nouvel ADR plutôt qu'un amendement ADR-0037

ADR-0037 décide la *stack runtime* (Wasmtime no_std + Cranelift, isolation S1b) ; C.10 en **hérite** sans la rejouer. C.10 introduit trois décisions structurantes nouvelles — réintroduction du JIT dans la chaîne **vivante** post-gel, durcissement W^X touchant le **modèle de menace** (surface « cap VSpace au runtime »), naissance de la **crate commune** — plus un impact modèle de menace. Trois décisions structurantes méritent un ADR propre.

### Le fait structurel qui motive tout C.10

Sous seL4, un composant ne peut durcir/remapper sa propre mémoire **que s'il détient les caps correspondantes**. Le runtime c8/c9 reçoit un VSpace entièrement câblé par le superviseur (`child_vspace.rs`) dont il n'a **pas** la cap, et son CNode (4 slots) ne contient aucune cap de frame. `wasmtime_mprotect` était donc *structurellement* incapable de fonctionner, pas seulement « pas encore implémenté ». Tout durcissement mémoire intra-runtime (W^X, guard pages, remap) est une décision **superviseur** (provisionnement de caps), pas une décision runtime. C'est l'invariant que D4/D5 instancient.

---

## Conséquences

**Positives :**
- Lève la dette S1 / ADR-0040 §247 (no-op mprotect) sur un substrat vivant.
- Crée la crate commune (D1), réduisant la duplication ~6× et le risque de dérive future.
- Établit le pattern de provisionnement de caps pour tout durcissement mémoire intra-composant ultérieur (guard pages, W^X data, etc.).

**Négatives :**
- CNode runtime ×64 (size_bits 2→8) ; pression accrue sur le CNode racine du superviseur (risque §97 à gérer).
- Sortir le pool du `.bss` complexifie l'allocation mémoire JIT (frames dédiées vs static).
- Un jalon de plus (C.10 + C.10-crash) avant de pouvoir charger du non-confié (C.11).

---

## Questions tranchées dans cet ADR (étaient bloquantes)

1. **Qui remappe** → le runtime (D4), pas le superviseur.
2. **Budget CNode** → runtime size_bits=8, pool hors `.bss`, mitigations §97 côté racine (D5) ; chiffrage exact à la compilation.
3. **C.10 modifie-t-il le protocole de commit §71** → non (D6) ⇒ smoke P6 suffit.

---

## Séquencement

**Cadrage (cet ADR) maintenant ; code C.10 puis C.10-crash en session(s) dédiée(s).** Ne pas enchaîner le code dans la foulée : les trois points ci-dessus devaient être tranchés avant la première ligne (modèle de menace, topologie de caps, budget ressource — pas de la plomberie). C.11 (non confié) est cadré séparément quand C.10/C.10-crash sont PASS.

---

## Références

- `decisions/0037-stack-runtime-sel4.md` (stack Wasmtime no_std, isolation S1b — héritée)
- `decisions/0040-chemin-sel4-hyperviseur-vs-natif.md` §247 (dette no-op mprotect — levée par C.10)
- `decisions/0043-integration-verticale-c6.md` §71 (invariant protocole de commit), §97 (risque CNode racine n°1)
- `decisions/0045-critere-completude-poc-sel4.md` §72 (P2 smoke post-store), §76-79 (C.8 PASS)
- `decisions/0046-scope-phase-9.md` (D-reopen, gel D1)
- `decisions/0036-autorité-causale-agent-add-cause.md` (B-fort dormant = déclencheur T2)
- `spec/08-modele-menace.md` §15, §101-111 (vecteurs supply-chain `.wasm`, isolation S1b)
- `poc/sel4-hello/c8-store/runtime/src/platform.rs:4-5,50,59,64` (no-op mprotect à remplacer)
- `poc/sel4-hello/c9-reopen/supervisor/src/main.rs:60` (`CHILD_CNODE_SIZE_BITS=2` → ≥8 côté JIT)
- `TODO.md` §Dettes soundness seL4 (S1, D1) ; revue C.1→C.9 (2026-05-29)
- `lab/LESSONS.md` L68 (capacité de brique ≠ propriété démontrée)
