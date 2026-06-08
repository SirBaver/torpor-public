#!/usr/bin/env bash
# run-all.sh — exécute séquentiellement S1–S14 et produit scenarios/report.json.
#
# Format de sortie :
#   {
#     "timestamp": "...",
#     "verdicts": {
#       "S1-supervision-algorithmique": "pass",
#       "S2-self-rollback-incoherence": "pass",
#       "S3-inference-cap": "pass",
#       "S4-scheduler-rollback": "pass",
#       "S5-fairness-priority": "pass",
#       "S9-capability-isolation": "pass"
#     },
#     "summary": "6/6 passed"
#   }
#
# Exit code : 0 si les six scénarios passent, 1 sinon.
#
# Prérequis :
#   - Rust toolchain + cible wasm32-unknown-unknown (ADR-0020 D1)
#   - CXXFLAGS="-include cstdint" (GCC 15.x, librocksdb-sys 0.16.0)
#   - Pas de prérequis Ollama : tous les scénarios utilisent
#     FixedResponseBackend ou SleepyBackend (déterministes).
#
# Usage :
#   bash poc/scenarios/run-all.sh
#   cat poc/scenarios/report.json
#   echo "exit=$?"

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(dirname "$SCRIPT_DIR")"
REPORT="$SCRIPT_DIR/report.json"

export CXXFLAGS="${CXXFLAGS:--include cstdint}"

cd "$POC_DIR"

# --- Étape 1 : compilation des agents WASM (ADR-0020 D5) -------------------
echo "[run-all] Compilation des agents WASM (wasm32-unknown-unknown, release)..."
if ! cargo build \
        --target wasm32-unknown-unknown \
        -p agent-sdk \
        --examples \
        --release \
        2>&1 | tail -3; then
    echo "[run-all] FATAL : la compilation des agents WASM a échoué."
    exit 2
fi

# --- Étape 2 : exécution des quatre scénarios ------------------------------
# Chaque scénario correspond à un test d'intégration nommé sN_<slug> dans
# poc/runtime/src/lib.rs (cf. ADR-0021 §convention nommage).
run_scenario() {
    # IMPORTANT : tous les logs vont sur stderr ; seul "pass"/"fail" sort
    # sur stdout, parce que la fonction est appelée en substitution de
    # commande ($(...)).
    local filter="$1"
    local label="$2"
    local start_ms end_ms duration
    start_ms=$(date +%s%3N)
    # On capture le statut explicitement et on inspecte la sortie : un
    # `cargo test` avec --quiet retourne 0 même sur certains filtres vides,
    # donc on vérifie aussi qu'au moins un test a effectivement tourné.
    local out
    out=$(cargo test -p os-poc-runtime --release -- "$filter" --quiet --exact 2>&1)
    local code=$?
    end_ms=$(date +%s%3N)
    duration=$((end_ms - start_ms))
    if [ $code -eq 0 ] && echo "$out" | grep -qE "[1-9][0-9]* passed"; then
        echo "  [$label] pass (${duration} ms)" >&2
        printf "pass"
    else
        echo "  [$label] fail (${duration} ms)" >&2
        # En cas d'échec, on dump la sortie pour diagnostic.
        echo "----- sortie cargo test (extrait) -----" >&2
        echo "$out" | tail -20 >&2
        echo "---------------------------------------" >&2
        printf "fail"
    fi
}

echo "[run-all] Exécution des scénarios..."
S1=$(run_scenario "tests::s1_supervision_algorithmique" "S1-supervision-algorithmique")
S2=$(run_scenario "tests::s2_self_rollback_incoherence" "S2-self-rollback-incoherence")
S3=$(run_scenario "tests::s3_inference_cap"             "S3-inference-cap")
S4=$(run_scenario "tests::s4_scheduler_rollback"        "S4-scheduler-rollback")
S5=$(run_scenario "tests::s5_fairness_priority"         "S5-fairness-priority")
S9=$(run_scenario "tests::s9_capability_isolation"      "S9-capability-isolation")

# S10 est un binaire standalone (pipeline C2→C1, pas un test lib)
echo "  [S10-unified-scheduler] lancement s10-runner..." >&2
S10_REPORT="$SCRIPT_DIR/S10-unified-scheduler/report_run-all.json"
if cargo run --release -p os-poc-runtime --bin s10-runner -- --out-report "$S10_REPORT" >/dev/null 2>&1; then
    S10="pass"
    echo "  [S10-unified-scheduler] pass" >&2
else
    S10="fail"
    echo "  [S10-unified-scheduler] fail" >&2
fi

# S11 est un binaire standalone (cycle éviction/réveil, ADR-0030 §FutureWork)
echo "  [S11-evict-wake] lancement s11-runner..." >&2
S11_REPORT="$SCRIPT_DIR/S11-evict-wake/report_run-all.json"
if cargo run --release -p os-poc-runtime --bin s11-runner -- \
       --out-report "$S11_REPORT" >/dev/null 2>&1; then
    S11="pass"
    echo "  [S11-evict-wake] pass" >&2
else
    S11="fail"
    echo "  [S11-evict-wake] fail" >&2
fi

# S12 est un binaire standalone (SchedulerCoordinator, ADR-0031)
echo "  [S12-scheduler-coordinator] lancement s12-runner..." >&2
S12_REPORT="$SCRIPT_DIR/S12-scheduler-coordinator/report_run-all.json"
if cargo run --release -p os-poc-runtime --bin s12-runner -- \
       --out-report "$S12_REPORT" >/dev/null 2>&1; then
    S12="pass"
    echo "  [S12-scheduler-coordinator] pass" >&2
else
    S12="fail"
    echo "  [S12-scheduler-coordinator] fail" >&2
fi

# S13 est un binaire standalone (persistance état après redémarrage, SEF-1)
echo "  [S13-persistence-restart] lancement sef1-runner..." >&2
S13_REPORT="$SCRIPT_DIR/S13-persistence-restart/report_run-all.json"
if cargo run --release -p os-poc-runtime --bin sef1-runner -- \
       --db-store "$SCRIPT_DIR/S13-persistence-restart/work/run-all/store" \
       --db-log   "$SCRIPT_DIR/S13-persistence-restart/work/run-all/log" \
       --agent-id "00000000000000000000000000009999" \
       --n-actions 100 \
       --out-report "$S13_REPORT" >/dev/null 2>&1; then
    S13="pass"
    echo "  [S13-persistence-restart] pass" >&2
else
    S13="fail"
    echo "  [S13-persistence-restart] fail" >&2
fi

# S14 est un binaire standalone (traçabilité causale, SEF-5 / P3a).
# Population 10⁸ entrées + K=3 passes. LONG (~10–20 min). Skippé par défaut
# dans run-all (trop lent) — lancer manuellement via S14/run.sh.
if [ "${RUN_S14:-0}" = "1" ]; then
    echo "  [S14-causal-lookup] lancement sef5-runner (long)..." >&2
    if bash "$SCRIPT_DIR/S14-causal-lookup/run.sh" >/dev/null 2>&1; then
        S14="pass"
        echo "  [S14-causal-lookup] pass" >&2
    else
        S14="fail"
        echo "  [S14-causal-lookup] fail" >&2
    fi
else
    S14="skipped"
    echo "  [S14-causal-lookup] skipped (set RUN_S14=1 pour inclure)" >&2
fi

# S15 — Crash machine concurrent + cache invalidé (UC-17 / ADR-0050 D4).
# Requiert root (drop_caches). Skippé par défaut — lancer via S15/run.sh.
if [ "${RUN_S15:-0}" = "1" ]; then
    echo "  [S15-crash-machine-concurrent] lancement (requiert sudo)..." >&2
    S15_REPORT="$SCRIPT_DIR/S15-crash-machine-concurrent/report_run-all.json"
    if sudo bash "$SCRIPT_DIR/S15-crash-machine-concurrent/run.sh" >/dev/null 2>&1; then
        S15="pass"
        echo "  [S15-crash-machine-concurrent] pass" >&2
    else
        S15="fail"
        echo "  [S15-crash-machine-concurrent] fail" >&2
    fi
else
    S15="skipped"
    echo "  [S15-crash-machine-concurrent] skipped (set RUN_S15=1 + sudo pour inclure)" >&2
fi

# --- Étape 3 : rapport JSON -----------------------------------------------
PASS_COUNT=0
for v in "$S1" "$S2" "$S3" "$S4" "$S5" "$S9" "$S10" "$S11" "$S12" "$S13"; do
    [ "$v" = "pass" ] && PASS_COUNT=$((PASS_COUNT + 1))
done
# S14 et S15 comptés seulement si exécutés
[ "${S14:-skipped}" = "pass" ] && PASS_COUNT=$((PASS_COUNT + 1))
[ "${S15:-skipped}" = "pass" ] && PASS_COUNT=$((PASS_COUNT + 1))

TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
cat > "$REPORT" <<EOF
{
  "timestamp": "$TIMESTAMP",
  "verdicts": {
    "S1-supervision-algorithmique": "$S1",
    "S2-self-rollback-incoherence": "$S2",
    "S3-inference-cap": "$S3",
    "S4-scheduler-rollback": "$S4",
    "S5-fairness-priority": "$S5",
    "S9-capability-isolation": "$S9",
    "S10-unified-scheduler": "$S10",
    "S11-evict-wake": "$S11",
    "S12-scheduler-coordinator": "$S12",
    "S13-persistence-restart": "$S13",
    "S14-causal-lookup": "${S14:-skipped}",
    "S15-crash-machine-concurrent": "${S15:-skipped}"
  },
  "summary": "$PASS_COUNT/10 passed (S14 skipped si RUN_S14!=1, S15 skipped si RUN_S15!=1)"
}
EOF

echo ""
echo "[run-all] Rapport : $REPORT"
cat "$REPORT"
echo ""

if [ "$PASS_COUNT" -eq 10 ]; then
    exit 0
else
    exit 1
fi
