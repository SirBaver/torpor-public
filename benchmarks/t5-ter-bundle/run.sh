#!/usr/bin/env bash
# T5-ter — isolation p99 vs compaction RocksDB (ADR-0032 §D4)
#
# Mode A : disable_auto_compactions + compact_all avant mesure → P3b-intrinsèque.
#   Critère PASS : p99 stable run-à-run dans bande ±20% sur K=3 runs.
#
# Mode B : config normale + poll num-running-compactions à chaque cycle.
#   Critère CONFIRMED : ≥ 80% des spikes p99 > 5ms cooccurrent avec une compaction.
#
# Usage :
#   SKIP_INSTALL=1 bash run.sh [a|b] [N]
#   N = nombre d'entrées à précharger (défaut 100_000_000 = 10⁸)
#
# Variables d'environnement :
#   T5TER_MODE     : "a" ou "b" (défaut "a")
#   T5TER_N        : N entrées (défaut 100_000_000)
#   T5TER_K        : nombre de runs (défaut 3)
#   SKIP_INSTALL   : si "1", pas d'install apt/dnf
#   BENCH_DB_BASE  : répertoire de base pour les DBs (défaut dans results/)

set -euo pipefail
export LC_ALL=C

MODE="${1:-${T5TER_MODE:-a}}"
BENCH_N="${2:-${T5TER_N:-100000000}}"
K="${T5TER_K:-3}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TS="$(date -u +'%Y-%m-%dT%H%M%SZ')"
RESULTS_DIR="$REPO_ROOT/results/T5-ter/$MODE/$TS"
mkdir -p "$RESULTS_DIR"

log()  { printf '[%s] %s\n' "$(date -u +'%H:%M:%SZ')" "$*" >&2; }
die()  { log "ERREUR : $*"; exit 1; }

log "T5-ter Mode ${MODE^^} — démarrage. K=$K N=$BENCH_N"
log "Résultats : $RESULTS_DIR"

# Vérification fstype (DB doit être sur NVMe, pas tmpfs)
BENCH_DB_BASE="${BENCH_DB_BASE:-$RESULTS_DIR}"
BENCH_FSTYPE="$(df -T "$REPO_ROOT" | tail -n1 | awk '{print $2}')"
if [[ "$BENCH_FSTYPE" == "tmpfs" ]]; then
    die "Repo sur tmpfs — mesure P3b invalide. Pointer BENCH_DB_BASE sur un FS persistant."
fi
log "fstype=$BENCH_FSTYPE — OK"

# Install dépendances
if [[ "${SKIP_INSTALL:-0}" != "1" ]]; then
    if command -v apt-get >/dev/null 2>&1; then
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential clang libclang-dev pkg-config
    fi
fi
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
command -v cargo >/dev/null 2>&1 || die "cargo introuvable"

# Build release
log "Build release..."
CXXFLAGS="-include cstdint" cargo build -p os-poc-benchmarks --release \
    --manifest-path "$REPO_ROOT/poc/Cargo.toml" 2>/dev/null
BENCH_BIN="$REPO_ROOT/poc/target/release/os-poc-benchmarks"
[[ -f "$BENCH_BIN" ]] || die "Binaire introuvable : $BENCH_BIN"
log "Build OK"

# K runs
declare -a P99_VALUES=()

for run in $(seq 1 "$K"); do
    log "=== Run $run/$K Mode ${MODE^^} ==="

    # Drop caches entre runs
    sync
    if echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null 2>&1; then
        log "drop_caches=3 appliqué"
    else
        log "AVERTISSEMENT : drop_caches échoué (sudo requis)"
    fi

    RUN_DIR="$RESULTS_DIR/run$run"
    mkdir -p "$RUN_DIR"

    # Lancer le bench — la DB est créée dans results/T5-ter/<mode>/<ts>/
    # On force la DB dans run_dir pour isoler les runs
    STDOUT="$RUN_DIR/stdout.log"
    STDERR="$RUN_DIR/stderr.log"

    CXXFLAGS="-include cstdint" \
        "$BENCH_BIN" t5-ter "$MODE" "$BENCH_N" \
        > "$STDOUT" 2> "$STDERR" || true

    # Le binaire crée son propre répertoire results/T5-ter/<mode>/<ts2>/
    # On récupère le chemin depuis stdout
    BENCH_OUT_DIR=$(grep "Résultats :" "$STDOUT" | awk '{print $NF}' | head -1)
    if [[ -n "$BENCH_OUT_DIR" ]] && [[ -f "$BENCH_OUT_DIR/verdict.json" ]]; then
        cp "$BENCH_OUT_DIR/verdict.json" "$RUN_DIR/verdict.json"
        cp "$BENCH_OUT_DIR/events.jsonl" "$RUN_DIR/events.jsonl" 2>/dev/null || true
        # Supprimer la DB (21 GB) — le verdict.json est la seule sortie utile.
        rm -rf "$BENCH_OUT_DIR/db"
        log "Run $run : verdict copié, DB supprimée ($BENCH_OUT_DIR/db)"
    else
        log "AVERTISSEMENT : verdict.json introuvable pour run $run"
        cp "$STDOUT" "$RUN_DIR/stdout.log"
        continue
    fi

    # Extraire p99
    P99=$(python3 -c "
import json, sys
with open('$RUN_DIR/verdict.json') as f:
    d = json.load(f)
print(d.get('p99_us', 0))
" 2>/dev/null || echo 0)
    P99_VALUES+=("$P99")
    log "Run $run : p99=$P99 µs"

    # Pause thermique entre runs (Mode B inclus)
    if [[ "$run" -lt "$K" ]]; then
        log "Pause 30s entre runs..."
        sleep 30
    fi
done

# ── Analyse K runs ──────────────────────────────────────────────────────────
log "=== Analyse K=$K runs ==="

P99_CSV=$(IFS=,; echo "${P99_VALUES[*]:-0}")

python3 - <<PYEOF
import json, os, math

mode = "$MODE"
k    = $K
out  = "$RESULTS_DIR"

p99s = [int(x) for x in "$P99_CSV".split(",") if x]
print(f"p99 par run : {p99s} µs")

if len(p99s) < 2:
    print("INSUFFICIENT_DATA")
    exit(0)

p99_min = min(p99s)
p99_max = max(p99s)
p99_med = sorted(p99s)[len(p99s)//2]

# Mode A : critère ±20% autour de la médiane
if mode == "a":
    band_lo = p99_med * 0.80
    band_hi = p99_med * 1.20
    in_band = all(band_lo <= v <= band_hi for v in p99s)
    verdict = "PASS" if in_band else "FAIL"
    note = f"band=[{band_lo:.0f},{band_hi:.0f}]µs median={p99_med}µs"
    print(f"P3b-intrinsèque : p99_médiane={p99_med}µs  bande±20%: {note}")
    print(f"Verdict Mode A : {verdict}")
else:
    # Mode B : agréger les taux de corrélation
    correlations = []
    for run in range(1, k+1):
        vf = os.path.join(out, f"run{run}", "verdict.json")
        if os.path.exists(vf):
            with open(vf) as f:
                d = json.load(f)
            correlations.append(d.get("correlation_pct", 0.0))
    if correlations:
        avg_corr = sum(correlations) / len(correlations)
        verdict = "CONFIRMED" if avg_corr >= 80.0 else "UNCONFIRMED"
        print(f"Corrélation moyenne : {avg_corr:.1f}% ({correlations})")
        print(f"Verdict Mode B : {verdict}")
    else:
        verdict = "INSUFFICIENT_DATA"

# Synthèse JSON
summary = {
    "mode": mode,
    "k_runs": k,
    "n_entries": $BENCH_N,
    "p99_per_run_us": p99s,
    "p99_min_us": p99_min,
    "p99_max_us": p99_max,
    "p99_median_us": p99_med,
    "verdict": verdict,
}
with open(os.path.join(out, "summary.json"), "w") as f:
    json.dump(summary, f, indent=2)
print(f"\nSummary : {os.path.join(out, 'summary.json')}")
PYEOF

log "T5-ter Mode ${MODE^^} terminé. Résultats : $RESULTS_DIR"
