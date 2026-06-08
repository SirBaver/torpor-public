#!/usr/bin/env bash
# run.sh — bundle one-shot pour le benchmark T5 sur instance AWS (i3.xlarge / i3.2xlarge).
#
# Ce script est conçu pour être lancé depuis la racine du repo, sur une instance EC2
# fraîchement provisionnée (Ubuntu 22.04 LTS ou Amazon Linux 2023 recommandés).
#
# Étapes :
#   1. Vérifier le hardware minimal et installer les paquets système requis.
#   2. Installer Rust (rustup) si absent.
#   3. Détecter et monter le NVMe local éphémère AWS (si présent).
#   4. Lancer `iostat` en arrière-plan pour capturer cpu_steal et io_wait.
#   5. Produire hardware.json (avec fio en interne) et software.json.
#   6. Construire et lancer `cargo bench --bench causal_lookup` avec BENCH_DIR sur NVMe.
#   7. Stopper iostat, calculer les indicateurs cloud, assembler workload.json + verdict.json.
#   8. Compresser results/T5/<ts>/ en t5-results-<ts>.tar.gz et afficher la commande scp.
#
# Variables d'environnement modulables :
#   BENCH_N        : taille de la population (défaut 100000000 = 10⁸).
#   T5_BENCH_DIR   : forcer le répertoire de la DB (sinon auto-détection NVMe ou /tmp).
#   SKIP_INSTALL   : si "1", n'essaie pas d'installer apt/dnf (utile en réexécution).
#   MIN_RAM_GB     : seuil RAM dur en dessous duquel on abort (défaut 8).

set -euo pipefail
# Forcer locale C : sinon awk/printf en fr_FR.UTF-8 utilisent ',' comme séparateur
# décimal, ce qui casse le JSON ("2,00" au lieu de "2.00") et fait que awk parse
# "0.05" comme l'entier 0. Tous les calculs numériques shell doivent rester en C.
export LC_ALL=C

# --- Constantes ---------------------------------------------------------------
BENCH_N="${BENCH_N:-100000000}"
MIN_RAM_GB="${MIN_RAM_GB:-8}"
WARN_RAM_GB=16

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TS="$(date -u +'%Y-%m-%dT%H%M%SZ')"
RESULTS_DIR="$REPO_ROOT/results/T5/$TS"
RAW_DIR="$RESULTS_DIR/raw"
ARCHIVE="$REPO_ROOT/t5-results-$TS.tar.gz"

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
log "T5 bundle — démarrage. TS=$TS"
log "Repo : $REPO_ROOT"
log "Résultats : $RESULTS_DIR"
mkdir -p "$RAW_DIR"

if [[ ! -f "$REPO_ROOT/poc/Cargo.toml" ]]; then
    die "Repo introuvable : $REPO_ROOT/poc/Cargo.toml absent. Lancer depuis la racine du clone."
fi
if [[ ! -d "$REPO_ROOT/.git" ]]; then
    log "AVERTISSEMENT : $REPO_ROOT/.git absent — git_commit sera null dans software.json. Cloner le repo complet pour la traçabilité."
fi

RAM_KB=$(awk '/^MemTotal:/ {print $2; exit}' /proc/meminfo)
RAM_GB=$(( (RAM_KB + 524288) / 1048576 ))
log "RAM détectée : ${RAM_GB} GB"
if (( RAM_GB < MIN_RAM_GB )); then
    die "RAM ${RAM_GB} GB < ${MIN_RAM_GB} GB (seuil dur). Le bench T5 N=10⁸ va swapper. Utiliser une instance plus grosse."
fi
if (( RAM_GB < WARN_RAM_GB )); then
    log "AVERTISSEMENT : RAM ${RAM_GB} GB < ${WARN_RAM_GB} GB recommandé. Régime cache-miss exagéré ; verdict reste interprétable mais à documenter."
fi

# --- Étape 1 : install dépendances système ------------------------------------
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
        die "Aucun package manager reconnu (apt/dnf/yum). Installer manuellement : ${PKGS_NEEDED[*]}"
    fi
}
install_pkgs

# rustup si rustc absent
if ! command -v rustc >/dev/null 2>&1; then
    log "rustc absent — install rustup (minimal toolchain stable)."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
fi
# shellcheck disable=SC1090
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
command -v cargo >/dev/null 2>&1 || die "cargo introuvable après install rustup."
log "rustc : $(rustc --version)"

# --- Étape 2 : détecter et préparer le NVMe local AWS -------------------------
# Stratégie : un device NVMe dont le MODEL contient "EC2 NVMe Instance Storage" est le
# disque instance store. On le formate en xfs (rapide, prod-ready) et on le monte sur
# /mnt/nvme-bench si pas déjà monté ailleurs.

BENCH_DIR=""
NVME_NOTE=""

if [[ -n "${T5_BENCH_DIR:-}" ]]; then
    BENCH_DIR="$T5_BENCH_DIR"
    NVME_NOTE="T5_BENCH_DIR fourni explicitement : $BENCH_DIR"
    mkdir -p "$BENCH_DIR"
    log "$NVME_NOTE"
else
    # Chercher un device "Amazon EC2 NVMe Instance Storage"
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
        # Vérifier s'il est déjà monté quelque part
        EXISTING_MOUNT=$(lsblk -no MOUNTPOINT "$INSTANCE_NVME" 2>/dev/null | head -n1 | tr -d ' ')
        if [[ -n "${EXISTING_MOUNT:-}" ]]; then
            log "Déjà monté sur $EXISTING_MOUNT — réutilisation."
            BENCH_DIR="$EXISTING_MOUNT/t5-bench"
            sudo mkdir -p "$BENCH_DIR"
            sudo chown "$(id -u):$(id -g)" "$BENCH_DIR"
            NVME_NOTE="NVMe instance store réutilisé : $INSTANCE_NVME -> $EXISTING_MOUNT"
        else
            # Vérifier si le device a déjà un FS
            FS_EXISTING=$(sudo blkid -o value -s TYPE "$INSTANCE_NVME" 2>/dev/null || true)
            if [[ -z "${FS_EXISTING:-}" ]]; then
                log "Formatage xfs de $INSTANCE_NVME (vide)."
                # mkfs.xfs présent dans xfsprogs ; si absent on fallback ext4
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
            BENCH_DIR="$MOUNT_POINT/t5-bench"
            mkdir -p "$BENCH_DIR"
            NVME_NOTE="NVMe instance store monté : $INSTANCE_NVME -> $MOUNT_POINT"
        fi
    else
        BENCH_DIR="/tmp/t5-bench"
        mkdir -p "$BENCH_DIR"
        NVME_NOTE="Aucun NVMe instance store détecté — fallback /tmp (résultat à interpréter avec prudence : peut être sur EBS, pas NVMe local)."
        log "$NVME_NOTE"
    fi
fi

log "BENCH_DIR = $BENCH_DIR"
log "Espace libre BENCH_DIR : $(df -h "$BENCH_DIR" | tail -n1 | awk '{print $4}')"

# --- Étape 3 : capture snapshots hardware/software ----------------------------
log "Capture hardware.json (fio interne, ~10 s)..."
bash "$SCRIPT_DIR/hardware_probe.sh" "$BENCH_DIR" "$RESULTS_DIR/hardware.json"

log "Capture software.json..."
bash "$SCRIPT_DIR/software_probe.sh" "$REPO_ROOT" "$RESULTS_DIR/software.json"

# --- Étape 4 : lancer iostat en background ------------------------------------
# `iostat -xt -y 5` : 5 s d'intervalle, skip first sample (-y), timestamp (-t),
# extended stats (-x). On capture les colonnes CPU (avg-cpu) qui contient %steal
# et %iowait, et les colonnes disque (await, %util).

IOSTAT_OUT="$RAW_DIR/iostat.txt"
log "Démarrage iostat (intervalle 5 s) → $IOSTAT_OUT"
# Utilisation : on capture tout ; le parsing CSV se fait en fin de run.
( iostat -xt -y 5 > "$IOSTAT_OUT" 2>&1 ) &
IOSTAT_PID=$!
log "iostat pid=$IOSTAT_PID"

# Petit délai pour s'assurer qu'iostat a démarré
sleep 2

# --- Étape 5 : vider le page cache OS avant le bench -------------------------
# Sans drop_caches, les runs successifs sur la même instance accumulent du page
# cache OS (ratio RAM/dataset peut être > 1 sur i3en.xlarge, 31 GB / 15 GB = 2×).
# Le régime déclaré "cache-miss-dominant" n'est honnête que si le page cache est vidé.
# Réf : avis-externe §1.1 trou n°1.
log "Vidage du page cache OS (sync + drop_caches=3)..."
sync
if echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null 2>&1; then
    log "drop_caches=3 appliqué — page cache vidé."
    DROP_CACHES_APPLIED="true"
else
    log "AVERTISSEMENT : drop_caches échoué (sudo requis). Le régime cache peut être mixte."
    DROP_CACHES_APPLIED="false"
fi

# --- Étape 6 : lancer le benchmark --------------------------------------------
BENCH_LOG="$RAW_DIR/bench_stdout.log"
BENCH_ERR="$RAW_DIR/bench_stderr.log"
RUN_STARTED_UNIX_MS=$(( $(date -u +%s) * 1000 ))
log "Lancement du bench (BENCH_N=$BENCH_N, BENCH_DIR=$BENCH_DIR)..."
log "Logs : $BENCH_LOG (stdout) + $BENCH_ERR (stderr)"

BENCH_RC=0
CXXFLAGS="-include cstdint" BENCH_N="$BENCH_N" BENCH_DIR="$BENCH_DIR" \
    cargo bench --bench causal_lookup --manifest-path "$REPO_ROOT/poc/Cargo.toml" \
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

# --- Étape 8 : parser iostat → cpu_steal_max, io_wait_max ---------------------
# Lignes pertinentes : après "avg-cpu:", la ligne suivante contient
# %user %nice %system %iowait %steal %idle
# On extrait tous les samples (peut y en avoir des dizaines sur un run long).

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
    # Colonnes 5 (iowait) et 6 (steal), header sauté.
    IO_WAIT_MAX=$(awk -F, 'NR>1 {if ($5+0 > m) m=$5+0} END {if (m=="") print 0; else printf "%.2f", m}' "$CLOUD_CSV")
    CPU_STEAL_MAX=$(awk -F, 'NR>1 {if ($6+0 > m) m=$6+0} END {if (m=="") print 0; else printf "%.2f", m}' "$CLOUD_CSV")
fi
log "cpu_steal_max_pct=$CPU_STEAL_MAX  io_wait_max_pct=$IO_WAIT_MAX"

# --- Étape 9 : extraire métriques T5 depuis le stdout du bench ----------------
# La ligne attendue dans bench_stdout.log :
#   T5_METRICS: {"p50_us":...,"p95_us":...,"p99_us":...,"p99_9_us":...,"pass":...,...}
METRICS_LINE=$(grep -m1 '^T5_METRICS:' "$BENCH_LOG" 2>/dev/null || true)
if [[ -z "${METRICS_LINE:-}" ]]; then
    # parfois Criterion mélange stdout : tenter aussi dans stderr
    METRICS_LINE=$(grep -m1 '^T5_METRICS:' "$BENCH_ERR" 2>/dev/null || true)
fi
METRICS_JSON=""
if [[ -n "${METRICS_LINE:-}" ]]; then
    METRICS_JSON="${METRICS_LINE#T5_METRICS: }"
fi

# Extraire chaque percentile avec awk (pas de jq garanti)
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
                # capture jusqu au prochain , ou }
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

# Outcome final
if [[ "$BENCH_RC" -ne 0 ]]; then
    OUTCOME="inconclusive"
    OUTCOME_NOTE="cargo bench rc=$BENCH_RC ; voir raw/bench_stderr.log"
elif [[ -z "$METRICS_JSON" ]]; then
    OUTCOME="inconclusive"
    OUTCOME_NOTE="Ligne T5_METRICS absente du stdout. Possible build neuf : Criterion peut avoir échoué silencieusement."
elif [[ "$PASS_BOOL" == "true" ]]; then
    OUTCOME="pass"
    OUTCOME_NOTE="p99=${P99}µs ≤ 10000µs"
else
    OUTCOME="fail"
    OUTCOME_NOTE="p99=${P99}µs > 10000µs — H-causal-latence dégradée"
fi

# --- Étape 10 : assembler workload.json ----------------------------------------
# cache_regime : "cache-miss-dominant" uniquement si drop_caches a réussi ET
# dataset >> RAM (ratio > 5× per protocole §2.3). Sinon "cache-mixte".
if [[ "$DROP_CACHES_APPLIED" == "true" ]]; then
    DATASET_SIZE_GB=15
    if (( RAM_GB * 5 > DATASET_SIZE_GB )); then
        CACHE_REGIME="cache-mixte"
        CACHE_NOTE="drop_caches appliqué mais RAM (${RAM_GB}GB) / dataset (~${DATASET_SIZE_GB}GB) = ratio $(( RAM_GB / DATASET_SIZE_GB ))× < 5× requis par §2.3 pour cache-miss-dominant"
    else
        CACHE_REGIME="cache-miss-dominant"
        CACHE_NOTE="drop_caches appliqué, ratio RAM/dataset > 5× conforme §2.3"
    fi
else
    CACHE_REGIME="cache-mixte"
    CACHE_NOTE="drop_caches non appliqué (sudo indisponible) — page cache OS non vidé"
fi

cat > "$RESULTS_DIR/workload.json" <<EOF
{
  "test": "T5",
  "bench_n": $BENCH_N,
  "n_measures": 10000,
  "cache_regime": "$CACHE_REGIME",
  "cache_regime_note": "$CACHE_NOTE",
  "drop_caches_applied": $DROP_CACHES_APPLIED,
  "p99_ms_max_target": 10,
  "bench_dir": "$BENCH_DIR",
  "bench_dir_note": "$NVME_NOTE",
  "run_started_unix_ms": $RUN_STARTED_UNIX_MS,
  "run_ended_unix_ms": $RUN_ENDED_UNIX_MS,
  "wall_duration_ms": $((RUN_ENDED_UNIX_MS - RUN_STARTED_UNIX_MS)),
  "emit_payload_size_distribution": null
}
EOF
log "workload.json écrit."

# --- Étape 11 : assembler verdict.json ----------------------------------------
# Classification : "indicatif" pour un seul run sur une seule instance (protocole §5).

VERDICT_NOTES="$OUTCOME_NOTE — single run, single instance, classification indicatif (protocole §5). $NVME_NOTE"
VERDICT_NOTES_ESCAPED=$(printf '%s' "$VERDICT_NOTES" | sed 's/\\/\\\\/g; s/"/\\"/g')

cat > "$RESULTS_DIR/verdict.json" <<EOF
{
  "test": "T5",
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
    "note": "Températures CPU/NVMe non exposées au tenant AWS — protocole §8.10. cpu_steal et io_wait servent de proxys."
  },
  "notes": "$VERDICT_NOTES_ESCAPED"
}
EOF
log "verdict.json écrit."

# --- Étape 12 : summary.md lisible humain -------------------------------------
cat > "$RESULTS_DIR/summary.md" <<EOF
# T5 — H-causal-latence — run $TS

## Résultat
- **Outcome :** $OUTCOME
- **Classification :** indicatif (1 instance, 1 run — protocole §5)
- **Note :** $OUTCOME_NOTE

## Métriques (10 000 lookups aléatoires)
- p50  : ${P50} µs
- p95  : ${P95} µs
- **p99  : ${P99} µs** (cible ≤ 10 000 µs)
- p99.9 : ${P99_9} µs

## Conditions
- **Propriété mesurée :** P3a (lookup point isolé)
- **BENCH_N :** $BENCH_N
- **Cache régime :** $CACHE_REGIME ($CACHE_NOTE)
- **drop_caches :** $DROP_CACHES_APPLIED
- **BENCH_DIR :** \`$BENCH_DIR\`
- **NVMe note :** $NVME_NOTE
- **RAM instance :** ${RAM_GB} GB
- **Wall duration :** $((RUN_ENDED_UNIX_MS - RUN_STARTED_UNIX_MS)) ms

## Cloud thermal proxy (§8.10)
- cpu_steal_max_pct : $CPU_STEAL_MAX
- io_wait_max_pct   : $IO_WAIT_MAX

## Fichiers
- \`hardware.json\` — CPU, RAM, storage (avec débit fio mesuré), GPU, métadonnées AWS
- \`software.json\` — OS, kernel, rustc, rocksdb_version, git_commit, git_dirty, source_tree_sha256
- \`workload.json\` — paramètres T5
- \`verdict.json\` — outcome + métriques + thermal proxy
- \`raw/iostat.txt\` — capture brute iostat (5 s)
- \`raw/cloud_io.csv\` — CSV parsé (cpu_steal, iowait par sample)
- \`raw/bench_stdout.log\` / \`raw/bench_stderr.log\` — sortie Cargo / bench
EOF
log "summary.md écrit."

# --- Étape 13 : archive -------------------------------------------------------
log "Compression résultats → $ARCHIVE"
tar -czf "$ARCHIVE" -C "$REPO_ROOT/results/T5" "$TS"
ARCHIVE_SIZE_KB=$(( $(stat -c %s "$ARCHIVE") / 1024 ))
log "Archive : $ARCHIVE (${ARCHIVE_SIZE_KB} KB)"

# --- Étape 14 : résumé final --------------------------------------------------
INST_TYPE_DISPLAY="non-aws"
if grep -q '"instance_type"' "$RESULTS_DIR/hardware.json" 2>/dev/null; then
    INST_TYPE_DISPLAY=$(awk -F'"' '/instance_type/ {print $4; exit}' "$RESULTS_DIR/hardware.json")
fi

cat <<EOF

════════════════════════════════════════════════════════════════════════
 T5 BUNDLE — RÉSUMÉ FINAL
════════════════════════════════════════════════════════════════════════
 Outcome             : $OUTCOME
 Classification      : indicatif (protocole §5)
 p99 mesuré          : ${P99} µs (cible ≤ 10 000 µs)
 BENCH_N             : $BENCH_N
 Instance            : $INST_TYPE_DISPLAY
 Archive résultats   : $ARCHIVE
 Taille archive      : ${ARCHIVE_SIZE_KB} KB

 Pour récupérer l'archive depuis votre machine locale :

   scp -i <key.pem> ec2-user@<public-dns>:$ARCHIVE ./
   # ou pour Ubuntu :
   scp -i <key.pem> ubuntu@<public-dns>:$ARCHIVE ./

 Décompression locale :
   tar -xzf t5-results-$TS.tar.gz

════════════════════════════════════════════════════════════════════════
EOF

exit 0
