#!/usr/bin/env bash
# T6 Docker baseline — overhead RAM réel par container (hôte) pour H-densité.
#
# Usage : ./benchmarks/t6-docker-baseline.sh [N_AGENTS]
#
# Deux mesures complémentaires :
#   (A) Delta RAM hôte (MemAvailable avant/après) — inclut namespaces kernel,
#       overlay2, slab, tout le coût réel du point de vue de l'OS hôte.
#   (B) docker stats (mémoire userspace du process dans le container).
#
# À comparer avec :
#   CXXFLAGS="-include cstdint" cargo run -p os-poc-benchmarks -- t6

set -euo pipefail

N_AGENTS="${1:-20}"
LABEL="t6-baseline"
IMAGE="alpine:3.19"

echo "=== T6 Docker baseline : overhead RAM par container ==="
echo ""
echo "  Image   : $IMAGE"
echo "  N       : $N_AGENTS containers"
echo "  Méthode : (A) delta MemAvailable hôte  +  (B) docker stats process"
echo ""

# ── Vérifier docker ──────────────────────────────────────────────────────────
if ! command -v docker &>/dev/null; then
    echo "ERREUR : docker non trouvé." >&2; exit 1
fi
if ! docker info &>/dev/null; then
    echo "ERREUR : daemon Docker non actif." >&2; exit 1
fi

# ── Nettoyage préalable ──────────────────────────────────────────────────────
docker ps -a --filter "label=t6-role=$LABEL" -q \
    | xargs -r docker rm -f &>/dev/null || true

# ── Pull image ───────────────────────────────────────────────────────────────
echo "  Pull $IMAGE..."
docker pull "$IMAGE" --quiet

# ── Mesure (A) : RAM hôte AVANT lancement ────────────────────────────────────
mem_before_kb=$(grep '^MemAvailable:' /proc/meminfo | awk '{print $2}')
echo ""
echo "  RAM disponible avant : ${mem_before_kb} KB"

# ── Lancement des containers ─────────────────────────────────────────────────
echo "  Lancement de $N_AGENTS containers..."

for i in $(seq 1 "$N_AGENTS"); do
    docker run -d \
        --label "t6-role=$LABEL" \
        --memory="512m" \
        --cpus="0.1" \
        --name "t6-agent-$i" \
        "$IMAGE" \
        sleep infinity \
        > /dev/null
done

echo "  Attente stabilisation (5s)..."
sleep 5

# ── Mesure (A) : RAM hôte APRÈS lancement ────────────────────────────────────
mem_after_kb=$(grep '^MemAvailable:' /proc/meminfo | awk '{print $2}')
delta_total_kb=$(( mem_before_kb - mem_after_kb ))
delta_per_kb=$(awk "BEGIN {printf \"%.0f\", $delta_total_kb / $N_AGENTS}")

echo "  RAM disponible après : ${mem_after_kb} KB"
echo ""
echo "  ── (A) Delta RAM hôte (overhead réel vu de l'OS) ──"
echo "    Total delta : ${delta_total_kb} KB pour $N_AGENTS containers"
echo "    Overhead/container : ${delta_per_kb} KB  ($(awk "BEGIN {printf \"%.1f\", $delta_per_kb/1024}") MiB)"

# ── Mesure (B) : docker stats (mémoire userspace process) ────────────────────
echo ""
echo "  ── (B) docker stats (mémoire userspace du process dans le container) ──"

# Afficher les 5 premiers
docker stats --no-stream --format "{{.Name}}  {{.MemUsage}}  {{.MemPerc}}" \
    $(docker ps --filter "label=t6-role=$LABEL" -q) \
    | head -5
echo "  ..."

# Parser correctement KiB / MiB / GiB → KB
stats_total_kb=$(docker stats --no-stream --format "{{.MemUsage}}" \
    $(docker ps --filter "label=t6-role=$LABEL" -q) \
    | awk '{
        val = $1
        # Extraire la valeur numérique et l unité
        split(val, a, /[A-Za-z]+/)
        num = a[1] + 0
        if (val ~ /GiB/) kb = num * 1024 * 1024
        else if (val ~ /MiB/) kb = num * 1024
        else kb = num  # KiB (cas typique pour containers légers)
        total += kb
    }
    END { printf "%.0f", total }')

stats_per_kb=$(awk "BEGIN {printf \"%.0f\", $stats_total_kb / $N_AGENTS}")

echo ""
echo "    Total userspace : ${stats_total_kb} KB pour $N_AGENTS containers"
echo "    Overhead/container : ${stats_per_kb} KB  ($(awk "BEGIN {printf \"%.2f\", $stats_per_kb/1024}") MiB)"

# ── Résumé et comparaison ─────────────────────────────────────────────────────
echo ""
echo "  ── Résumé comparatif (H-densité) ──"

# Densité sur 16 GB avec état app 50 MB = 51200 KB
app_state_kb=51200
ram_total_kb=$((16 * 1024 * 1024))

density_docker_host=$(awk "BEGIN {printf \"%.0f\", $ram_total_kb / ($delta_per_kb + $app_state_kb)}")
density_docker_proc=$(awk "BEGIN {printf \"%.0f\", $ram_total_kb / ($stats_per_kb + $app_state_kb)}")
density_wasmtime=328  # résultat T6 dev : 5 KB overhead + 51200 KB app ≈ 328 acteurs

echo "    Overhead Docker (A — hôte)   : ${delta_per_kb} KB/container"
echo "    Overhead Docker (B — process): ${stats_per_kb} KB/container"
echo "    Overhead Wasmtime (T6 dev)   : 5 KB/acteur"
echo ""
echo "    Densité 16 GB (50 MB état app) :"
echo "      Docker (hôte)   : ${density_docker_host} containers"
echo "      Docker (process): ${density_docker_proc} containers"
echo "      Wasmtime        : ${density_wasmtime} acteurs"
echo ""

ratio_host=$(awk "BEGIN {printf \"%.1f\", $density_wasmtime / ($density_docker_host > 0 ? $density_docker_host : 1)}")
ratio_proc=$(awk "BEGIN {printf \"%.1f\", $density_wasmtime / ($density_docker_proc > 0 ? $density_docker_proc : 1)}")

echo "    Ratio Wasmtime/Docker (hôte)   : ${ratio_host}×"
echo "    Ratio Wasmtime/Docker (process): ${ratio_proc}×"
echo "    Cible H-densité : ≥ 5×"

# ── Nettoyage ────────────────────────────────────────────────────────────────
echo ""
echo "  Nettoyage..."
docker ps --filter "label=t6-role=$LABEL" -q \
    | xargs -r docker rm -f &>/dev/null
echo "  Done."
echo ""
echo "  Note : (A) est la mesure canonique pour H-densité — elle inclut tous"
echo "  les coûts kernel (namespaces, slab, overlay2) que (B) ignore."
