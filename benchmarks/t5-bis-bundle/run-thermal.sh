#!/usr/bin/env bash
# run-thermal.sh — T5-bis-thermal : dissociation causale p50/p95 vs p99
#
# Hypothèse falsifiable (TODO.md §T5-bis-thermal) :
#   Si la dégradation p99 observée dans T5-bis est causalement thermique :
#     Phase A : Spearman(rank(p99), rank(T_max)) > 0.7     (corrélation positive)
#     Phase B : pente régression p99 ~ run_index ≈ 0, |b/se_b| < 1  (pas de progression)
#   Prédiction : p50/p95 stables dans les deux phases.
#
# Capteurs (lecture sysfs sans root) :
#   NVMe Composite : /sys/class/hwmon/hwmon3/temp1_input  (millidegrés)
#   CPU k10temp    : /sys/class/hwmon/hwmon4/temp1_input  (millidegrés)
#
# Phases :
#   A : 3 runs consécutifs sans pause (reproduit la progression thermique)
#   B : 3 runs avec pause entre chaque (T_nvme ≤ T_init+5°C stable 30 s, timeout 10 min)
#
# Sorties : results/T5-bis-thermal/<TS>/thermal.jsonl + verdict.json + summary.md
#
# Variables :
#   BENCH_N         : population initiale causal-log (défaut 100 000 000)
#   COOL_TARGET_DEG : delta au-dessus de T_init pour fin pause phase B (défaut 5)
#   STABLE_S        : secondes stables requises pour fin pause phase B (défaut 30)
#   COOL_TIMEOUT_S  : timeout max cool-down phase B en secondes (défaut 600)
#   SKIP_PHASE_A    : si "1", saute phase A (debug)
#   SKIP_PHASE_B    : si "1", saute phase B (debug)

set -euo pipefail
export LC_ALL=C

# ── Constantes ─────────────────────────────────────────────────────────────────
BENCH_N="${BENCH_N:-100000000}"
COOL_TARGET_DEG="${COOL_TARGET_DEG:-5}"     # °C au-dessus de T_init
STABLE_S="${STABLE_S:-30}"                  # s stables requis
COOL_TIMEOUT_S="${COOL_TIMEOUT_S:-600}"     # timeout cool-down

NVME_HWMON="/sys/class/hwmon/hwmon3/temp1_input"
CPU_HWMON="/sys/class/hwmon/hwmon4/temp1_input"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TS="$(date -u +'%Y-%m-%dT%H%M%SZ')"
OUT_DIR="$REPO_ROOT/results/T5-bis-thermal/$TS"
THERMAL_JSONL="$OUT_DIR/thermal.jsonl"

mkdir -p "$OUT_DIR"

log()  { printf '[%s] %s\n' "$(date -u +'%H:%M:%SZ')" "$*" >&2; }
die()  { log "ERREUR : $*"; exit 1; }

# ── Lecture capteurs ───────────────────────────────────────────────────────────
read_nvme_mc()  { cat "$NVME_HWMON" 2>/dev/null || echo 0; }
read_cpu_mc()   { cat "$CPU_HWMON"  2>/dev/null || echo 0; }
mc_to_c()       { awk "BEGIN { printf \"%.1f\", $1 / 1000 }"; }

# ── Monitor thermique (boucle background) ─────────────────────────────────────
# Lance un sous-shell en background qui échantillonne les températures toutes les 1 s
# et écrit dans THERMAL_JSONL jusqu'à ce que le fichier sentinel $1 existe.
start_thermal_monitor() {
    local sentinel="$1" phase="$2" run_idx="$3"
    local t0
    t0=$(date +%s)
    (
        while [[ ! -f "$sentinel" ]]; do
            local ts elapsed T_nvme T_cpu
            ts=$(date +%s)
            elapsed=$(( ts - t0 ))
            T_nvme=$(read_nvme_mc)
            T_cpu=$(read_cpu_mc)
            printf '{"ts":%d,"elapsed_s":%d,"phase":"%s","run_idx":%d,"T_nvme_mc":%d,"T_cpu_mc":%d}\n' \
                "$ts" "$elapsed" "$phase" "$run_idx" "$T_nvme" "$T_cpu" \
                >> "$THERMAL_JSONL"
            sleep 1
        done
    ) >/dev/null 2>/dev/null &
    echo $!
}

# ── Cool-down (phase B) ────────────────────────────────────────────────────────
# Attend que T_nvme ≤ T_target_mc millidegrés pendant STABLE_S secondes consécutives.
# Retourne 0 si stabilisé, 1 si timeout.
wait_cool_down() {
    local T_target_mc="$1"
    local stable=0 elapsed=0
    local t0; t0=$(date +%s)
    log "  Cool-down : attente T_nvme ≤ $(mc_to_c $T_target_mc)°C stable ${STABLE_S}s (timeout ${COOL_TIMEOUT_S}s)"
    while true; do
        local T_now
        T_now=$(read_nvme_mc)
        elapsed=$(( $(date +%s) - t0 ))
        if (( T_now <= T_target_mc )); then
            (( stable++ ))
            printf '\r  t=%ds  T_nvme=%s°C  stable=%ds/%ds     ' \
                "$elapsed" "$(mc_to_c $T_now)" "$stable" "$STABLE_S" >&2
        else
            stable=0
            printf '\r  t=%ds  T_nvme=%s°C  refroidissement en cours...     ' \
                "$elapsed" "$(mc_to_c $T_now)" >&2
        fi
        if (( stable >= STABLE_S )); then
            echo ""
            log "  Cool-down terminé en ${elapsed}s (T=$(mc_to_c $T_now)°C)"
            return 0
        fi
        if (( elapsed >= COOL_TIMEOUT_S )); then
            echo ""
            log "  WARN: cool-down timeout après ${COOL_TIMEOUT_S}s — continue avec T=$(mc_to_c $T_now)°C"
            return 1
        fi
        sleep 1
    done
}

# ── Exécution d'un run T5-bis ─────────────────────────────────────────────────
# Retourne p99_us via echo sur stdout (les logs vont dans un fichier séparé).
run_one() {
    local phase="$1" run_idx="$2"
    local run_log="$OUT_DIR/${phase}_run${run_idx}.log"

    log "  Démarrage run ${phase}${run_idx} (BENCH_N=$BENCH_N)..."

    # Température de départ
    local T_start_nvme T_start_cpu
    T_start_nvme=$(read_nvme_mc)
    T_start_cpu=$(read_cpu_mc)
    log "  T_init  : NVMe=$(mc_to_c $T_start_nvme)°C  CPU=$(mc_to_c $T_start_cpu)°C"

    # Monitor thermique en background
    local sentinel; sentinel=$(mktemp)
    rm -f "$sentinel"  # le sentinel n'existe pas encore → le monitor tourne
    local monitor_pid
    monitor_pid=$(start_thermal_monitor "$sentinel" "$phase" "$run_idx")

    # Lancer run.sh (bloquant).
    # BENCH_DIR partagé : le fichier fio (.t5_fio_probe.dat, 4 GB) n'est créé qu'une fois.
    # Chaque run crée son propre sous-répertoire t5bis-causal-<ts> à l'intérieur.
    local SHARED_BENCH_DIR="$REPO_ROOT/poc/results/T5-bis-bench/thermal-shared"
    mkdir -p "$SHARED_BENCH_DIR"
    local run_exit=0
    CXXFLAGS="-include cstdint" BENCH_N="$BENCH_N" SKIP_INSTALL=1 \
        T5BIS_BENCH_DIR="$SHARED_BENCH_DIR" \
        bash "$SCRIPT_DIR/run.sh" > "$run_log" 2>&1 || run_exit=$?

    # Arrêter le monitor
    touch "$sentinel"
    wait "$monitor_pid" 2>/dev/null || true

    # Libérer l'espace disque : supprimer les sous-répertoires t5bis-causal-* créés par ce run.
    # Le fichier .t5_fio_probe.dat est conservé pour éviter de le recréer au prochain run.
    find "$SHARED_BENCH_DIR" -maxdepth 1 -name 't5bis-causal-*' -type d \
        -exec rm -rf {} + 2>/dev/null || true

    # Température de fin
    local T_end_nvme T_end_cpu
    T_end_nvme=$(read_nvme_mc)
    T_end_cpu=$(read_cpu_mc)

    # T_max NVMe pendant le run
    local T_max_nvme
    T_max_nvme=$(grep "\"phase\":\"${phase}\",\"run_idx\":${run_idx}" "$THERMAL_JSONL" 2>/dev/null \
        | awk -F'"T_nvme_mc":' '{print $2}' | awk -F',' '{print $1}' | sort -n | tail -1)
    T_max_nvme="${T_max_nvme:-$T_end_nvme}"

    log "  T_end   : NVMe=$(mc_to_c $T_end_nvme)°C  CPU=$(mc_to_c $T_end_cpu)°C"
    log "  T_max   : NVMe=$(mc_to_c $T_max_nvme)°C  (run)"

    if [[ $run_exit -ne 0 ]]; then
        log "  WARN: run.sh exit=$run_exit — p99 non récupéré, run skipped"
        echo "0 0 0 0 $T_start_nvme $T_max_nvme"
        return
    fi

    # Parser les métriques depuis la ligne T5BIS_THERMAL: écrite par run.sh
    local p50 p95 p99 p99_9
    local thermal_line
    thermal_line=$(grep '^T5BIS_THERMAL:' "$run_log" | tail -1 || true)
    p50=$(printf '%s' "$thermal_line"   | grep -o 'p50_us=[0-9]*'   | cut -d= -f2 || echo 0)
    p95=$(printf '%s' "$thermal_line"   | grep -o 'p95_us=[0-9]*'   | cut -d= -f2 || echo 0)
    p99=$(printf '%s' "$thermal_line"   | grep -o ' p99_us=[0-9]*'  | cut -d= -f2 || echo 0)
    p99_9=$(printf '%s' "$thermal_line" | grep -o 'p99_9_us=[0-9]*' | cut -d= -f2 || echo 0)
    [[ -z "$p50"   ]] && p50=0
    [[ -z "$p95"   ]] && p95=0
    [[ -z "$p99"   ]] && p99=0
    [[ -z "$p99_9" ]] && p99_9=0

    log "  p50=${p50}µs  p95=${p95}µs  p99=${p99}µs  p99.9=${p99_9}µs"
    echo "$p50 $p95 $p99 $p99_9 $T_start_nvme $T_max_nvme"
}

# ── Statistiques finales (python3) ─────────────────────────────────────────────
compute_verdict() {
    python3 - <<PYEOF
import sys, math, json

# Données phase A et B passées via variables d'environnement
import os

def parse_runs(env_key, n=3):
    raw = os.environ.get(env_key, "")
    rows = [r.split() for r in raw.strip().split("|") if r.strip()]
    return rows

phase_a = parse_runs("PHASE_A_DATA")
phase_b = parse_runs("PHASE_B_DATA")

def to_f(s):
    try: return float(s)
    except: return 0.0

def spearman(xs, ys):
    n = len(xs)
    def rank(v):
        sv = sorted(v)
        return [sv.index(x) + 1 for x in v]
    rx, ry = rank(xs), rank(ys)
    d2 = sum((a-b)**2 for a, b in zip(rx, ry))
    return 1 - 6*d2 / (n*(n**2-1)) if n > 1 else 0.0

def lin_reg(xs, ys):
    n = len(xs)
    if n < 3: return 0.0, float('inf')
    xm, ym = sum(xs)/n, sum(ys)/n
    sxx = sum((x-xm)**2 for x in xs)
    sxy = sum((x-xm)*(y-ym) for x, y in zip(xs, ys))
    b = sxy/sxx if sxx > 1e-10 else 0.0
    sse = sum((y - (b*(x-xm) + ym))**2 for x, y in zip(xs, ys))
    se_b = math.sqrt(sse/(n-2)/sxx) if (n > 2 and sxx > 1e-10) else float('inf')
    return b, se_b

# --- Phase A ---
pa_p50  = [to_f(r[0]) for r in phase_a if len(r) >= 6]
pa_p95  = [to_f(r[1]) for r in phase_a if len(r) >= 6]
pa_p99  = [to_f(r[2]) for r in phase_a if len(r) >= 6]
pa_Tmax = [to_f(r[5]) / 1000 for r in phase_a if len(r) >= 6]  # °C
idx_a   = list(range(1, len(pa_p99)+1))

spearman_p99_T  = spearman(pa_p99, pa_Tmax)   if len(pa_p99) >= 2 else 0.0
spearman_p50_T  = spearman(pa_p50, pa_Tmax)   if len(pa_p50) >= 2 else 0.0
spearman_p95_T  = spearman(pa_p95, pa_Tmax)   if len(pa_p95) >= 2 else 0.0

# --- Phase B ---
pb_p50  = [to_f(r[0]) for r in phase_b if len(r) >= 6]
pb_p99  = [to_f(r[2]) for r in phase_b if len(r) >= 6]
idx_b   = list(range(1, len(pb_p99)+1))

b_p99, se_p99 = lin_reg(idx_b, pb_p99)
b_p50, se_p50 = lin_reg(idx_b, pb_p50)

t_p99 = abs(b_p99 / se_p99) if se_p99 > 1e-10 else float('inf')
t_p50 = abs(b_p50 / se_p50) if se_p50 > 1e-10 else float('inf')

# --- Verdict ---
crit_A = spearman_p99_T > 0.7
crit_B = t_p99 < 1.0
stab_A = spearman_p50_T < 0.5 and spearman_p95_T < 0.5  # p50/p95 non corrélés
stab_B = t_p50 < 1.0

verdict = "CONFIRMED" if crit_A and crit_B else \
          "PARTIAL"   if crit_A or crit_B else \
          "REFUTED"

result = {
    "hypothesis": "T5-bis-thermal: p99 dégradation causalement thermique",
    "phase_a": {
        "p99_us": pa_p99,
        "p50_us": pa_p50,
        "T_max_C": pa_Tmax,
        "spearman_p99_vs_T": round(spearman_p99_T, 3),
        "spearman_p50_vs_T": round(spearman_p50_T, 3),
        "spearman_p95_vs_T": round(spearman_p95_T, 3),
        "criterion_A": crit_A,
        "p50_p95_stable": stab_A,
    },
    "phase_b": {
        "p99_us": pb_p99,
        "p50_us": pb_p50,
        "slope_p99": round(b_p99, 2),
        "se_b_p99": round(se_p99, 2),
        "t_stat_p99": round(t_p99, 2),
        "t_stat_p50": round(t_p50, 2),
        "criterion_B": crit_B,
        "p50_stable_B": stab_B,
    },
    "verdict": verdict,
    "interpretation": (
        "Dissociation thermique confirmée : p99 corrèle avec T_max (A) et disparaît avec cooling (B)."
        if verdict == "CONFIRMED" else
        "Partiel : un seul critère satisfait — cause thermique probable mais non concluante."
        if verdict == "PARTIAL" else
        "Hypothèse thermique réfutée — chercher autre cause (write-stall RocksDB, GC, tail allocator)."
    ),
}
print(json.dumps(result, indent=2))
PYEOF
}

# ══════════════════════════════════════════════════════════════════════════════
#  Main
# ══════════════════════════════════════════════════════════════════════════════

log "T5-bis-thermal démarrage. TS=$TS"
log "Capteurs : NVMe=$(mc_to_c $(read_nvme_mc))°C  CPU=$(mc_to_c $(read_cpu_mc))°C"
log "Sorties   : $OUT_DIR"
log "BENCH_N   : $BENCH_N"
echo ""

T_INIT_MC=$(read_nvme_mc)
T_COOL_TARGET_MC=$(( T_INIT_MC + COOL_TARGET_DEG * 1000 ))

log "T_init NVMe : $(mc_to_c $T_INIT_MC)°C — T_cible cool-down : $(mc_to_c $T_COOL_TARGET_MC)°C"

PHASE_A_DATA=""
PHASE_B_DATA=""

# ── Phase A : 3 runs consécutifs sans pause ───────────────────────────────────
if [[ "${SKIP_PHASE_A:-0}" != "1" ]]; then
    echo ""
    log "╔═══════════════════════════════════════════╗"
    log "║  PHASE A — 3 runs consécutifs sans pause  ║"
    log "╚═══════════════════════════════════════════╝"
    for i in 1 2 3; do
        log "── Run A${i} ──────────────────────────────────"
        ROW=$(run_one "A" "$i")
        if [[ -n "$PHASE_A_DATA" ]]; then PHASE_A_DATA="${PHASE_A_DATA}|"; fi
        PHASE_A_DATA="${PHASE_A_DATA}${ROW}"
        log "A${i} : $ROW"
    done
fi

# ── Phase B : 3 runs avec cool-down entre chaque ─────────────────────────────
if [[ "${SKIP_PHASE_B:-0}" != "1" ]]; then
    echo ""
    log "╔════════════════════════════════════════════════════╗"
    log "║  PHASE B — 3 runs avec pause thermique entre chaque ║"
    log "╚════════════════════════════════════════════════════╝"
    for i in 1 2 3; do
        log "── Cool-down avant B${i} ──────────────────────────────"
        wait_cool_down "$T_COOL_TARGET_MC" || true
        log "── Run B${i} ───────────────────────────────────────────"
        ROW=$(run_one "B" "$i")
        if [[ -n "$PHASE_B_DATA" ]]; then PHASE_B_DATA="${PHASE_B_DATA}|"; fi
        PHASE_B_DATA="${PHASE_B_DATA}${ROW}"
        log "B${i} : $ROW"
    done
fi

# ── Verdict ───────────────────────────────────────────────────────────────────
echo ""
log "══ Calcul verdict ══"

VERDICT_JSON=$(PHASE_A_DATA="$PHASE_A_DATA" PHASE_B_DATA="$PHASE_B_DATA" compute_verdict)
echo "$VERDICT_JSON" > "$OUT_DIR/verdict.json"
log "verdict.json écrit."

# Summary markdown
SPEARMAN=$(echo "$VERDICT_JSON" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['phase_a']['spearman_p99_vs_T'])")
T_STAT=$(echo "$VERDICT_JSON" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['phase_b']['t_stat_p99'])")
VERDICT=$(echo "$VERDICT_JSON" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['verdict'])")
INTERP=$(echo "$VERDICT_JSON" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['interpretation'])")

cat > "$OUT_DIR/summary.md" <<EOF
# T5-bis-thermal — Dissociation causale p99 — $TS

## Verdict : $VERDICT

$INTERP

## Critères
- **Phase A** : Spearman(rank(p99), rank(T_max)) = $SPEARMAN (seuil > 0.7) → $([ "$(echo "$SPEARMAN > 0.7" | awk '{print ($1 > 0.7)}')" = "1" ] && echo "✓" || echo "✗")
- **Phase B** : |b/se_b| (p99) = $T_STAT (seuil < 1.0) → $(awk "BEGIN {print ($T_STAT < 1.0) ? \"✓\" : \"✗\"}")

## Données brutes

### Phase A — runs consécutifs
$(echo "$PHASE_A_DATA" | tr '|' '\n' | awk 'NF>=6 {printf "- Run A%d : p50=%s µs | p95=%s µs | p99=%s µs | T_max=%.1f°C\n", NR, $1, $2, $3, $6/1000}')

### Phase B — runs avec pause thermique
$(echo "$PHASE_B_DATA" | tr '|' '\n' | awk 'NF>=6 {printf "- Run B%d : p50=%s µs | p95=%s µs | p99=%s µs | T_max=%.1f°C\n", NR, $1, $2, $3, $6/1000}')

## Fichiers
- \`thermal.jsonl\` — échantillons thermiques (1 par seconde, toutes phases)
- \`verdict.json\` — statistiques complètes (Spearman, régression, verdict)
- \`A<n>.log\` / \`B<n>.log\` — sorties des runs T5-bis individuels
EOF

log "summary.md écrit."

echo ""
log "════════════════════════════════════════════════════════════════"
log " T5-bis-thermal — RÉSUMÉ FINAL"
log "════════════════════════════════════════════════════════════════"
log " Verdict       : $VERDICT"
log " Spearman A    : $SPEARMAN (seuil > 0.7)"
log " |b/se_b| B    : $T_STAT (seuil < 1.0)"
log " Résultats     : $OUT_DIR"
log "════════════════════════════════════════════════════════════════"

echo ""
echo "$VERDICT_JSON"
