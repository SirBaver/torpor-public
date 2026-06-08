#!/usr/bin/env bash
# S14 — Traçabilité causale — lookup causal (SEF-5 / P3a).
#
# Protocole :
#   1. Population unique : populate_synthetic(N=10⁸) + sauvegarde des ids
#      échantillonnés dans work/samples.json.
#   2. K=3 passes de mesure indépendantes sur la même DB (cache OS variable
#      entre les passes → représentatif du régime production).
#   Chaque passe vérifie :
#     P-α  p99 ≤ 10 ms pour log.get(action_id) sur 10 000 lookups.
#     P-β  1 000 entrées vérifiées — contenu bit-à-bit conforme au ground truth
#          (agent_id, hash_before, hash_after, emit_payload, action_id() == clé).
#
# Note : la DB partagée (~10–15 GB sur NVMe) est créée une seule fois pour
# éviter les effets thermiques cumulés d'une repopulation à chaque passe.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
if [ ! -d "$POC_DIR/runtime" ]; then
    POC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
fi
WORK_DIR="$SCRIPT_DIR/work"
REPORT="$SCRIPT_DIR/report.json"

export CXXFLAGS="${CXXFLAGS:--include cstdint}"

N_ENTRIES="${N_ENTRIES:-100000000}"
N_SAMPLES="${N_SAMPLES:-1000}"
N_READS="${N_READS:-10000}"
K_RUNS="${K_RUNS:-3}"

cd "$POC_DIR"

# --- Étape 1 : compilation (release) ----------------------------------------
echo "[S14] Compilation sef5-runner (release)..."
if ! cargo build --release -p os-poc-runtime --bin sef5-runner 2>&1 | tail -3; then
    echo "[S14] FATAL : compilation échouée"
    exit 2
fi

RUNNER="$POC_DIR/target/release/sef5-runner"

# --- Étape 2 : population unique --------------------------------------------
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"

DB_DIR="$WORK_DIR/db"
SAMPLES_FILE="$WORK_DIR/samples.json"

echo "[S14] Population de $N_ENTRIES entrées (DB unique, K=$K_RUNS passes de mesure)..."
"$RUNNER" \
    --db-dir      "$DB_DIR" \
    --n-entries   "$N_ENTRIES" \
    --n-samples   "$N_SAMPLES" \
    --n-reads     "$N_READS" \
    --save-samples "$SAMPLES_FILE" \
    --out-report  "$WORK_DIR/run0-populate.json" \
    >"$WORK_DIR/run0.out" 2>"$WORK_DIR/run0.err"
pop_exit=$?

if [ "$pop_exit" -ne 0 ]; then
    echo "[S14] FATAL : population échouée (exit=$pop_exit)"
    cat "$WORK_DIR/run0.err"
    exit 2
fi
echo "[S14] Population OK"

# --- Étape 3 : K passes de mesure -------------------------------------------
PASSED=0
TOTAL=0
DETAILS=()

for ((r = 1; r <= K_RUNS; r++)); do
    TOTAL=$((TOTAL + 1))

    "$RUNNER" \
        --db-dir       "$DB_DIR" \
        --load-samples "$SAMPLES_FILE" \
        --n-samples    "$N_SAMPLES" \
        --n-reads      "$N_READS" \
        --out-report   "$WORK_DIR/run${r}.json" \
        >"$WORK_DIR/run${r}.out" 2>"$WORK_DIR/run${r}.err"
    exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        PASSED=$((PASSED + 1))
        p99=$(grep '"p99_us"' "$WORK_DIR/run${r}.json" | grep -oE ':\s*[0-9]+' | grep -oE '[0-9]+')
        echo "  [S14 run${r}] pass  (p99=${p99} µs)"
        DETAILS+=("$r:pass:${p99}µs")
    else
        echo "  [S14 run${r}] FAIL — exit=$exit_code"
        tail -20 "$WORK_DIR/run${r}.out" | sed 's/^/    /'
        DETAILS+=("$r:fail")
    fi
done

# --- Étape 4 : rapport JSON -------------------------------------------------
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
VERDICT="fail"
[ "$PASSED" -eq "$TOTAL" ] && VERDICT="pass"

{
    echo "{"
    echo "  \"timestamp\": \"$TIMESTAMP\","
    echo "  \"scenario\": \"S14-causal-lookup\","
    echo "  \"property\": \"P3a\","
    echo "  \"sef\": \"SEF-5\","
    echo "  \"n_entries\": $N_ENTRIES,"
    echo "  \"n_samples\": $N_SAMPLES,"
    echo "  \"n_reads\": $N_READS,"
    echo "  \"k_runs\": $K_RUNS,"
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"p99_target_us\": 10000,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S14] Rapport : $REPORT"
cat "$REPORT"
echo ""
echo "[S14] Verdict global : $PASSED/$TOTAL pass"

[ "$VERDICT" = "pass" ]
