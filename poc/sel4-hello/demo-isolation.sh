#!/usr/bin/env bash
# demo-isolation.sh — Walkthrough seL4 « isolation forte » (Lot C chantier démos).
#
# Montre, en direct sur QEMU AArch64, ce que le substrat Linux NE PEUT PAS garantir
# (red team B, red-team/campagne-B-substrat/SYNTHESE.md) :
#
#   W^X MATÉRIEL (jalon C.10) — un agent tente d'écrire sur une page de code
#   exécutable (RX). Le résultat n'est pas un avertissement logiciel (mprotect,
#   contournable par un LPE kernel sur Linux) mais un `vm fault` du micronoyau seL4,
#   garanti par les page tables matérielles dont les capabilities ne sont pas
#   révocables depuis le domaine agent.
#
#   (Optionnel : I4 non-interférence — jalon C.7-crash — une évasion WASM d'un agent
#   ne touche pas le VSpace d'un autre. `--with-c7`.)
#
# HONNÊTETÉ (à dire) :
#   - Verdict d'ISOLATION, PAS de performance. La latence est NON RECEVABLE sur QEMU
#     (ADR-0046) ; D-P3a (média réel) reste bloqué infrastructure.
#   - seL4 n'élimine pas les bugs Wasmtime — il BORNE leur rayon d'impact au VSpace
#     de l'agent touché (red team B, finding B-3/B-4).
#   - Substrat seL4 : les verdicts ne transfèrent PAS depuis Linux (D7) — c'est ici
#     qu'on prouve l'isolation matérielle que Linux n'offre pas.
#
# Prérequis : Docker + image locale `rust-root-task-demo` (seL4 15.0.0 + QEMU AArch64).
# Le build est conteneurisé ; aucun toolchain host requis. ~1 GB de target/ généré
# (nettoyé par `make clean` en fin de run, sauf --keep).
#
# Usage :
#   bash poc/sel4-hello/demo-isolation.sh            # W^X (C.10)
#   bash poc/sel4-hello/demo-isolation.sh --with-c7  # + I4 non-interférence (C.7)
#   bash poc/sel4-hello/demo-isolation.sh --keep     # ne pas nettoyer target/build

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TRANSCRIPT_DIR="$SCRIPT_DIR/../../docs/demo/sel4-transcripts"
WITH_C7=0
KEEP=0
for arg in "$@"; do
    case "$arg" in
        --with-c7) WITH_C7=1 ;;
        --keep)    KEEP=1 ;;
        *) echo "option inconnue : $arg" ; exit 2 ;;
    esac
done

say() { printf '\n\033[1;36m%s\033[0m\n' "$*"; }

# ── Préconditions ───────────────────────────────────────────────────────────────
if ! command -v docker >/dev/null 2>&1; then
    echo "ERREUR : Docker requis (les builds seL4 sont conteneurisés)." >&2
    exit 1
fi
if ! docker image inspect rust-root-task-demo >/dev/null 2>&1; then
    echo "ERREUR : image Docker 'rust-root-task-demo' absente." >&2
    echo "  Construire d'abord la stack seL4 (voir poc/sel4-hello/c1-hello/run-c1.sh)." >&2
    exit 1
fi
mkdir -p "$TRANSCRIPT_DIR"

# ── Acte 1 — W^X matériel (C.10) ─────────────────────────────────────────────────
say "════════════════════════════════════════════════════════════════"
say "  seL4 — ISOLATION FORTE  ·  Acte 1 : W^X matériel (jalon C.10)"
say "════════════════════════════════════════════════════════════════"
cat <<'EOF'
Sur Linux, W^X repose sur mprotect() — logiciel, révocable par un exploit kernel
(red team B, finding B-4). Sur seL4, l'interdiction d'écrire sur une page exécutable
est garantie par les page tables matérielles. On va le voir : un agent tente
d'écrire sur sa page de code (RX). Attendez la ligne « vm fault on data at address ».
EOF
say "[build + boot QEMU AArch64 — peut prendre plusieurs minutes]"

C10_LOG="$TRANSCRIPT_DIR/c10-wx-run.log"
( cd "$SCRIPT_DIR/c10-wx" && CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" make test ) 2>&1 | tee "$C10_LOG"

say "── Ce qui vient d'être prouvé (C.10) ──"
cat <<'EOF'
  C10_HAPPY_PASS : un commit s'exécute normalement sous W^X JIT actif.
  vm fault on data at address 0x40010000  →  C10_NEG_PASS :
    l'écriture sur la page RX est REFUSÉE par le micronoyau seL4 (fault matériel),
    pas par une politique logicielle contournable.
  → Transcript de référence : docs/demo/sel4-transcripts/c10-wx-phaseA.txt
EOF

# ── Acte 2 (optionnel) — I4 non-interférence (C.7) ───────────────────────────────
if [ "$WITH_C7" = "1" ]; then
    say "════════════════════════════════════════════════════════════════"
    say "  seL4 — ISOLATION FORTE  ·  Acte 2 : I4 non-interférence (C.7)"
    say "════════════════════════════════════════════════════════════════"
    cat <<'EOF'
Deux runtimes dans deux VSpaces seL4 distincts. Une évasion/corruption dans l'un
ne peut PAS toucher la mémoire de l'autre — le micronoyau bloque tout accès
inter-VSpace (non-interférence d'intégrité, style Biba).
EOF
    C7_LOG="$TRANSCRIPT_DIR/c7-crash-run.log"
    ( cd "$SCRIPT_DIR/c7-crash" && CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" make test ) 2>&1 | tee "$C7_LOG"
    say "→ Transcript C.7 : $C7_LOG"
fi

# ── Honnêteté finale ──────────────────────────────────────────────────────────────
say "── À DIRE À VOIX HAUTE (garde-fous) ──"
cat <<'EOF'
  • Verdict d'ISOLATION, pas de performance : latence NON recevable sur QEMU
    (ADR-0046) ; D-P3a (média réel) reste bloqué infrastructure.
  • seL4 ne supprime pas les bugs Wasmtime — il BORNE leur rayon d'impact au
    VSpace de l'agent touché (red team B).
  • Substrat seL4 : verdicts non transférables depuis Linux (D7).
EOF

# ── Hygiène disque (CLAUDE.md) ────────────────────────────────────────────────────
if [ "$KEEP" = "0" ]; then
    say "[nettoyage des artefacts de build (~1 GB) — --keep pour conserver]"
    ( cd "$SCRIPT_DIR/c10-wx" && rm -rf build target ) || true
    [ "$WITH_C7" = "1" ] && ( cd "$SCRIPT_DIR/c7-crash" && rm -rf build target ) || true
fi

say "Walkthrough seL4 terminé."
