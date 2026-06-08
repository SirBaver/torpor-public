#!/usr/bin/env bash
# T6 Phase A — H-densité-hébergée : harness complet Wasmtime + Docker.
#
# Exécute bench_t6_phase_a (Rust) pour plusieurs valeurs de N, K fois chacune,
# puis t6-docker-python-baseline.sh sur les mêmes N.
# Produit results/T6/phase-a/<timestamp>/verdict.json.
#
# Usage :
#   ./benchmarks/t6-phase-a/run.sh [--n 100,500,1000] [--runs 3] [--skip-docker]
#
# Prérequis :
#   - Linux (lecture /proc/meminfo, /proc/self/status)
#   - Ollama non requis pour Phase A (pas d'inférence)
#   - Docker requis sauf --skip-docker
#   - CXXFLAGS="-include cstdint" si GCC ≥ 15 (GCC 15 workaround)

set -euo pipefail
export LC_ALL=C LANG=C  # force decimal '.' pour awk/printf

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
POC_DIR="$REPO_ROOT/poc"

# ── Paramètres par défaut ────────────────────────────────────────────────────
N_VALUES="100,500,1000"
K_RUNS=3
SKIP_DOCKER=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --n)       N_VALUES="$2"; shift 2 ;;
        --runs)    K_RUNS="$2";   shift 2 ;;
        --skip-docker) SKIP_DOCKER=1; shift ;;
        *) echo "Usage: $0 [--n 100,500,1000] [--runs 3] [--skip-docker]" >&2; exit 1 ;;
    esac
done

IFS=',' read -ra N_LIST <<< "$N_VALUES"

# ── Timestamp du run ─────────────────────────────────────────────────────────
TS=$(date -u +"%Y-%m-%dT%H%M%SZ")
OUT_DIR="$REPO_ROOT/results/T6/phase-a/$TS"
mkdir -p "$OUT_DIR"

echo "=== T6 Phase A — H-densité-hébergée ==="
echo "  N values : ${N_LIST[*]}"
echo "  K runs   : $K_RUNS"
echo "  Out dir  : $OUT_DIR"
echo ""

# ── Build release ────────────────────────────────────────────────────────────
echo "  Build cargo (release)..."
cd "$POC_DIR"
export CXXFLAGS="${CXXFLAGS:--include cstdint}"
cargo build -p os-poc-benchmarks --release --quiet
BIN="$POC_DIR/target/release/os-poc-benchmarks"
echo "  Build OK : $BIN"
echo ""

# ── Drop caches (root requis, ignoré si non disponible) ─────────────────────
drop_caches() {
    sync
    echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null 2>&1 \
        && echo "  drop_caches : OK" \
        || echo "  drop_caches : non disponible (pas root) — mesure en cache-mixte"
}

# ── Mesure Wasmtime ───────────────────────────────────────────────────────────
echo "  === Mesures Wasmtime ==="

# On revient à la racine du repo pour que results/ soit résolu correctement
cd "$REPO_ROOT"

declare -A WM_OVERHEAD  # overhead_per_agent_kb moyen par N
declare -A WM_RATIO     # ratio moyen par N

for N in "${N_LIST[@]}"; do
    echo ""
    echo "  N=$N : $K_RUNS runs"

    sum_overhead=0
    sum_ratio=0

    for run in $(seq 1 "$K_RUNS"); do
        drop_caches
        echo "    Run $run/$K_RUNS..."

        # Ligne compacte préfixée T6_JSON: pour extraction fiable
        output=$("$BIN" t6-phase-a "$N" 2>/dev/null)
        compact=$(echo "$output" | grep '^T6_JSON:' | sed 's/^T6_JSON://' || true)

        if [[ -z "$compact" ]]; then
            echo "    ERREUR : pas de ligne T6_JSON: dans la sortie. Sortie brute :"
            echo "$output" | head -20
            continue
        fi

        overhead=$(echo "$compact" | grep -oP '"overhead_per_agent_kb":\s*\K[0-9.]+' || echo "0")
        ratio=$(echo "$compact" | grep -oP '"ratio":\s*\K[0-9.]+' || echo "0")

        echo "    overhead/agent : ${overhead} KB  |  ratio : ${ratio}×"

        sum_overhead=$(awk "BEGIN {print $sum_overhead + $overhead}")
        sum_ratio=$(awk "BEGIN {print $sum_ratio + $ratio}")

        # Copier le JSON compact dans le répertoire de sortie
        json_file="$OUT_DIR/wasmtime_n${N}_run${run}.json"
        echo "$compact" > "$json_file"
    done

    WM_OVERHEAD[$N]=$(awk "BEGIN {printf \"%.1f\", $sum_overhead / $K_RUNS}")
    WM_RATIO[$N]=$(awk "BEGIN {printf \"%.1f\", $sum_ratio / $K_RUNS}")

    echo "  N=$N moyenne : overhead=${WM_OVERHEAD[$N]} KB/agent  ratio=${WM_RATIO[$N]}×"
done

# ── Mesure Docker baseline ───────────────────────────────────────────────────
declare -A DOCKER_OVERHEAD_HOST  # KB/container méthode A (hôte)

if [[ $SKIP_DOCKER -eq 0 ]]; then
    echo ""
    echo "  === Mesure Docker baseline (Python LLM agent) ==="

    DOCKER_SCRIPT="$REPO_ROOT/benchmarks/t6-docker-python-baseline.sh"
    if [[ ! -f "$DOCKER_SCRIPT" ]]; then
        echo "  AVERTISSEMENT : $DOCKER_SCRIPT introuvable — mesure Docker ignorée"
        SKIP_DOCKER=1
    elif ! command -v docker &>/dev/null || ! docker info &>/dev/null 2>&1; then
        echo "  AVERTISSEMENT : Docker non disponible — mesure Docker ignorée"
        SKIP_DOCKER=1
    fi
fi

if [[ $SKIP_DOCKER -eq 0 ]]; then
    for N in "${N_LIST[@]}"; do
        echo ""
        echo "  Docker N=$N..."
        drop_caches

        docker_out=$(bash "$DOCKER_SCRIPT" "$N" 2>/dev/null || true)

        # Extraire "Overhead/container : XXXX KB" (méthode A — hôte)
        host_overhead=$(echo "$docker_out" \
            | grep "Overhead/container" | head -1 \
            | grep -oP '[0-9]+(?= KB)' || echo "0")

        DOCKER_OVERHEAD_HOST[$N]="$host_overhead"
        echo "  Docker N=$N : overhead hôte = ${host_overhead} KB/container"

        echo "$docker_out" > "$OUT_DIR/docker_n${N}.txt"
    done
fi

# ── Rapport et verdict.json ──────────────────────────────────────────────────
echo ""
echo "  === Résumé T6 Phase A — H-densité-hébergée ==="
echo ""
echo "  N        overhead Wasmtime   ratio Wasmtime/Docker   verdict P1a"
echo "  ──────   ─────────────────   ──────────────────────   ───────────"

RAM_16GB_KB=$((16 * 1024 * 1024))
overall_pass=true
metrics_json=""

for N in "${N_LIST[@]}"; do
    wm_oh="${WM_OVERHEAD[$N]:-0}"
    wm_ratio="${WM_RATIO[$N]:-N/A}"
    docker_oh="${DOCKER_OVERHEAD_HOST[$N]:-N/A}"

    if [[ "$wm_ratio" != "N/A" ]] && awk "BEGIN {exit ($wm_ratio < 5) ? 0 : 1}"; then
        v_pass="FAIL"
        overall_pass=false
    else
        v_pass="PASS"
    fi

    printf "  %-8s %-20s %-24s %s\n" \
        "N=$N" "${wm_oh} KB/agent" "${wm_ratio}× (Docker: ${docker_oh} KB)" "$v_pass"

    metrics_json+="$(printf '{"n":%s,"overhead_per_agent_kb":%s,"ratio":%s,"docker_baseline_kb":"%s","verdict":"%s"}' \
        "$N" "$wm_oh" "$wm_ratio" "$docker_oh" "$v_pass"),"
done

echo ""
overall_verdict="pass"
if [[ "$overall_pass" == "false" ]]; then
    overall_verdict="fail"
fi

# Écrire verdict.json
cat > "$OUT_DIR/verdict.json" <<EOF
{
  "test": "T6-phase-a",
  "hypothesis": "H-densité-hébergée",
  "outcome": "$overall_verdict",
  "classification": "indicatif",
  "k_runs": $K_RUNS,
  "n_values": [$(printf '%s,' "${N_LIST[@]}" | sed 's/,$//')]},
  "metrics": [${metrics_json%,}],
  "target": "ratio >= 5x (P1a)",
  "docker_baseline": "Python 3.11 + deps LLM (L27/L28)",
  "note": "Agents dormants dans inbox.recv() — tâches Tokio actives. Classification indicatif : 1 hardware, K=$K_RUNS runs. Pour 'partiellement valide' : NVMe >= 1 GB/s + hardware.json + K>=3."
}
EOF

echo "  verdict.json → $OUT_DIR/verdict.json"
echo "  Verdict global : $overall_verdict"
echo ""
echo "  Note : classification 'indicatif' — 1 hardware."
echo "  Pour 'partiellement validé' : relancer sur NVMe qualifié + ajouter hardware.json."
