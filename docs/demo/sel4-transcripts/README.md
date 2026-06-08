# Transcripts seL4 — walkthrough isolation forte

Sorties **réelles** de QEMU AArch64 (seL4 15.0.0), capturées par
`poc/sel4-hello/demo-isolation.sh`. Rejouables sans rebuild : ces fichiers servent de
référence pour la démo quand on ne veut pas relancer un build conteneurisé de plusieurs
minutes devant le public.

## `c10-wx-phaseA.txt` — W^X matériel (jalon C.10)

Capturé le 2026-06-06 (`make test`, exit 0). Boot du noyau seL4 → root task → run W^X.

Lignes-clés (preuve d'isolation) :

```
[C10] 128 frames JIT pré-mappées RW+XN à VA 0x40000000
[C10] runtime W^X: module WASM instancié (JIT W^X actif)
[C10] runtime W^X: run() terminé — K=1 commit sous W^X ✓
C10_HAPPY_PASS                       ← un commit s'exécute normalement sous W^X
[C10] test négatif : tentative écriture sur page RX va=0x40010000
vm fault on data at address 0x40010000 with status 0x9308004f
[C10] C10_NEG_PASS ✓ (VM fault observé sur page RX — W^X prouvé)
```

**Ce que ça prouve :** l'écriture sur une page de code exécutable (RX) est refusée par
un **fault matériel du micronoyau seL4** — pas par une politique logicielle (mprotect)
contournable par un LPE kernel comme sur Linux (red team B, finding B-4).

**Ce que ça ne prouve PAS :** rien sur la performance. La **latence est non recevable sur
QEMU** (ADR-0046). seL4 ne supprime pas les bugs Wasmtime, il **borne leur rayon d'impact**
au VSpace de l'agent touché. Verdict substrat seL4, non transférable depuis Linux (D7).

## Régénérer

```sh
bash poc/sel4-hello/demo-isolation.sh            # W^X (C.10)
bash poc/sel4-hello/demo-isolation.sh --with-c7  # + I4 non-interférence (C.7)
```

Prérequis : Docker + image locale `rust-root-task-demo`. Les `*-run.log` produits par le
script ne sont pas versionnés (voir `.gitignore`) ; seul `c10-wx-phaseA.txt` (curé) l'est.
