#!/usr/bin/env bash
# S15-crash-machine-concurrent — P6 valid-prefix sous crash machine concurrent.
#
# Régime : process::exit(1) + sync + drop_caches (cache froid).
# Propriété : P6 atomicité crash (R1 — actif indépendamment de l'inférence).
# Substrat  : Linux. Non transférable seL4 (D7 / ADR-0050).
#
# Garde-fou L32 : sync + drop_caches obligatoire.
#   - sync    : flush des dirty pages vers le disque (sinon durabilité fantôme).
#   - drop_caches : vide le page cache → réouverture depuis le disque froid.
#   Un kill sans drop_caches ne teste pas plus que S6 (page-cache intact).
#
# USAGE :
#   sudo bash scenarios/S15-crash-machine-concurrent/run.sh \
#     [N_AGENTS] [COMMITS_PER_AGENT] [KILL_THRESHOLD] [K_RUNS]
#
# PRÉREQUIS :
#   root ou sudo sans mot de passe (pour drop_caches)
#   CXXFLAGS="-include cstdint" (GCC récent)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN_DIR="$POC_DIR/target/release"

N_AGENTS="${1:-4}"
COMMITS_PER_AGENT="${2:-25}"
KILL_THRESHOLD="${3:-40}"
K_RUNS="${4:-5}"
BLOCK_SIZE=64

REPORT_FILE="$SCRIPT_DIR/report.json"

echo "[S15] crash-machine-concurrent : N_AGENTS=$N_AGENTS COMMITS_PER_AGENT=$COMMITS_PER_AGENT KILL_THRESHOLD=$KILL_THRESHOLD K_RUNS=$K_RUNS"
echo "[S15] Substrat : Linux. Régime : R1 (P6). Garde-fou L32 actif (sync+drop_caches)."

# Vérifier les binaires.
for bin in s15-writer s15-verifier; do
    if [[ ! -x "$BIN_DIR/$bin" ]]; then
        echo "[S15] ERREUR : $BIN_DIR/$bin introuvable."
        echo "  Compiler avec :"
        echo "    cd $POC_DIR && CXXFLAGS=\"-include cstdint\" cargo build --release -p os-poc-runtime --bin s15-writer --bin s15-verifier"
        exit 1
    fi
done

# Vérifier l'accès à drop_caches.
if [[ $EUID -ne 0 ]] && ! sudo -n true 2>/dev/null; then
    echo "[S15] ERREUR : drop_caches requiert root ou sudo sans mot de passe."
    echo "  Relancer avec sudo : sudo bash $0 $*"
    exit 1
fi

drop_caches() {
    sync
    if [[ $EUID -eq 0 ]]; then
        echo 3 > /proc/sys/vm/drop_caches
    else
        sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
    fi
}

PASSED=0
FAILED=0
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

for run in $(seq 1 "$K_RUNS"); do
    echo ""
    echo "[S15] Run $run/$K_RUNS"

    TS="$(date +%s%3N)"
    WORK="$SCRIPT_DIR/work/$TS"
    mkdir -p "$WORK"

    STORE="$WORK/store"
    LOG="$WORK/log"
    WITNESS="$WORK/witness.json"
    REPORT_RUN="$WORK/report.json"

    # Phase 1 : écriture concurrente + kill.
    echo "[S15]   Phase 1 : writer (${N_AGENTS} agents concurrents, kill_threshold=${KILL_THRESHOLD})"
    "$BIN_DIR/s15-writer" \
        --db-store  "$STORE" \
        --db-log    "$LOG"   \
        --witness   "$WITNESS" \
        --n-agents  "$N_AGENTS" \
        --commits-per-agent "$COMMITS_PER_AGENT" \
        --kill-threshold    "$KILL_THRESHOLD" \
        --block-size        "$BLOCK_SIZE" || true   # exit(1) attendu

    if [[ ! -f "$WITNESS" ]]; then
        echo "[S15]   ERREUR : témoin absent ($WITNESS)"
        FAILED=$((FAILED + 1))
        continue
    fi

    # Phase 2 : sync + drop_caches (garde-fou L32).
    echo "[S15]   Phase 2 : sync + drop_caches"
    drop_caches
    echo "[S15]   drop_caches OK"

    # Phase 3 : vérification oracle P6 valid-prefix.
    echo "[S15]   Phase 3 : verifier (disque froid)"
    if "$BIN_DIR/s15-verifier" \
            --db-store   "$STORE" \
            --db-log     "$LOG"   \
            --witness    "$WITNESS" \
            --out-report "$REPORT_RUN"; then
        echo "[S15]   Run $run/$K_RUNS : PASS"
        PASSED=$((PASSED + 1))
    else
        echo "[S15]   Run $run/$K_RUNS : FAIL — violation P6 detectee"
        echo "[S15]   Rapport : $REPORT_RUN"
        FAILED=$((FAILED + 1))
    fi
done

TOTAL=$((PASSED + FAILED))
VERDICT="fail"
[[ "$PASSED" -eq "$K_RUNS" ]] && VERDICT="pass"

echo ""
echo "[S15] Verdict global : $PASSED/$K_RUNS pass"
echo "[S15] Substrat : Linux. Régime : R1 (P6). Non transférable seL4 (D7)."

cat > "$REPORT_FILE" <<REPORTEOF
{
  "timestamp": "$TIMESTAMP",
  "harness": "S15-crash-machine-concurrent",
  "substrat": "Linux",
  "regime": "R1",
  "adr_ref": "ADR-0050 D4, ADR-0027 D3",
  "n_agents": $N_AGENTS,
  "commits_per_agent": $COMMITS_PER_AGENT,
  "kill_threshold": $KILL_THRESHOLD,
  "k_runs": $K_RUNS,
  "passed": $PASSED,
  "total": $TOTAL,
  "verdict": "$VERDICT"
}
REPORTEOF

echo "[S15] Rapport : $REPORT_FILE"

[[ "$VERDICT" = "pass" ]] && exit 0 || exit 1
