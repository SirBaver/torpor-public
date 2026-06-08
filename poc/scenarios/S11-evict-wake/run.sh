#!/usr/bin/env bash
# S11 — Cycle éviction/réveil (ADR-0030 §FutureWork).
#
# Teste le cycle complet :
#   1. Spawn N agents, exécuter n_actions chacun.
#   2. Évincer tous les agents (dormant).
#   3. Réveiller tous les agents (wake depuis ContentStore).
#   4. Vérifier continuité causale (P-γ) et log Suspended (P-δ).
#
# Usage : ./run.sh [--n-agents N] [--n-actions N] [--runs K]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
POC_DIR="$REPO_ROOT/poc"

N_AGENTS=3
N_ACTIONS=10
K_RUNS=3

while [[ $# -gt 0 ]]; do
    case "$1" in
        --n-agents)  N_AGENTS="$2";  shift 2 ;;
        --n-actions) N_ACTIONS="$2"; shift 2 ;;
        --runs)      K_RUNS="$2";    shift 2 ;;
        *) echo "Usage: $0 [--n-agents N] [--n-actions N] [--runs K]" >&2; exit 1 ;;
    esac
done

echo "[S11] Compilation s11-runner (release)..."
cd "$POC_DIR"
export CXXFLAGS="${CXXFLAGS:--include cstdint}"
cargo build --bin s11-runner --release --quiet
BIN="$POC_DIR/target/release/s11-runner"
echo "  OK : $BIN"

WORK_DIR="$SCRIPT_DIR/work"
mkdir -p "$WORK_DIR"

passed=0
max_io_list=()
elapsed_list=()

for run in $(seq 1 "$K_RUNS"); do
    run_dir="$WORK_DIR/run${run}"
    mkdir -p "$run_dir"
    report="$run_dir/report.json"
    db_dir="$run_dir/db"

    # Ignorer stderr pour masquer le bruit pthread de shutdown Wasmtime
    stdout=$("$BIN" \
        --n-agents  "$N_AGENTS" \
        --n-actions "$N_ACTIONS" \
        --out-report "$report" \
        --db-root "$db_dir" \
        2>"$run_dir/runner.err" || true)
    echo "$stdout" > "$run_dir/runner.out"

    verdict=$(echo "$stdout" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('verdict','?'))" 2>/dev/null || echo "?")

    if [[ "$verdict" == "pass" ]]; then
        passed=$((passed + 1))
        echo "  [S11 run${run}] pass"
    else
        echo "  [S11 run${run}] FAIL"
        cat "$run_dir/runner.err" >&2 || true
    fi
done

# Rapport consolidé
REPORT="$SCRIPT_DIR/report.json"
TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
python3 - <<EOF > "$REPORT"
import json, glob, os

runs_data = []
for i in range(1, ${K_RUNS}+1):
    p = "${WORK_DIR}/run{}/report.json".format(i)
    if os.path.exists(p):
        with open(p) as f:
            runs_data.append(json.load(f))

print(json.dumps({
    "timestamp": "${TS}",
    "scenario": "S11-evict-wake",
    "adr": "ADR-0030",
    "n_agents": ${N_AGENTS},
    "n_actions": ${N_ACTIONS},
    "k_runs": ${K_RUNS},
    "passed": ${passed},
    "total": ${K_RUNS},
    "verdict": "pass" if ${passed} == ${K_RUNS} else "fail",
    "runs": runs_data,
}, indent=2))
EOF

echo ""
echo "[S11] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S11] Verdict global : ${passed}/${K_RUNS} pass"
