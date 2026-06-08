#!/usr/bin/env bash
# S6 — Crash atomicity test harness (SEF-4 / ADR-0024 + ADR-0027).
#
# Pour chaque (kill_point, kill_action, run) :
#   1. Une DB fraîche est créée (store/, log/).
#   2. `sef4-victim --kill-at <point>:<k>` est lancé. Il exécute les actions 0..k-1,
#      sauvegarde l'état pre-kill dans expected.json, puis tue le processus pendant
#      l'action k (process::exit(1) — simulation SIGKILL ADR-0027 D3).
#   3. La même DB est rouverte par `sef4-verify` qui détermine l'état observable
#      via le log et le compare aux deux états admissibles :
#        - pre[k] (action k non committed)
#        - successeur direct de pre[k] (action k committed, parent = pre[k])
#
# Total : 4 kill_points × 2 kill_actions × K_RUNS répétitions.

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

N_ACTIONS=10
K_RUNS=5
AGENT_ID="00000000000000000000000000000001"
KILL_POINTS=(
    "pre_put_block"
    "between_put_block_and_put_snapshot"
    "post_put_snapshot_pre_log_append"
    "post_log_append"
)
KILL_ACTIONS=(3 4)

cd "$POC_DIR"

# --- Étape 1 : compilation ---------------------------------------------------
echo "[S6] Compilation des binaires (features crash-injection)..."
if ! cargo build -p os-poc-runtime --features crash-injection \
        --bin sef4-victim --bin sef4-verify 2>&1 | tail -3; then
    echo "[S6] FATAL : compilation échouée"
    exit 2
fi

VICTIM="$POC_DIR/target/debug/sef4-victim"
VERIFY="$POC_DIR/target/debug/sef4-verify"

# --- Étape 2 : crash runs ----------------------------------------------------
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

PASSED=0
TOTAL=0
DETAILS=()

for kp in "${KILL_POINTS[@]}"; do
    for ka in "${KILL_ACTIONS[@]}"; do
        for ((r = 1; r <= K_RUNS; r++)); do
            TOTAL=$((TOTAL + 1))
            RUN_DIR="$WORK_DIR/${kp}/a${ka}/run${r}"
            mkdir -p "$RUN_DIR"

            # Lance la victime — DOIT mourir avec exit 1 (CrashPoint::fire).
            "$VICTIM" \
                --db-store "$RUN_DIR/store" \
                --db-log   "$RUN_DIR/log" \
                --agent-id "$AGENT_ID" \
                --n-actions "$N_ACTIONS" \
                --kill-at "${kp}:${ka}" \
                --out-expected "$RUN_DIR/expected.json" \
                >"$RUN_DIR/victim.out" 2>"$RUN_DIR/victim.err"
            v_exit=$?

            if [ "$v_exit" -ne 1 ]; then
                echo "  [${kp} a${ka} run${r}] FAIL — victim exit=$v_exit (attendu 1)"
                echo "    --- victim.err (extrait) ---"
                tail -5 "$RUN_DIR/victim.err" | sed 's/^/    /'
                echo "    -----------------------------"
                DETAILS+=("$kp/$ka/$r:victim-exit-$v_exit")
                continue
            fi

            # Lance la vérification.
            "$VERIFY" \
                --db-store "$RUN_DIR/store" \
                --db-log   "$RUN_DIR/log" \
                --agent-id "$AGENT_ID" \
                --expected "$RUN_DIR/expected.json" \
                --kill-action "$ka" \
                >"$RUN_DIR/verify.out" 2>"$RUN_DIR/verify.err"
            x_exit=$?

            if [ "$x_exit" -eq 0 ]; then
                PASSED=$((PASSED + 1))
                echo "  [${kp} a${ka} run${r}] pass"
                DETAILS+=("$kp/$ka/$r:pass")
            else
                echo "  [${kp} a${ka} run${r}] FAIL — verify exit=$x_exit"
                echo "    --- verify.out ---"
                cat "$RUN_DIR/verify.out" | sed 's/^/    /'
                echo "    ------------------"
                DETAILS+=("$kp/$ka/$r:verify-exit-$x_exit")
            fi
        done
    done
done

# --- Étape 3 : rapport JSON --------------------------------------------------
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VERDICT="fail"
[ "$PASSED" -eq "$TOTAL" ] && VERDICT="pass"

{
    echo "{"
    echo "  \"timestamp\": \"$TIMESTAMP\","
    echo "  \"scenario\": \"S6-crash-atomicity\","
    echo "  \"n_actions\": $N_ACTIONS,"
    echo "  \"k_runs\": $K_RUNS,"
    printf "  \"kill_points\": ["
    for i in "${!KILL_POINTS[@]}"; do
        [ "$i" -gt 0 ] && printf ", "
        printf "\"%s\"" "${KILL_POINTS[$i]}"
    done
    echo "],"
    printf "  \"kill_actions\": ["
    for i in "${!KILL_ACTIONS[@]}"; do
        [ "$i" -gt 0 ] && printf ", "
        printf "%d" "${KILL_ACTIONS[$i]}"
    done
    echo "],"
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S6] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S6] Verdict global : $PASSED/$TOTAL pass"

[ "$VERDICT" = "pass" ]
