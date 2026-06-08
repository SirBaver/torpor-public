#!/usr/bin/env bash
# S10 — Scheduler unifié C1+C2 (ADR-0030).
#
# Pipeline C2→C1 : IoAdmissionQueue gate les lectures ContentStore,
# puis InferencePool gate les inférences mock LLM.
#
# Propriétés vérifiées :
#   P-α  max I/O concurrent ≤ cap_io
#   P-β  max inférences concurrent ≤ k_infer (garanti par InferencePool)
#   P-γ  tous les agents complètent
#   P-δ  Supervisor médiane < Batch médiane (priorité observable)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
if [ ! -d "$POC_DIR/runtime" ]; then
    POC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
fi
WORK_DIR="$SCRIPT_DIR/work"
REPORT="$SCRIPT_DIR/report.json"

export CXXFLAGS="${CXXFLAGS:--include cstdint}"

N_AGENTS="${N_AGENTS:-8}"
CAP_IO="${CAP_IO:-3}"
K_INFER="${K_INFER:-2}"
INFER_DELAY_MS="${INFER_DELAY_MS:-50}"
K_RUNS="${K_RUNS:-3}"

cd "$POC_DIR"

# --- Compilation -----------------------------------------------------------
echo "[S10] Compilation s10-runner (release)..."
if ! cargo build --release -p os-poc-runtime --bin s10-runner 2>&1 | tail -3; then
    echo "[S10] FATAL : compilation échouée"
    exit 2
fi

RUNNER="$POC_DIR/target/release/s10-runner"

# --- Runs -----------------------------------------------------------------
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

PASSED=0
TOTAL=0
MAX_IO_LIST=()
ELAPSED_LIST=()
DETAILS=()

for ((r = 1; r <= K_RUNS; r++)); do
    TOTAL=$((TOTAL + 1))
    RUN_DIR="$WORK_DIR/run${r}"
    mkdir -p "$RUN_DIR"

    "$RUNNER" \
        --db-root        "$RUN_DIR/db" \
        --n-agents       "$N_AGENTS" \
        --cap-io         "$CAP_IO" \
        --k-infer        "$K_INFER" \
        --infer-delay-ms "$INFER_DELAY_MS" \
        --out-report     "$RUN_DIR/report.json" \
        >"$RUN_DIR/runner.out" 2>"$RUN_DIR/runner.err"
    exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        PASSED=$((PASSED + 1))
        MAX_IO=$(grep -oE '"max_io_concurrent":[[:space:]]*[0-9]+' "$RUN_DIR/report.json" \
                 | head -1 | grep -oE '[0-9]+')
        ELAPSED=$(grep -oE '"elapsed_ms":[[:space:]]*[0-9]+' "$RUN_DIR/report.json" \
                  | head -1 | grep -oE '[0-9]+')
        MAX_IO_LIST+=("${MAX_IO:-0}")
        ELAPSED_LIST+=("${ELAPSED:-0}")
        echo "  [S10 run${r}] pass (max_io=${MAX_IO:-?}, elapsed=${ELAPSED:-?}ms)"
        DETAILS+=("$r:pass:${MAX_IO:-0}:${ELAPSED:-0}")
    else
        echo "  [S10 run${r}] FAIL — exit=$exit_code"
        tail -10 "$RUN_DIR/runner.err" | sed 's/^/    /'
        DETAILS+=("$r:fail:0:0")
    fi
done

# --- Rapport JSON ---------------------------------------------------------
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VERDICT="fail"
[ "$PASSED" -eq "$TOTAL" ] && VERDICT="pass"

{
    echo "{"
    echo "  \"timestamp\": \"$TIMESTAMP\","
    echo "  \"scenario\": \"S10-unified-scheduler\","
    echo "  \"adr\": \"ADR-0030\","
    echo "  \"n_agents\": $N_AGENTS,"
    echo "  \"cap_io\": $CAP_IO,"
    echo "  \"k_infer\": $K_INFER,"
    echo "  \"infer_delay_ms\": $INFER_DELAY_MS,"
    echo "  \"k_runs\": $K_RUNS,"
    printf "  \"max_io_concurrent\": ["
    for i in "${!MAX_IO_LIST[@]}"; do
        [ "$i" -gt 0 ] && printf ", "
        printf "%s" "${MAX_IO_LIST[$i]}"
    done
    echo "],"
    printf "  \"elapsed_ms\": ["
    for i in "${!ELAPSED_LIST[@]}"; do
        [ "$i" -gt 0 ] && printf ", "
        printf "%s" "${ELAPSED_LIST[$i]}"
    done
    echo "],"
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S10] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S10] Verdict global : $PASSED/$TOTAL pass"

[ "$VERDICT" = "pass" ]
