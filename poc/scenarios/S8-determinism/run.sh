#!/usr/bin/env bash
# S8 — Déterminisme de transition d'état test harness (SEF-6 / P5).
#
# Pour chaque répétition r ∈ 1..=K_RUNS :
#   1. Une DB fraîche (instance-a/, instance-b/) est créée sous work/run<r>/.
#   2. `sef6-runner --n-actions=1000 --clock-start=...` est lancé. Il :
#      - lance deux instances A et B avec LogicalClock(clock_start) identique ;
#      - envoie la même séquence de 1000 messages à chacune ;
#      - compare hash final + séquence d'action_ids ;
#      - vérifie 3 propriétés (P-α, P-β, P-γ).
#   3. Exit code 0 = pass, autre = fail.
#
# Total : K_RUNS répétitions. Le rapport agrégé est écrit dans report.json.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# Si on est lancé depuis poc/, le parent du parent est la racine du repo
if [ ! -d "$POC_DIR/runtime" ]; then
    POC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
fi
WORK_DIR="$SCRIPT_DIR/work"
REPORT="$SCRIPT_DIR/report.json"

export CXXFLAGS="${CXXFLAGS:--include cstdint}"

N_ACTIONS="${N_ACTIONS:-1000}"
K_RUNS="${K_RUNS:-5}"
CLOCK_START_BASE="${CLOCK_START_BASE:-1700000000000}"

cd "$POC_DIR"

# --- Étape 1 : compilation (release) ---------------------------------------
echo "[S8] Compilation sef6-runner (release)..."
if ! cargo build --release -p os-poc-runtime --bin sef6-runner 2>&1 | tail -3; then
    echo "[S8] FATAL : compilation échouée"
    exit 2
fi

RUNNER="$POC_DIR/target/release/sef6-runner"

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

    # agent_id distinct par run (collisions secondary index évitées) ; A et B
    # partagent le même par construction du runner (un seul --agent-id passé).
    AGENT_ID=$(printf "0000000000000000000000000000%04d" "$r")
    # clock_start déterministe par run (pour traçabilité dans le rapport agrégé)
    CLOCK_START=$((CLOCK_START_BASE + r))

    "$RUNNER" \
        --db-root  "$RUN_DIR" \
        --agent-id "$AGENT_ID" \
        --n-actions "$N_ACTIONS" \
        --clock-start "$CLOCK_START" \
        --out-report "$RUN_DIR/report.json" \
        >"$RUN_DIR/runner.out" 2>"$RUN_DIR/runner.err"
    exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        PASSED=$((PASSED + 1))
        echo "  [S8 run${r}] pass"
        DETAILS+=("$r:pass")
    else
        echo "  [S8 run${r}] FAIL — exit=$exit_code"
        echo "    --- runner.out (extrait) ---"
        tail -25 "$RUN_DIR/runner.out" | sed 's/^/    /'
        echo "    -----------------------------"
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
    echo "  \"scenario\": \"S8-determinism\","
    echo "  \"property\": \"P5\","
    echo "  \"sef\": \"SEF-6\","
    echo "  \"n_actions\": $N_ACTIONS,"
    echo "  \"clock_start_base\": $CLOCK_START_BASE,"
    echo "  \"k_runs\": $K_RUNS,"
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S8] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S8] Verdict global : $PASSED/$TOTAL pass"

[ "$VERDICT" = "pass" ]
