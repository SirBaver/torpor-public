#!/usr/bin/env bash
# S9 — Capability isolation test harness (SEF-3 / ADR-0029).
#
# Exécute le test Rust s9_capability_isolation et écrit le résultat dans report.json.
#
# Protocole :
#   1 parent + 10 sous-agents, chacun avec une cap exclusive sur Ri.
#   - Étape 2 : chaque sous-agent écrit dans Ri → doit réussir (P4-a).
#   - Étape 3 : chaque sous-agent tente de lire R_{i+1} → doit échouer (P4-b).
#   - Étape 3b : chaque sous-agent tente de lire RP → doit échouer (P4-b).
#   - Étape 4 : chaque refus doit avoir un CapabilityDenied (0x14) dans le log (P4-c).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POC_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
REPORT="$SCRIPT_DIR/report.json"

export CXXFLAGS="${CXXFLAGS:--include cstdint}"

cd "$POC_DIR"

echo "[S9] Exécution du test s9_capability_isolation..."
OUTPUT=$(cargo test -p os-poc-runtime --release -- tests::s9_capability_isolation --nocapture 2>&1)
EXIT_CODE=$?

echo "$OUTPUT"

TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

if [ $EXIT_CODE -eq 0 ]; then
    VERDICT="pass"
    PASSED=1
    TOTAL=1
    echo "[S9] Test PASS"
else
    VERDICT="fail"
    PASSED=0
    TOTAL=1
    echo "[S9] Test FAIL (exit=$EXIT_CODE)"
fi

{
    echo "{"
    echo "  \"timestamp\": \"$TIMESTAMP\","
    echo "  \"scenario\": \"S9-capability-isolation\","
    echo "  \"n_children\": 10,"
    echo "  \"criteria\": ["
    echo "    \"P4-a: 100% authorized accesses succeed\","
    echo "    \"P4-b: 100% unauthorized accesses fail\","
    echo "    \"P4-c: 100% denials logged as CapabilityDenied (0x14)\""
    echo "  ],"
    echo "  \"adr\": \"ADR-0029\","
    echo "  \"passed\": $PASSED,"
    echo "  \"total\": $TOTAL,"
    echo "  \"verdict\": \"$VERDICT\""
    echo "}"
} > "$REPORT"

echo ""
echo "[S9] Rapport : $REPORT"
cat "$REPORT"

[ "$VERDICT" = "pass" ]
