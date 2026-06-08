#!/usr/bin/env bash
# S12 — SchedulerCoordinator réveil à la demande (ADR-0031).
#
# Teste Scheduler::deliver :
#   1. Spawn N agents, exécuter n_actions chacun.
#   2. Évincer n_dormant agents (table dormant).
#   3. deliver() sur les dormants → réveil via C2 + livraison.
#   4. deliver() sur les actifs → livraison directe (bypass C2).
#
# Propriétés :
#   P-α : tous les dormants réveillés (n_woken == n_dormant)
#   P-β : cap_io respecté (jamais plus de cap_io réveils simultanés)
#   P-γ : actifs reçoivent sans C2 (direct_deliveries == n_active)
#
# Usage : ./run.sh [--n-agents N] [--n-dormant N] [--cap-io N] [--n-actions N] [--runs K]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
POC_DIR="$REPO_ROOT/poc"

N_AGENTS=6
N_DORMANT=3
CAP_IO=2
N_ACTIONS=5
K_RUNS=3

while [[ $# -gt 0 ]]; do
    case "$1" in
        --n-agents)  N_AGENTS="$2";  shift 2 ;;
        --n-dormant) N_DORMANT="$2"; shift 2 ;;
        --cap-io)    CAP_IO="$2";    shift 2 ;;
        --n-actions) N_ACTIONS="$2"; shift 2 ;;
        --runs)      K_RUNS="$2";    shift 2 ;;
        *) echo "Usage: $0 [--n-agents N] [--n-dormant N] [--cap-io N] [--n-actions N] [--runs K]" >&2; exit 1 ;;
    esac
done

echo "[S12] Compilation s12-runner (release)..."
cd "$POC_DIR"
export CXXFLAGS="${CXXFLAGS:--include cstdint}"
cargo build --bin s12-runner --release --quiet
BIN="$POC_DIR/target/release/s12-runner"
echo "  OK : $BIN"

WORK_DIR="$SCRIPT_DIR/work"
mkdir -p "$WORK_DIR"

passed=0

for run in $(seq 1 "$K_RUNS"); do
    run_dir="$WORK_DIR/run${run}"
    mkdir -p "$run_dir"
    report="$run_dir/report.json"
    db_dir="$run_dir/db"

    stdout=$("$BIN" \
        --n-agents  "$N_AGENTS" \
        --n-dormant "$N_DORMANT" \
        --cap-io    "$CAP_IO" \
        --n-actions "$N_ACTIONS" \
        --out-report "$report" \
        --db-root "$db_dir" \
        2>"$run_dir/runner.err" || true)
    echo "$stdout" > "$run_dir/runner.out"

    verdict=$(echo "$stdout" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('verdict','?'))" 2>/dev/null || echo "?")

    if [[ "$verdict" == "pass" ]]; then
        passed=$((passed + 1))
        echo "  [S12 run${run}] pass"
    else
        echo "  [S12 run${run}] FAIL"
        cat "$run_dir/runner.err" >&2 || true
        cat "$run_dir/runner.out" >&2 || true
    fi
done

# Rapport consolidé
REPORT="$SCRIPT_DIR/report.json"
TS=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
python3 - <<EOF > "$REPORT"
import json, os

runs_data = []
for i in range(1, ${K_RUNS}+1):
    p = "${WORK_DIR}/run{}/report.json".format(i)
    if os.path.exists(p):
        with open(p) as f:
            runs_data.append(json.load(f))

print(json.dumps({
    "timestamp": "${TS}",
    "scenario": "S12-scheduler-coordinator",
    "adr": "ADR-0031",
    "n_agents":  ${N_AGENTS},
    "n_dormant": ${N_DORMANT},
    "cap_io":    ${CAP_IO},
    "k_runs":    ${K_RUNS},
    "passed":    ${passed},
    "total":     ${K_RUNS},
    "verdict":   "pass" if ${passed} == ${K_RUNS} else "fail",
    "runs":      runs_data,
}, indent=2))
EOF

echo ""
echo "[S12] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S12] Verdict global : ${passed}/${K_RUNS} pass"

if [[ ${passed} -ne ${K_RUNS} ]]; then
    exit 1
fi
