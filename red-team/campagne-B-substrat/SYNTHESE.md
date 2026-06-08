# Campagne B — Synthese : limites structurelles Linux -> argument seL4

**Date :** 2026-06-03
**Substrat evalue :** Linux x86-64, Wasmtime 25, 1 processus Tokio par noeud
**Substrat cible :** seL4 microkernel, Wasmtime min-platform, VSpaces separes

---

## Bilan des findings

| ID | Classe | Rejoue | Type | Statut |
|----|--------|--------|------|--------|
| B-1 | CVE Wasmtime actifs sur v25 | Non | Structurelle | **15 advisories actifs** dont 2 critiques (sandbox escape) ; 1 (aarch64) touche la cible seL4 ; upgrade = dette |
| B-1b | Dependance morte `wasmtime-wasi` | N/A | Correctible | **Retiree** (16→15 advisories ; RUSTSEC-2026-0149 elimine) |
| B-2 | Bounds check inconsistant (agent_check_cap, agent_add_cause) | Oui | Correctible | **Correctif applique** (2026-06-03) |
| B-3 | N agents dans 1 processus Linux | Partiellement | Structurelle | Sandbox WASM tenu ; post-evasion = tout le processus |
| B-4 | W^X Linux : mprotect() logiciel | Non | Structurelle | Revocable via kernel exploit |
| B-5 | TCB Linux ~30 MLOC, non prouve | Non | Structurelle | Non corrigeable par patch |

---

## 1 limite correctible

**B-2** : deux host functions utilisaient `ptr + len > data_len` au lieu de
`checked_add`. Non exploitable sur 64 bits, mais incoherent avec le reste du code.
Correctif applique et teste. Cela ne motive pas un changement de substrat.

---

## 4 limites structurelles confirmees -> argument seL4

### (a) La classe "sandbox escape" Wasmtime est ACTIVE sur v25 (B-1)

`cargo audit` (2026-06-03) remonte 15 advisories actifs sur wasmtime 25.0.3, dont
**deux critiques (CVSS 9.0) de classe sandbox escape** :
RUSTSEC-2026-0096 (miscompilation heap **aarch64 Cranelift**) et RUSTSEC-2026-0095
(backend Winch, non utilise ici). La classe n'est donc **pas** eliminee par
construction — RUSTSEC-2026-0096 en est une instance reelle, et elle touche
**directement la cible AArch64/seL4**. Seule une preuve formelle du compilateur
la fermerait. Solution editeur = upgrade ≥36.0.7/≥42.0.2/≥43.0.1, **bloque par
l'epinglage v25 pour la compat seL4 min-platform** → dette tracee (TODO).

**seL4 :** n'elimine pas les bugs Wasmtime, mais isole leur rayon d'impact.
Meme RUSTSEC-2026-0096 ne compromet que le VSpace de l'agent touche (B-3). C'est
precisement pourquoi l'isolation ne doit pas reposer sur le seul sandbox logiciel.

### (b) N agents dans 1 processus : isolation logicielle seulement (B-3)

Sur Linux, la memoire de N agents Tokio dans 1 processus est mutuellement accessible
apres une evasion Wasmtime. L'isolation est entierement logicielle (Wasmtime sandbox).

**seL4 :** chaque runtime dans son VSpace. Une evasion Wasmtime d'un agent ne donne
acces qu'a son propre VSpace — le microkernel bloque tout acces inter-VSpace.
Demontre en C.7-crash (I4 : non-interference d'integrite).

### (c) W^X Linux : contournable via privilege kernel (B-4)

mprotect() est revocable par une escalade de privileges. Un agent qui obtient ring 0
peut ecrire dans les pages RX de Wasmtime et executer du code arbitraire dans le
contexte du runtime.

**seL4 :** W^X garanti par les page tables hardware (capabilities seL4 non-revocables
depuis le domaine agent). Demontre en C.10 : ecriture sur page RX => vm fault seL4.

### (d) TCB Linux non prouve (~30 MLOC) (B-5)

Toute application Linux herite de la surface d'attaque du noyau. Les LPEs (Local
Privilege Escalation) sont une classe active de vulnerabilites. Un LPE bypass
tous les sandboxes applicatifs.

**seL4 :** 9 000 LOC C avec preuve formelle d'integrite (Isabelle/HOL). Aucun LPE
publie sur le code prouve. TCB orders of magnitude plus petit.

---

## Argument seL4 structure

```
Limite Linux                         Fermeture seL4
---------------------------------------- -----------------------------------------------
Sandbox escape => tout le processus   -> Sandbox escape => 1 VSpace uniquement (C.7)
W^X = mprotect() logiciel            -> W^X = page tables hardware (C.10)
TCB ~30 MLOC, LPEs actifs            -> TCB ~9K LOC C, integrite prouvee formellement
Isolation = logicielle (Wasmtime)     -> Isolation = materielle (MMU sous controle seL4)
```

Ces 4 classes sont fermees **par construction** sur seL4, pas par patch applicatif.
C'est la raison pour laquelle la spec (ADR-0037, ADR-0040, spec/08 §0.2) retient seL4
comme substrat cible, et non Linux durcisse.

---

## Ce qui ne change pas

Les proprietes P2/P3/P4/P6 validees sur Linux (campagne A, phases 7-13) restent
valides sous leurs hypotheses de substrat (D7). Les 4 limites structurelles ci-dessus
ne remettent pas en cause les verdicts de campagne A — elles precisent le perimetre
dans lequel ces verdicts sont valables (substrat non compromis).
