#!/usr/bin/env bash
# ICSR-drop-caches — vérification I-CSR sous cache froid (spec/10 §4, ADR-0051 §Amendement).
#
# Régime : SIGKILL (process::exit) + flush page cache (drop_caches).
# C'est le régime adversarial identifié dans SEF-10 comme "recevable non exécutable" faute
# de root. Maintenant exécutable.
#
# Ce que ça teste :
#   1. icsr-writer écrit N commits puis exit(1) — aucun destructeur RocksDB, WAL non fermé.
#   2. echo 3 > drop_caches — vide le page cache OS (requiert root).
#   3. icsr-verifier rouvre store + log depuis le disque froid → vérifie I-CSR.
#
# Si la violation attendue par SEF-10 se produit (log WAL sur disque, store WAL absent),
# on verra snapshot_missing > 0 dans le rapport → violation I-CSR réelle.
#
# USAGE :
#   sudo ./run.sh [N_COMMITS] [BLOCK_SIZE]
#
# Ou sans sudo, si l'utilisateur courant a accès à drop_caches :
#   ./run.sh
#
# VARIABLES d'environnement :
#   ICSR_DIR    — répertoire de travail (défaut : ./work/<timestamp>)
#   AGENT_ID    — hex 32 chars (défaut : généré)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN_DIR="$POC_DIR/target/release"

N_COMMITS="${1:-100}"
BLOCK_SIZE="${2:-64}"
TS="$(date +%s)"
WORK="${ICSR_DIR:-$SCRIPT_DIR/work/$TS}"
AGENT_ID="${AGENT_ID:-0102030405060708090a0b0c0d0e0f10}"

mkdir -p "$WORK"
STORE="$WORK/store"
LOG="$WORK/log"
WITNESS="$WORK/witness.json"
REPORT="$WORK/report.json"

echo "[ICSR-drop-caches] work=$WORK n_commits=$N_COMMITS block_size=$BLOCK_SIZE"

# Vérifier que les binaires existent.
for bin in icsr-writer icsr-verifier; do
    if [[ ! -x "$BIN_DIR/$bin" ]]; then
        echo "[ICSR-drop-caches] ERREUR : $BIN_DIR/$bin introuvable."
        echo "  Compiler d'abord :"
        echo "    cd $POC_DIR && CXXFLAGS=\"-include cstdint\" cargo build --release -p os-poc-runtime --bin icsr-writer --bin icsr-verifier"
        exit 1
    fi
done

# ── Phase 1 : écriture + SIGKILL ──────────────────────────────────────────────
echo ""
echo "=== Phase 1 : écriture + process::exit(1) (SIGKILL simulé) ==="
"$BIN_DIR/icsr-writer" \
    --db-store "$STORE" \
    --db-log   "$LOG" \
    --witness  "$WITNESS" \
    --agent-id "$AGENT_ID" \
    --n-commits "$N_COMMITS" \
    --block-size "$BLOCK_SIZE" \
    --cut-mode exit || true   # exit code 1 attendu (process::exit)

echo "[Phase 1] témoin : $WITNESS"
echo "[Phase 1] commits écrits : $N_COMMITS"

# ── Phase 2 : vider le page cache ─────────────────────────────────────────────
echo ""
echo "=== Phase 2 : flush page cache (drop_caches) ==="
if [[ $EUID -eq 0 ]]; then
    # Exécuté en root directement.
    sync
    echo 3 > /proc/sys/vm/drop_caches
    echo "[Phase 2] drop_caches OK (root direct)"
elif sudo -n true 2>/dev/null; then
    # sudo sans mot de passe disponible.
    sync
    sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
    echo "[Phase 2] drop_caches OK (sudo)"
else
    echo "[Phase 2] ERREUR : drop_caches requiert root."
    echo "  Relancer avec sudo, ou en tant que root."
    echo "  Commande manuelle : sudo sh -c 'sync && echo 3 > /proc/sys/vm/drop_caches'"
    exit 1
fi

# ── Phase 3 : vérification I-CSR sous cache froid ─────────────────────────────
echo ""
echo "=== Phase 3 : vérification I-CSR (cache froid) ==="
"$BIN_DIR/icsr-verifier" \
    --db-store "$STORE" \
    --db-log   "$LOG" \
    --witness  "$WITNESS" \
    --out-report "$REPORT"

EXIT_VERIF=$?

echo ""
echo "=== Résultat ==="
echo "rapport : $REPORT"
grep -E '"(verdict|checked|log_missing|snapshot_missing|data_block_missing|icsr_ok)"' "$REPORT" || true

echo ""
if [[ $EXIT_VERIF -eq 0 ]]; then
    echo "VERDICT : PASS — I-CSR satisfait sous cache froid (régime SIGKILL + drop_caches)"
    echo "  → Les données ont atteint le disque avant la coupure. Cohérence cross-store maintenue."
else
    echo "VERDICT : FAIL — I-CSR violé sous cache froid"
    echo "  → Violation(s) détectée(s) : voir snapshot_missing / data_block_missing dans $REPORT"
    echo "  → Ce serait la matérialisation de la fenêtre SEF-10 (référence pendante cross-store)."
fi

exit $EXIT_VERIF
