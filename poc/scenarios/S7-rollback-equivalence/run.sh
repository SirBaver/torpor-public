#!/usr/bin/env bash
# S7 — Rollback transactionnel test harness (SEF-2 / P2).
#
# Pour chaque répétition r ∈ 1..=K_RUNS :
#   1. Une DB fraîche (store/, log/) est créée sous work/run<r>/.
#   2. `sef2-runner --n-actions=1000 --k-target=500` est lancé. Il :
#      - exécute 1 000 actions ;
#      - capture hash_at_k via le log ;
#      - appelle Scheduler::rollback(target_seq=499) ;
#      - vérifie 5 propriétés (P-α à P-ε).
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
K_TARGET="${K_TARGET:-500}"
K_RUNS="${K_RUNS:-5}"
ROLLBACK_BUDGET_MS="${ROLLBACK_BUDGET_MS:-100}"

cd "$POC_DIR"

# --- Étape 1 : compilation (release pour mesure de P-ε fiable) -------------
echo "[S7] Compilation sef2-runner (release)..."
if ! cargo build --release -p os-poc-runtime --bin sef2-runner 2>&1 | tail -3; then
    echo "[S7] FATAL : compilation échouée"
    exit 2
fi

RUNNER="$POC_DIR/target/release/sef2-runner"

# --- Étape 2 : runs ---------------------------------------------------------
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

PASSED=0
TOTAL=0
DURATIONS=()
DETAILS=()

for ((r = 1; r <= K_RUNS; r++)); do
    TOTAL=$((TOTAL + 1))
    RUN_DIR="$WORK_DIR/run${r}"
    mkdir -p "$RUN_DIR"

    # agent_id distinct par run : 16 octets dont l'octet bas porte r.
    AGENT_ID=$(printf "0000000000000000000000000000%04d" "$r")

    "$RUNNER" \
        --db-store "$RUN_DIR/store" \
        --db-log   "$RUN_DIR/log" \
        --agent-id "$AGENT_ID" \
        --n-actions "$N_ACTIONS" \
        --k-target  "$K_TARGET" \
        --rollback-budget-ms "$ROLLBACK_BUDGET_MS" \
        --out-report "$RUN_DIR/report.json" \
        >"$RUN_DIR/runner.out" 2>"$RUN_DIR/runner.err"
    exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        PASSED=$((PASSED + 1))
        DUR=$(grep -oE '"rollback_duration_ms":[[:space:]]*[0-9]+' "$RUN_DIR/report.json" \
              | head -1 | grep -oE '[0-9]+')
        DURATIONS+=("${DUR:-0}")
        echo "  [S7 run${r}] pass (rollback=${DUR:-?}ms)"
        DETAILS+=("$r:pass:${DUR:-0}")
    else
        echo "  [S7 run${r}] FAIL — exit=$exit_code"
        echo "    --- runner.out (extrait) ---"
        tail -20 "$RUN_DIR/runner.out" | sed 's/^/    /'
        echo "    -----------------------------"
        DETAILS+=("$r:fail:0")
    fi
done

# --- Étape 3 : rapport JSON ------------------------------------------------
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VERDICT="fail"
[ "$PASSED" -eq "$TOTAL" ] && VERDICT="pass"

{
    echo "{"
    echo "  \"timestamp\": \"$TIMESTAMP\","
    echo "  \"scenario\": \"S7-rollback-equivalence\","
    echo "  \"n_actions\": $N_ACTIONS,"
    echo "  \"k_target\": $K_TARGET,"
    echo "  \"rollback_budget_ms\": $ROLLBACK_BUDGET_MS,"
    echo "  \"k_runs\": $K_RUNS,"
    printf "  \"rollback_duration_ms\": ["
    for i in "${!DURATIONS[@]}"; do
        [ "$i" -gt 0 ] && printf ", "
        printf "%s" "${DURATIONS[$i]}"
    done
    echo "],"
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S7] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S7] Verdict global : $PASSED/$TOTAL pass"

[ "$VERDICT" = "pass" ]
