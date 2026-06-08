#!/usr/bin/env bash
# run.sh — bundle one-shot pour le benchmark T5-bis (P3b end-to-end : append_durable+get).
#
# T5-bis mesure le cycle complet qu'un agent en production paie avant de pouvoir relire
# une action qu'il vient d'émettre : `append_durable()` (WAL fsync forcé) → `get(action_id)`.
# C'est P3b (spec/02-properties.md), distinct de P3a (T5 — lookup point seul).
#
# Différences avec t5-bundle/run.sh :
#   - cible : `causal_end_to_end` au lieu de `causal_lookup`
#   - parse `T5BIS_METRICS:` au lieu de `T5_METRICS:`
#   - workload.json.test = "T5-bis", target_property = "P3b", p99_ms_max_target = 20
#   - hardware_probe.sh et software_probe.sh sont réutilisés depuis t5-bundle/
#
# Pourquoi un harness séparé plutôt qu'un flag dans t5-bundle/run.sh :
#   - Les deux benchmarks ne mesurent pas la même propriété (P3a vs P3b).
#   - Les bornes diffèrent (10 ms vs 20 ms).
#   - Le profil d'écriture diffère (DB statique vs writes pendant la mesure → throttling
#     NVMe potentiellement différent). Documenter et exécuter séparément est plus honnête.
#
# Variables d'environnement modulables :
#   BENCH_N        : taille de la population initiale (défaut 100000000 = 10⁸).
#   T5BIS_BENCH_DIR : forcer le répertoire de la DB.
#   SKIP_INSTALL   : si "1", n'essaie pas d'installer apt/dnf.
#   MIN_RAM_GB     : seuil RAM dur en dessous duquel on abort (défaut 8).

set -euo pipefail
export LC_ALL=C

# --- Constantes ---------------------------------------------------------------
BENCH_N="${BENCH_N:-100000000}"
MIN_RAM_GB="${MIN_RAM_GB:-8}"
WARN_RAM_GB=16

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
T5_BUNDLE_DIR="$REPO_ROOT/benchmarks/t5-bundle"
TS="$(date -u +'%Y-%m-%dT%H%M%SZ')"
RESULTS_DIR="$REPO_ROOT/results/T5-bis/$TS"
RAW_DIR="$RESULTS_DIR/raw"
ARCHIVE="$REPO_ROOT/t5bis-results-$TS.tar.gz"

IOSTAT_PID=""

# --- Helpers ------------------------------------------------------------------
log()  { printf '[%s] %s\n' "$(date -u +'%H:%M:%SZ')" "$*" >&2; }
die()  { log "ERREUR : $*"; exit 1; }

cleanup() {
    local rc=$?
    if [[ -n "${IOSTAT_PID:-}" ]] && kill -0 "$IOSTAT_PID" 2>/dev/null; then
        log "Arrêt iostat (pid=$IOSTAT_PID)..."
        kill "$IOSTAT_PID" 2>/dev/null || true
        wait "$IOSTAT_PID" 2>/dev/null || true
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM

# --- Étape 0 : sanity checks --------------------------------------------------
log "T5-bis bundle — démarrage. TS=$TS"
log "Repo : $REPO_ROOT"
log "Résultats : $RESULTS_DIR"
mkdir -p "$RAW_DIR"

if [[ ! -f "$REPO_ROOT/poc/Cargo.toml" ]]; then
    die "Repo introuvable : $REPO_ROOT/poc/Cargo.toml absent. Lancer depuis la racine du clone."
fi
if [[ ! -d "$T5_BUNDLE_DIR" ]]; then
    die "t5-bundle introuvable : $T5_BUNDLE_DIR (probes hardware/software réutilisées)."
fi

RAM_KB=$(awk '/^MemTotal:/ {print $2; exit}' /proc/meminfo)
RAM_GB=$(( (RAM_KB + 524288) / 1048576 ))
log "RAM détectée : ${RAM_GB} GB"
if (( RAM_GB < MIN_RAM_GB )); then
    die "RAM ${RAM_GB} GB < ${MIN_RAM_GB} GB (seuil dur). Le bench T5-bis N=10⁸ va swapper. Utiliser une instance plus grosse."
fi
if (( RAM_GB < WARN_RAM_GB )); then
    log "AVERTISSEMENT : RAM ${RAM_GB} GB < ${WARN_RAM_GB} GB recommandé."
fi

# --- Étape 1 : install dépendances système ------------------------------------
# Réutilise exactement la même liste que t5-bundle.
PKGS_NEEDED=(build-essential clang libclang-dev pkg-config fio sysstat curl ca-certificates git)
PKGS_NEEDED_RPM=(gcc gcc-c++ make clang clang-devel pkgconf-pkg-config fio sysstat curl ca-certificates git)

install_pkgs() {
    if [[ "${SKIP_INSTALL:-0}" == "1" ]]; then
        log "SKIP_INSTALL=1 — pas d'install paquets."
        return
    fi
    if command -v apt-get >/dev/null 2>&1; then
        log "Install paquets (apt) : ${PKGS_NEEDED[*]}"
        sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${PKGS_NEEDED[@]}"
    elif command -v dnf >/dev/null 2>&1; then
        log "Install paquets (dnf) : ${PKGS_NEEDED_RPM[*]}"
        sudo dnf install -y -q "${PKGS_NEEDED_RPM[@]}"
    elif command -v yum >/dev/null 2>&1; then
        log "Install paquets (yum) : ${PKGS_NEEDED_RPM[*]}"
        sudo yum install -y -q "${PKGS_NEEDED_RPM[@]}"
    else
        die "Aucun package manager reconnu. Installer manuellement : ${PKGS_NEEDED[*]}"
    fi
}
install_pkgs

if ! command -v rustc >/dev/null 2>&1; then
    log "rustc absent — install rustup."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
fi
# shellcheck disable=SC1090
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
command -v cargo >/dev/null 2>&1 || die "cargo introuvable après install rustup."
log "rustc : $(rustc --version)"

# --- Étape 2 : détection NVMe ------------------------------------------------
# Stratégie identique à t5-bundle : NVMe instance store AWS prioritaire, sinon /tmp
# (mais /tmp peut être tmpfs → fsync no-op : avertir l'utilisateur).
BENCH_DIR=""
NVME_NOTE=""

if [[ -n "${T5BIS_BENCH_DIR:-}" ]]; then
    BENCH_DIR="$T5BIS_BENCH_DIR"
    NVME_NOTE="T5BIS_BENCH_DIR fourni explicitement : $BENCH_DIR"
    mkdir -p "$BENCH_DIR"
    log "$NVME_NOTE"
else
    INSTANCE_NVME=""
    while read -r name model; do
        if [[ "$model" == *"Instance Storage"* ]] || [[ "$model" == *"EC2 NVMe"* ]]; then
            INSTANCE_NVME="/dev/$name"
            break
        fi
    done < <(lsblk -dn -o NAME,MODEL 2>/dev/null || true)

    if [[ -n "$INSTANCE_NVME" ]]; then
        log "NVMe instance store détecté : $INSTANCE_NVME"
        MOUNT_POINT="/mnt/nvme-bench"
        EXISTING_MOUNT=$(lsblk -no MOUNTPOINT "$INSTANCE_NVME" 2>/dev/null | head -n1 | tr -d ' ')
        if [[ -n "${EXISTING_MOUNT:-}" ]]; then
            log "Déjà monté sur $EXISTING_MOUNT — réutilisation."
            BENCH_DIR="$EXISTING_MOUNT/t5bis-bench"
            sudo mkdir -p "$BENCH_DIR"
            sudo chown "$(id -u):$(id -g)" "$BENCH_DIR"
            NVME_NOTE="NVMe instance store réutilisé : $INSTANCE_NVME -> $EXISTING_MOUNT"
        else
            FS_EXISTING=$(sudo blkid -o value -s TYPE "$INSTANCE_NVME" 2>/dev/null || true)
            if [[ -z "${FS_EXISTING:-}" ]]; then
                log "Formatage xfs de $INSTANCE_NVME (vide)."
                if command -v mkfs.xfs >/dev/null 2>&1; then
                    sudo mkfs.xfs -f -q "$INSTANCE_NVME"
                else
                    sudo mkfs.ext4 -q -F "$INSTANCE_NVME"
                fi
            else
                log "$INSTANCE_NVME a déjà un FS ($FS_EXISTING) — pas de reformat."
            fi
            sudo mkdir -p "$MOUNT_POINT"
            sudo mount "$INSTANCE_NVME" "$MOUNT_POINT"
            sudo chown "$(id -u):$(id -g)" "$MOUNT_POINT"
            BENCH_DIR="$MOUNT_POINT/t5bis-bench"
            mkdir -p "$BENCH_DIR"
            NVME_NOTE="NVMe instance store monté : $INSTANCE_NVME -> $MOUNT_POINT"
        fi
    else
        BENCH_DIR="/tmp/t5bis-bench"
        mkdir -p "$BENCH_DIR"
        NVME_NOTE="Aucun NVMe instance store détecté — fallback /tmp. ATTENTION : si /tmp est tmpfs, fsync est no-op et la mesure P3b n'est pas valide. Forcer T5BIS_BENCH_DIR sur un FS persistant."
        log "$NVME_NOTE"
    fi
fi

# Détection critique : si BENCH_DIR est sur tmpfs, fsync est un no-op.
BENCH_FSTYPE="$(df -T "$BENCH_DIR" | tail -n1 | awk '{print $2}')"
if [[ "$BENCH_FSTYPE" == "tmpfs" ]]; then
    die "BENCH_DIR=$BENCH_DIR est sur tmpfs — fsync no-op, mesure P3b invalide. Forcer T5BIS_BENCH_DIR sur un FS persistant (ext4/xfs/btrfs)."
fi
log "BENCH_DIR = $BENCH_DIR (fstype=$BENCH_FSTYPE)"
log "Espace libre BENCH_DIR : $(df -h "$BENCH_DIR" | tail -n1 | awk '{print $4}')"

# --- Étape 3 : capture snapshots hardware/software ----------------------------
# Réutilise les probes de t5-bundle (factorisation : c'est exactement la même mesure
# du hardware sous-jacent).
log "Capture hardware.json (réutilise probe t5-bundle, fio interne, ~10 s)..."
bash "$T5_BUNDLE_DIR/hardware_probe.sh" "$BENCH_DIR" "$RESULTS_DIR/hardware.json"

log "Capture software.json..."
bash "$T5_BUNDLE_DIR/software_probe.sh" "$REPO_ROOT" "$RESULTS_DIR/software.json"

# --- Étape 4 : lancer iostat en background ------------------------------------
IOSTAT_OUT="$RAW_DIR/iostat.txt"
log "Démarrage iostat (intervalle 5 s) → $IOSTAT_OUT"
( iostat -xt -y 5 > "$IOSTAT_OUT" 2>&1 ) &
IOSTAT_PID=$!
log "iostat pid=$IOSTAT_PID"
sleep 2

# --- Étape 5 : drop_caches ----------------------------------------------------
log "Vidage du page cache OS (sync + drop_caches=3)..."
sync
if echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null 2>&1; then
    log "drop_caches=3 appliqué."
    DROP_CACHES_APPLIED="true"
else
    log "AVERTISSEMENT : drop_caches échoué (sudo requis)."
    DROP_CACHES_APPLIED="false"
fi

# --- Étape 6 : lancer le bench T5-bis -----------------------------------------
BENCH_LOG="$RAW_DIR/bench_stdout.log"
BENCH_ERR="$RAW_DIR/bench_stderr.log"
RUN_STARTED_UNIX_MS=$(( $(date -u +%s) * 1000 ))
log "Lancement T5-bis (BENCH_N=$BENCH_N, BENCH_DIR=$BENCH_DIR)..."
log "Logs : $BENCH_LOG (stdout) + $BENCH_ERR (stderr)"

BENCH_RC=0
CXXFLAGS="-include cstdint" BENCH_N="$BENCH_N" BENCH_DIR="$BENCH_DIR" \
    cargo bench --bench causal_end_to_end --manifest-path "$REPO_ROOT/poc/Cargo.toml" \
    -p os-poc-causal-log \
    > "$BENCH_LOG" 2> "$BENCH_ERR" || BENCH_RC=$?

RUN_ENDED_UNIX_MS=$(( $(date -u +%s) * 1000 ))
log "Bench terminé (rc=$BENCH_RC) en $((RUN_ENDED_UNIX_MS - RUN_STARTED_UNIX_MS)) ms (wall)."

# --- Étape 7 : stopper iostat -------------------------------------------------
if [[ -n "$IOSTAT_PID" ]] && kill -0 "$IOSTAT_PID" 2>/dev/null; then
    kill "$IOSTAT_PID" 2>/dev/null || true
    wait "$IOSTAT_PID" 2>/dev/null || true
    log "iostat arrêté."
fi
IOSTAT_PID=""

# --- Étape 8 : parser iostat → cpu_steal, io_wait ---------------------------
CLOUD_CSV="$RAW_DIR/cloud_io.csv"
{
    echo "sample,cpu_user,cpu_nice,cpu_system,cpu_iowait,cpu_steal,cpu_idle"
    awk '
        /^avg-cpu:/ { take=1; next }
        take==1 && NF >= 6 {
            print sample","$1","$2","$3","$4","$5","$6
            sample++
            take=0
        }
        BEGIN { sample=0 }
    ' "$IOSTAT_OUT"
} > "$CLOUD_CSV"

CPU_STEAL_MAX="0"
IO_WAIT_MAX="0"
if [[ -s "$CLOUD_CSV" ]]; then
    IO_WAIT_MAX=$(awk -F, 'NR>1 {if ($5+0 > m) m=$5+0} END {if (m=="") print 0; else printf "%.2f", m}' "$CLOUD_CSV")
    CPU_STEAL_MAX=$(awk -F, 'NR>1 {if ($6+0 > m) m=$6+0} END {if (m=="") print 0; else printf "%.2f", m}' "$CLOUD_CSV")
fi
log "cpu_steal_max_pct=$CPU_STEAL_MAX  io_wait_max_pct=$IO_WAIT_MAX"

# --- Étape 9 : extraire métriques T5-bis depuis le stdout ---------------------
METRICS_LINE=$(grep -m1 '^T5BIS_METRICS:' "$BENCH_LOG" 2>/dev/null || true)
if [[ -z "${METRICS_LINE:-}" ]]; then
    METRICS_LINE=$(grep -m1 '^T5BIS_METRICS:' "$BENCH_ERR" 2>/dev/null || true)
fi
METRICS_JSON=""
if [[ -n "${METRICS_LINE:-}" ]]; then
    METRICS_JSON="${METRICS_LINE#T5BIS_METRICS: }"
fi

extract_num() {
    local key="$1"
    local default="$2"
    if [[ -z "$METRICS_JSON" ]]; then echo "$default"; return; fi
    printf '%s' "$METRICS_JSON" | awk -v k="\"$key\":" '
        BEGIN { v = "" }
        {
            n = index($0, k)
            if (n > 0) {
                rest = substr($0, n + length(k))
                sub(/[,}].*$/, "", rest)
                gsub(/[ \t]/, "", rest)
                v = rest
            }
            print v
            exit
        }
    '
}

P50=$(extract_num p50_us "null")
P95=$(extract_num p95_us "null")
P99=$(extract_num p99_us "null")
P99_9=$(extract_num p99_9_us "null")
PASS_BOOL=$(extract_num pass "false")
[[ -z "$P50" ]] && P50="null"
[[ -z "$P95" ]] && P95="null"
[[ -z "$P99" ]] && P99="null"
[[ -z "$P99_9" ]] && P99_9="null"
[[ -z "$PASS_BOOL" ]] && PASS_BOOL="false"

if [[ "$BENCH_RC" -ne 0 ]]; then
    OUTCOME="inconclusive"
    OUTCOME_NOTE="cargo bench rc=$BENCH_RC ; voir raw/bench_stderr.log"
elif [[ -z "$METRICS_JSON" ]]; then
    OUTCOME="inconclusive"
    OUTCOME_NOTE="Ligne T5BIS_METRICS absente du stdout."
elif [[ "$PASS_BOOL" == "true" ]]; then
    OUTCOME="pass"
    OUTCOME_NOTE="p99=${P99}µs ≤ 20000µs (P3b conforme)"
else
    OUTCOME="fail"
    OUTCOME_NOTE="p99=${P99}µs > 20000µs — P3b dégradée, borne à amender ou hardware sous-dimensionné"
fi

# --- Étape 10 : assembler workload.json ---------------------------------------
if [[ "$DROP_CACHES_APPLIED" == "true" ]]; then
    DATASET_SIZE_GB=15
    if (( RAM_GB * 2 >= DATASET_SIZE_GB )); then
        CACHE_REGIME="cache-mixte"
        CACHE_NOTE="drop_caches appliqué, ratio RAM(${RAM_GB}GB)/dataset(${DATASET_SIZE_GB}GB) = $(awk -v r=$RAM_GB -v d=$DATASET_SIZE_GB 'BEGIN{printf "%.2f", r/d}')× — régime cache-mixte contraint (ADR-0026)"
    else
        CACHE_REGIME="cache-miss-dominant"
        CACHE_NOTE="drop_caches appliqué, RAM << dataset"
    fi
else
    CACHE_REGIME="cache-mixte"
    CACHE_NOTE="drop_caches non appliqué"
fi

cat > "$RESULTS_DIR/workload.json" <<EOF
{
  "test": "T5-bis",
  "target_property": "P3b",
  "bench_n": $BENCH_N,
  "n_measures": 10000,
  "cache_regime": "$CACHE_REGIME",
  "cache_regime_note": "$CACHE_NOTE",
  "drop_caches_applied": $DROP_CACHES_APPLIED,
  "p99_ms_max_target": 20,
  "access_pattern": "uniform",
  "access_pattern_note": "T5-bis génère des entrées distinctes par cycle (agent_id varié, ts_ms incrémental) ; le get retourne sur l'action que l'on vient d'écrire (cache memtable côté lecture, isole le coût fsync côté écriture). Modèle d'accès Q2 = uniform (Modèle A) puisque la séquence d'agent_id est uniformément distribuée. Pour un test recency (Modèle B) avec relectures différées, un bench séparé sera nécessaire.",
  "bench_dir": "$BENCH_DIR",
  "bench_dir_note": "$NVME_NOTE",
  "bench_dir_fstype": "$BENCH_FSTYPE",
  "run_started_unix_ms": $RUN_STARTED_UNIX_MS,
  "run_ended_unix_ms": $RUN_ENDED_UNIX_MS,
  "wall_duration_ms": $((RUN_ENDED_UNIX_MS - RUN_STARTED_UNIX_MS)),
  "emit_payload_size_distribution": null
}
EOF
log "workload.json écrit."

# --- Étape 11 : verdict.json --------------------------------------------------
VERDICT_NOTES="$OUTCOME_NOTE — single run, single instance, classification indicatif (protocole §5). $NVME_NOTE"
VERDICT_NOTES_ESCAPED=$(printf '%s' "$VERDICT_NOTES" | sed 's/\\/\\\\/g; s/"/\\"/g')

cat > "$RESULTS_DIR/verdict.json" <<EOF
{
  "test": "T5-bis",
  "target_property": "P3b",
  "outcome": "$OUTCOME",
  "classification": "indicatif",
  "metrics": {
    "p50_us": $P50,
    "p95_us": $P95,
    "p99_us": $P99,
    "p99_9_us": $P99_9
  },
  "thermal": {
    "status": "cloud-vm-no-thermal",
    "cpu_steal_max_pct": $CPU_STEAL_MAX,
    "io_wait_max_pct": $IO_WAIT_MAX,
    "note": "Sur AWS, températures non exposées. Sur bare-metal, exécuter benchmarks/thermal-capture.sh en parallèle."
  },
  "notes": "$VERDICT_NOTES_ESCAPED"
}
EOF
log "verdict.json écrit."

# --- Étape 12 : summary.md ---------------------------------------------------
cat > "$RESULTS_DIR/summary.md" <<EOF
# T5-bis — P3b end-to-end (append_durable + get) — run $TS

## Résultat
- **Outcome :** $OUTCOME
- **Classification :** indicatif (1 instance, 1 run — protocole §5)
- **Propriété mesurée :** P3b (cf. \`spec/02-properties.md §P3b\`)
- **Note :** $OUTCOME_NOTE

## Métriques (10 000 cycles append_durable + get)
- p50  : ${P50} µs
- p95  : ${P95} µs
- **p99  : ${P99} µs** (cible P3b ≤ 20 000 µs)
- p99.9 : ${P99_9} µs

## Conditions
- **BENCH_N (population initiale) :** $BENCH_N
- **Cache régime :** $CACHE_REGIME — $CACHE_NOTE
- **drop_caches :** $DROP_CACHES_APPLIED
- **BENCH_DIR :** \`$BENCH_DIR\`
- **Filesystem :** $BENCH_FSTYPE
- **NVMe note :** $NVME_NOTE
- **RAM instance :** ${RAM_GB} GB
- **Wall duration :** $((RUN_ENDED_UNIX_MS - RUN_STARTED_UNIX_MS)) ms

## Sémantique de mesure
Chaque cycle exécute :
1. \`append_durable(entry)\` — WAL fsync forcé via \`WriteOptions::set_sync(true)\`
2. \`get(action_id)\` — lookup point sur l'action qu'on vient d'écrire

Le timer démarre avant (1) et s'arrête après (2). Cette sémantique mesure « le temps qu'un agent paie avant de pouvoir relire une action qu'il vient d'émettre » (spec P3b).

## Cloud thermal proxy (§8.10)
- cpu_steal_max_pct : $CPU_STEAL_MAX
- io_wait_max_pct   : $IO_WAIT_MAX

## Fichiers
- \`hardware.json\` — CPU, RAM, storage (fio mesuré), GPU, métadonnées
- \`software.json\` — OS, kernel, rustc, rocksdb_version, git_commit, source_tree_sha256
- \`workload.json\` — paramètres T5-bis
- \`verdict.json\` — outcome + métriques + thermal proxy
- \`raw/iostat.txt\` — capture brute iostat (5 s)
- \`raw/cloud_io.csv\` — CSV parsé (cpu_steal, iowait)
- \`raw/bench_stdout.log\` / \`raw/bench_stderr.log\`
EOF
log "summary.md écrit."

# --- Étape 13 : archive -------------------------------------------------------
log "Compression résultats → $ARCHIVE"
tar -czf "$ARCHIVE" -C "$REPO_ROOT/results/T5-bis" "$TS"
ARCHIVE_SIZE_KB=$(( $(stat -c %s "$ARCHIVE") / 1024 ))
log "Archive : $ARCHIVE (${ARCHIVE_SIZE_KB} KB)"

# --- Étape 14 : résumé final --------------------------------------------------
cat <<EOF

════════════════════════════════════════════════════════════════════════
 T5-bis BUNDLE — RÉSUMÉ FINAL
════════════════════════════════════════════════════════════════════════
 Propriété cible      : P3b (append_durable + get)
 Outcome              : $OUTCOME
 Classification       : indicatif (protocole §5)
 p99 mesuré           : ${P99} µs (cible ≤ 20 000 µs)
 BENCH_N              : $BENCH_N
 BENCH_DIR            : $BENCH_DIR ($BENCH_FSTYPE)
 Archive résultats    : $ARCHIVE
 Taille archive       : ${ARCHIVE_SIZE_KB} KB

 Pour K=3 runs nécessaires à "partiellement validé" sur une classe, relancer
 ce script trois fois et agréger les résultats dans results/T5-bis/SYNTHESE.md.

════════════════════════════════════════════════════════════════════════
EOF

printf 'T5BIS_THERMAL: p50_us=%s p95_us=%s p99_us=%s p99_9_us=%s outcome=%s\n' \
    "$P50" "$P95" "$P99" "$P99_9" "$OUTCOME"

exit 0
