#!/usr/bin/env bash
# S13 — Persistance d'état après redémarrage (SEF-1).
#
# Pour chaque répétition r ∈ 1..=K_RUNS :
#   1. Un répertoire frais work/run<r>/{store,log} est créé.
#   2. `sef1-runner` est lancé avec N_ACTIONS=100 (spec §SEF-1 : « 100 documents »).
#      Le binaire effectue les deux phases en un seul run :
#        Phase 1 : N actions → shutdown propre (drop tx + await handle).
#        Phase 2 : réouverture des mêmes chemins → 4 propriétés :
#          P-α  get_header(H_before) intact et cohérent après réouverture.
#          P-β  n entrées du log >= avant shutdown (toutes lisibles).
#          P-γ  bloc de 64 octets bit-à-bit identique après réouverture.
#          P-δ  ActorInstance restauré via restore_from_evicted → premier
#               ActionResult a hash_before == H_before (chaîne causale intacte).
#   3. Exit code 0 = pass, autre = fail.
#
# Total : K_RUNS répétitions. Rapport agrégé dans report.json.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
if [ ! -d "$POC_DIR/runtime" ]; then
    POC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
fi
WORK_DIR="$SCRIPT_DIR/work"
REPORT="$SCRIPT_DIR/report.json"

export CXXFLAGS="${CXXFLAGS:--include cstdint}"

N_ACTIONS="${N_ACTIONS:-100}"
K_RUNS="${K_RUNS:-5}"

cd "$POC_DIR"

# --- Étape 1 : compilation (release) ----------------------------------------
echo "[S13] Compilation sef1-runner (release)..."
if ! cargo build --release -p os-poc-runtime --bin sef1-runner 2>&1 | tail -3; then
    echo "[S13] FATAL : compilation échouée"
    exit 2
fi

RUNNER="$POC_DIR/target/release/sef1-runner"

# --- Étape 2 : runs ---------------------------------------------------------
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

PASSED=0
TOTAL=0
DETAILS=()

for ((r = 1; r <= K_RUNS; r++)); do
    TOTAL=$((TOTAL + 1))
    RUN_DIR="$WORK_DIR/run${r}"
    mkdir -p "$RUN_DIR"

    # agent_id distinct par run pour éviter les collisions dans l'index secondaire.
    AGENT_ID=$(printf "0000000000000000000000000000%04d" "$r")

    "$RUNNER" \
        --db-store  "$RUN_DIR/store" \
        --db-log    "$RUN_DIR/log" \
        --agent-id  "$AGENT_ID" \
        --n-actions "$N_ACTIONS" \
        --out-report "$RUN_DIR/report.json" \
        >"$RUN_DIR/runner.out" 2>"$RUN_DIR/runner.err"
    exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        PASSED=$((PASSED + 1))
        echo "  [S13 run${r}] pass"
        DETAILS+=("$r:pass")
    else
        echo "  [S13 run${r}] FAIL — exit=$exit_code"
        echo "    --- runner.out (extrait) ---"
        tail -30 "$RUN_DIR/runner.out" | sed 's/^/    /'
        echo "    --- runner.err (extrait) ---"
        tail -10 "$RUN_DIR/runner.err" | sed 's/^/    /'
        echo "    ----------------------------"
        DETAILS+=("$r:fail")
    fi
done

# --- Étape 3 : rapport JSON ------------------------------------------------
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VERDICT="fail"
[ "$PASSED" -eq "$TOTAL" ] && VERDICT="pass"

{
    echo "{"
    echo "  \"timestamp\": \"$TIMESTAMP\","
    echo "  \"scenario\": \"S13-persistence-restart\","
    echo "  \"property\": \"persistance\","
    echo "  \"sef\": \"SEF-1\","
    echo "  \"n_actions\": $N_ACTIONS,"
    echo "  \"k_runs\": $K_RUNS,"
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S13] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S13] Verdict global : $PASSED/$TOTAL pass"

[ "$VERDICT" = "pass" ]
