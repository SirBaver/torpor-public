#!/usr/bin/env bash
# hardware_probe.sh — collecte hardware.json pour le protocole §3.1.
#
# Usage : hardware_probe.sh <bench_dir> <output_json>
#
#   <bench_dir>    : répertoire sur lequel mesurer le débit séquentiel (fio).
#                    Doit être sur le filesystem effectivement utilisé par le bench.
#   <output_json>  : chemin du fichier hardware.json à écrire.
#
# Sortie : un JSON valide conforme à test-protocol.md §3.1, avec :
#   - cpu_model, cpu_cores_physical, cpu_cores_logical, cpu_base_ghz (lus depuis lscpu/proc)
#   - ram_gb, ram_type (lus depuis /proc/meminfo et dmidecode si disponible)
#   - storage_model, storage_seq_read_mb_s (modèle via lsblk, débit via fio direct=1)
#   - gpu_model, gpu_vram_gb (lus via nvidia-smi si disponible, sinon null)
#   - cloud_metadata : si IMDSv2 répond, capture instance-type/az/instance-id
#
# Notes :
#   - Le débit fio utilise direct=1 et bs=1M, runtime=10 s, fichier de 1 GB.
#     Ne fonctionne pas sur tmpfs (qui ignore O_DIRECT) ; ce cas est détecté et reporté.
#   - dmidecode requiert root ; en cas d'échec, ram_type est null sans bloquer.

set -euo pipefail
# Forcer locale C : sinon `printf "%.2f"` peut produire "2,10" en fr_FR.UTF-8,
# ce qui casse le JSON (virgule décimale interprétée comme séparateur de champs).
export LC_ALL=C

if [[ $# -ne 2 ]]; then
    echo "Usage: $0 <bench_dir> <output_json>" >&2
    exit 2
fi

BENCH_DIR="$1"
OUT="$2"

# --- CPU ----------------------------------------------------------------------
CPU_MODEL=$(LC_ALL=C lscpu 2>/dev/null | awk -F: '/^Model name/ {sub(/^ +/, "", $2); print $2; exit}')
CPU_MODEL=${CPU_MODEL:-unknown}
CPU_CORES_PHYSICAL=$(LC_ALL=C lscpu -p=Core,Socket 2>/dev/null | grep -v '^#' | sort -u | wc -l || echo 0)
CPU_CORES_LOGICAL=$(nproc 2>/dev/null || echo 0)

# Fréquence de base en GHz : MHz depuis lscpu, sinon /proc/cpuinfo cpu MHz
CPU_BASE_MHZ=$(LC_ALL=C lscpu 2>/dev/null | awk -F: '/^CPU MHz|^CPU max MHz/ {sub(/^ +/, "", $2); print $2; exit}')
if [[ -z "${CPU_BASE_MHZ:-}" ]]; then
    CPU_BASE_MHZ=$(awk -F: '/cpu MHz/ {gsub(/ /, "", $2); print $2; exit}' /proc/cpuinfo 2>/dev/null || echo 0)
fi
CPU_BASE_GHZ=$(awk -v m="$CPU_BASE_MHZ" 'BEGIN { if (m+0 > 0) printf "%.2f", m/1000.0; else print "null" }')

# --- RAM ----------------------------------------------------------------------
RAM_KB=$(awk '/^MemTotal:/ {print $2; exit}' /proc/meminfo 2>/dev/null || echo 0)
RAM_GB=$(awk -v k="$RAM_KB" 'BEGIN { if (k+0 > 0) printf "%d", (k+524288)/1048576; else print 0 }')

# ram_type : nécessite dmidecode + root. Sur AWS sans root, retourne null.
RAM_TYPE="null"
if command -v dmidecode >/dev/null 2>&1; then
    # Premier slot configuré (Speed != "Unknown")
    RAM_INFO=$(sudo -n dmidecode -t memory 2>/dev/null | awk '
        /Type:/ && $2 != "Unknown" && type == "" {type=$2}
        /Speed:/ && $2 != "Unknown" && speed == "" {speed=$2" "$3}
        END {if (type != "") printf "%s-%s", type, speed}
    ' || true)
    if [[ -n "${RAM_INFO:-}" ]]; then
        RAM_TYPE=$(printf '"%s"' "$RAM_INFO" | tr -d '\n')
    fi
fi

# --- Storage ------------------------------------------------------------------
# Trouver le device qui supporte BENCH_DIR
DEV=$(df --output=source "$BENCH_DIR" 2>/dev/null | tail -n1 | tr -d ' ')
DEV_BASENAME=$(basename "$DEV")
# Remonter au disque parent (nvme0n1p1 → nvme0n1).
# lsblk peut échouer si DEV n'est pas un block device (tmpfs, overlay) ;
# on tolère l'échec (pipefail désactivé localement pour cette commande).
PARENT=""
if [[ -b "$DEV" ]]; then
    PARENT=$(lsblk -no PKNAME "$DEV" 2>/dev/null | head -n1 | tr -d ' ' || true)
fi
PARENT=${PARENT:-$DEV_BASENAME}
STORAGE_MODEL=""
if [[ -b "/dev/$PARENT" ]]; then
    STORAGE_MODEL=$(lsblk -dno MODEL "/dev/$PARENT" 2>/dev/null | sed -E 's/[ \t]+$//' | head -n1 || true)
fi
if [[ -z "${STORAGE_MODEL:-}" ]]; then
    STORAGE_MODEL="unknown ($DEV)"
fi

# Mesure fio — deux profils :
#   QD=1  mono-thread : coût d'une opération unitaire (borne basse, C2 admission control).
#   QD=32 multi-thread : capacité de classe hardware (borne haute, référence constructeur).
# Les deux sont reportés séparément — ne pas confondre les deux dans les calculs C2.
STORAGE_SEQ_READ_QD1="null"
STORAGE_SEQ_READ_QD32="null"
STORAGE_RAND_READ_IOPS_QD1="null"
STORAGE_RAND_READ_IOPS_QD32="null"
FIO_NOTE=""
FS_TYPE=$(df --output=fstype "$BENCH_DIR" 2>/dev/null | tail -n1 | tr -d ' ')
if [[ "$FS_TYPE" == "tmpfs" ]]; then
    FIO_NOTE="storage_seq_read non mesurable sur tmpfs (O_DIRECT ignoré)"
elif command -v fio >/dev/null 2>&1; then
    FIO_FILE="$BENCH_DIR/.t5_fio_probe.dat"

    # QD=1 — mono-thread, iodepth=1, numjobs=1 (latence unitaire)
    FIO_JSON_QD1=$(fio --name=t5probe_qd1 --filename="$FIO_FILE" --rw=read \
                       --bs=1M --size=1G --direct=1 --ioengine=libaio --iodepth=1 --numjobs=1 \
                       --runtime=10 --time_based --group_reporting --output-format=json 2>/dev/null \
                   || true)
    if [[ -n "${FIO_JSON_QD1:-}" ]]; then
        BW_BYTES=$(printf '%s' "$FIO_JSON_QD1" \
            | awk '/"bw_bytes"/ {gsub(/[^0-9]/,"",$3); if ($3+0>0) {print $3; exit}}')
        if [[ -n "${BW_BYTES:-}" && "$BW_BYTES" -gt 0 ]]; then
            STORAGE_SEQ_READ_QD1=$(awk -v b="$BW_BYTES" 'BEGIN { printf "%d", b/1000000 }')
        fi
    fi

    # QD=32 — multi-thread, iodepth=32, numjobs=4 (capacité hardware)
    # bs=1M identique à QD=1 : les deux mesurent le débit séquentiel, seul le parallélisme diffère.
    FIO_JSON_QD32=$(fio --name=t5probe_qd32 --filename="$FIO_FILE" --rw=read \
                        --bs=1M --size=4G --direct=1 --ioengine=libaio --iodepth=32 --numjobs=4 \
                        --runtime=30 --time_based --group_reporting --output-format=json 2>/dev/null \
                    || true)
    if [[ -n "${FIO_JSON_QD32:-}" ]]; then
        BW_BYTES=$(printf '%s' "$FIO_JSON_QD32" \
            | awk '/"bw_bytes"/ {gsub(/[^0-9]/,"",$3); if ($3+0>0) {print $3; exit}}')
        if [[ -n "${BW_BYTES:-}" && "$BW_BYTES" -gt 0 ]]; then
            STORAGE_SEQ_READ_QD32=$(awk -v b="$BW_BYTES" 'BEGIN { printf "%d", b/1000000 }')
        fi
    fi

    # Rand 4K QD=1 — latence I/O aléatoire unitaire (métrique P3a et C2, distincte du débit séquentiel)
    FIO_JSON_RAND_QD1=$(fio --name=t5probe_rand_qd1 --filename="$FIO_FILE" --rw=randread \
                           --bs=4k --size=1G --direct=1 --ioengine=libaio --iodepth=1 --numjobs=1 \
                           --runtime=10 --time_based --group_reporting --output-format=json 2>/dev/null \
                        || true)
    if [[ -n "${FIO_JSON_RAND_QD1:-}" ]]; then
        RAND_IOPS=$(printf '%s' "$FIO_JSON_RAND_QD1" \
            | awk '/"iops"/ && !/iops_/ {val=$3; gsub(/[^0-9.]/,"",val); if (val+0>0) {printf "%d", int(val+0.5); exit}}')
        if [[ -n "${RAND_IOPS:-}" && "$RAND_IOPS" -gt 0 ]]; then
            STORAGE_RAND_READ_IOPS_QD1="$RAND_IOPS"
        fi
    fi

    # Rand 4K QD=32 — capacité IOPS hardware multi-thread
    FIO_JSON_RAND_QD32=$(fio --name=t5probe_rand_qd32 --filename="$FIO_FILE" --rw=randread \
                            --bs=4k --size=4G --direct=1 --ioengine=libaio --iodepth=32 --numjobs=4 \
                            --runtime=30 --time_based --group_reporting --output-format=json 2>/dev/null \
                         || true)
    if [[ -n "${FIO_JSON_RAND_QD32:-}" ]]; then
        RAND_IOPS=$(printf '%s' "$FIO_JSON_RAND_QD32" \
            | awk '/"iops"/ && !/iops_/ {val=$3; gsub(/[^0-9.]/,"",val); if (val+0>0) {printf "%d", int(val+0.5); exit}}')
        if [[ -n "${RAND_IOPS:-}" && "$RAND_IOPS" -gt 0 ]]; then
            STORAGE_RAND_READ_IOPS_QD32="$RAND_IOPS"
        fi
    fi

    rm -f "$FIO_FILE"

    if [[ "$STORAGE_SEQ_READ_QD1" == "null" && "$STORAGE_SEQ_READ_QD32" == "null" && \
          "$STORAGE_RAND_READ_IOPS_QD1" == "null" && "$STORAGE_RAND_READ_IOPS_QD32" == "null" ]]; then
        FIO_NOTE="fio a tourné mais aucune métrique extraite du JSON (bw_bytes/iops introuvable)"
    fi
else
    FIO_NOTE="fio absent ; débit non mesuré"
fi
# Rétrocompatibilité : storage_seq_read_mb_s = valeur QD=1 (ancienne sémantique)
STORAGE_SEQ_READ="$STORAGE_SEQ_READ_QD1"

# --- GPU ----------------------------------------------------------------------
GPU_MODEL="null"
GPU_VRAM_GB="null"
if command -v nvidia-smi >/dev/null 2>&1; then
    NV=$(nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>/dev/null | head -n1 || true)
    if [[ -n "${NV:-}" ]]; then
        GPU_NAME=$(printf '%s' "$NV" | awk -F, '{sub(/^ +/, "", $1); print $1}')
        GPU_MIB=$(printf '%s' "$NV" | awk -F, '{gsub(/ /, "", $2); print $2}')
        if [[ -n "${GPU_NAME:-}" ]]; then
            GPU_MODEL=$(printf '"%s"' "$GPU_NAME")
        fi
        if [[ -n "${GPU_MIB:-}" && "$GPU_MIB" -gt 0 ]]; then
            GPU_VRAM_GB=$(awk -v m="$GPU_MIB" 'BEGIN { printf "%d", (m+512)/1024 }')
        fi
    fi
fi

# --- Cloud metadata (AWS IMDSv2) ---------------------------------------------
CLOUD_JSON="null"
if command -v curl >/dev/null 2>&1; then
    TOKEN=$(curl -fsS -m 1 -X PUT "http://169.254.169.254/latest/api/token" \
              -H "X-aws-ec2-metadata-token-ttl-seconds: 60" 2>/dev/null || true)
    if [[ -n "${TOKEN:-}" ]]; then
        INST_TYPE=$(curl -fsS -m 1 -H "X-aws-ec2-metadata-token: $TOKEN" \
                    http://169.254.169.254/latest/meta-data/instance-type 2>/dev/null || true)
        AZ=$(curl -fsS -m 1 -H "X-aws-ec2-metadata-token: $TOKEN" \
                    http://169.254.169.254/latest/meta-data/placement/availability-zone 2>/dev/null || true)
        INST_ID=$(curl -fsS -m 1 -H "X-aws-ec2-metadata-token: $TOKEN" \
                    http://169.254.169.254/latest/meta-data/instance-id 2>/dev/null || true)
        if [[ -n "${INST_TYPE:-}" ]]; then
            CLOUD_JSON=$(printf '{"provider":"aws","instance_type":"%s","availability_zone":"%s","instance_id":"%s"}' \
                "${INST_TYPE:-unknown}" "${AZ:-unknown}" "${INST_ID:-unknown}")
        fi
    fi
fi

# --- Sortie JSON --------------------------------------------------------------
# Champs RAM_TYPE, GPU_MODEL, CLOUD_JSON sont déjà soit `null` (sans guillemets)
# soit du JSON valide ; on les insère tels quels.

mkdir -p "$(dirname "$OUT")"
cat > "$OUT" <<EOF
{
  "cpu_model": "$CPU_MODEL",
  "cpu_cores_physical": $CPU_CORES_PHYSICAL,
  "cpu_cores_logical": $CPU_CORES_LOGICAL,
  "cpu_base_ghz": $CPU_BASE_GHZ,
  "ram_gb": $RAM_GB,
  "ram_type": $RAM_TYPE,
  "storage_device": "$DEV",
  "storage_model": "$STORAGE_MODEL",
  "storage_fs_type": "$FS_TYPE",
  "storage_seq_read_mb_s": $STORAGE_SEQ_READ,
  "storage_seq_read_mb_s_qd1": $STORAGE_SEQ_READ_QD1,
  "storage_seq_read_mb_s_qd32": $STORAGE_SEQ_READ_QD32,
  "storage_rand_read_iops_qd1": $STORAGE_RAND_READ_IOPS_QD1,
  "storage_rand_read_iops_qd32": $STORAGE_RAND_READ_IOPS_QD32,
  "storage_seq_read_note": "$FIO_NOTE",
  "gpu_model": $GPU_MODEL,
  "gpu_vram_gb": $GPU_VRAM_GB,
  "cloud": $CLOUD_JSON
}
EOF

echo "hardware.json écrit : $OUT" >&2
