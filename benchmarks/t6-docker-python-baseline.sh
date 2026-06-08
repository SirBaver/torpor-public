#!/usr/bin/env bash
# T6 Docker Python baseline — overhead RAM réel par container Python 3.11 + deps LLM.
#
# Mesure le RSS idle d'un container avec le runtime agent LLM réaliste :
#   Python 3.11 + langchain-core + openai + httpx + pydantic
#
# Usage : ./benchmarks/t6-docker-python-baseline.sh [N_AGENTS]
#
# Produit le pendant réaliste de t6-docker-baseline.sh (Alpine).
# La baseline pertinente pour H-densité est cette mesure, pas Alpine.

set -euo pipefail

N_AGENTS="${1:-10}"
LABEL="t6-python-baseline"
IMAGE="os-poc-t6-python-agent:latest"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== T6 Docker Python baseline : overhead RAM par container LLM agent ==="
echo ""
echo "  Image   : $IMAGE (Python 3.11 + langchain-core + openai + httpx + pydantic)"
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

# ── Build de l'image ─────────────────────────────────────────────────────────
echo "  Build $IMAGE..."
docker build -t "$IMAGE" "$SCRIPT_DIR/t6-python-agent/" --quiet
echo "  Build OK."
echo ""

# ── Nettoyage préalable ──────────────────────────────────────────────────────
docker ps -a --filter "label=t6-role=$LABEL" -q \
    | xargs -r docker rm -f &>/dev/null || true

# ── Drop caches pour mesure propre ──────────────────────────────────────────
sync
echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null 2>&1 || true

sleep 2

# ── Mesure (A) : RAM hôte AVANT lancement ────────────────────────────────────
mem_before_kb=$(grep '^MemAvailable:' /proc/meminfo | awk '{print $2}')
echo "  RAM disponible avant : ${mem_before_kb} KB"

# ── Lancement des containers ─────────────────────────────────────────────────
echo "  Lancement de $N_AGENTS containers..."

for i in $(seq 1 "$N_AGENTS"); do
    docker run -d \
        --label "t6-role=$LABEL" \
        --name "t6-python-$i" \
        "$IMAGE" \
        > /dev/null
done

# Attendre que tous les agents soient READY (max 60s)
echo "  Attente que les agents soient prêts..."
deadline=$(( $(date +%s) + 60 ))
ready=0
while [ "$ready" -lt "$N_AGENTS" ] && [ "$(date +%s)" -lt "$deadline" ]; do
    ready=$(docker ps --filter "label=t6-role=$LABEL" -q \
        | xargs -r -I{} docker logs {} 2>/dev/null \
        | grep -c "READY" || true)
    sleep 1
done

if [ "$ready" -lt "$N_AGENTS" ]; then
    echo "  AVERTISSEMENT : seulement $ready/$N_AGENTS agents READY après 60s — mesure quand même."
fi

echo "  Attente stabilisation RSS (5s)..."
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

# ── Mesure (B) : docker stats ────────────────────────────────────────────────
echo ""
echo "  ── (B) docker stats (RSS userspace process) ──"

docker stats --no-stream --format "{{.Name}}  {{.MemUsage}}  {{.MemPerc}}" \
    $(docker ps --filter "label=t6-role=$LABEL" -q)

stats_total_kb=$(docker stats --no-stream --format "{{.MemUsage}}" \
    $(docker ps --filter "label=t6-role=$LABEL" -q) \
    | awk '{
        val = $1
        split(val, a, /[A-Za-z]+/)
        num = a[1] + 0
        if (val ~ /GiB/) kb = num * 1024 * 1024
        else if (val ~ /MiB/) kb = num * 1024
        else kb = num
        total += kb
    }
    END { printf "%.0f", total }')

stats_per_kb=$(awk "BEGIN {printf \"%.0f\", $stats_total_kb / $N_AGENTS}")

echo ""
echo "    Total userspace : ${stats_total_kb} KB pour $N_AGENTS containers"
echo "    Overhead/container : ${stats_per_kb} KB  ($(awk "BEGIN {printf \"%.1f\", $stats_per_kb/1024}") MiB)"

# ── Résumé H-densité ──────────────────────────────────────────────────────────
echo ""
echo "  ── Résumé comparatif H-densité (nouveau modèle W1 révisé) ──"
echo ""
echo "  Modèle W1 révisé : état 50 MB dans ContentStore (partagé), pas en RAM par acteur."
echo "  → L'overhead à comparer est l'infrastructure runtime seule."
echo ""

ram_total_kb=$((16 * 1024 * 1024))

# W1 révisé : overhead infrastructure seul (pas + 50 MB état app)
density_wasmtime_infra=$(awk "BEGIN {printf \"%.0f\", $ram_total_kb / 5}")        # 5 KB overhead Wasmtime
density_docker_infra_host=$(awk "BEGIN {printf \"%.0f\", $ram_total_kb / ($delta_per_kb > 0 ? $delta_per_kb : 1)}")
density_docker_infra_proc=$(awk "BEGIN {printf \"%.0f\", $ram_total_kb / ($stats_per_kb > 0 ? $stats_per_kb : 1)}")

echo "  Agents hébergeables (overhead infra seul, état dans ContentStore) :"
echo "    Wasmtime (5 KB/acteur idle)          : ${density_wasmtime_infra}"
echo "    Docker Python (A — hôte) $(awk "BEGIN {printf \"%5.0f\", $delta_per_kb}") KB/cnt : ${density_docker_infra_host}"
echo "    Docker Python (B — process) $(awk "BEGIN {printf \"%5.0f\", $stats_per_kb}") KB/cnt : ${density_docker_infra_proc}"
echo ""

ratio_host=$(awk "BEGIN {printf \"%.0f\", $density_wasmtime_infra / ($density_docker_infra_host > 0 ? $density_docker_infra_host : 1)}")
ratio_proc=$(awk "BEGIN {printf \"%.0f\", $density_wasmtime_infra / ($density_docker_infra_proc > 0 ? $density_docker_infra_proc : 1)}")

echo "    Ratio Wasmtime/Docker (hôte)   : ${ratio_host}×"
echo "    Ratio Wasmtime/Docker (process): ${ratio_proc}×"
echo "    Cible H-densité : ≥ 5×"
echo ""

if [ "$ratio_host" -ge 5 ] 2>/dev/null; then
    echo "    ✓ H-densité satisfaite (méthode A — hôte)"
else
    echo "    ✗ H-densité non satisfaite (méthode A — hôte) avec ce N"
fi
if [ "$ratio_proc" -ge 5 ] 2>/dev/null; then
    echo "    ✓ H-densité satisfaite (méthode B — process)"
else
    echo "    ✗ H-densité non satisfaite (méthode B — process) avec ce N"
fi

# ── Nettoyage ────────────────────────────────────────────────────────────────
echo ""
echo "  Nettoyage..."
docker ps --filter "label=t6-role=$LABEL" -q \
    | xargs -r docker rm -f &>/dev/null
echo "  Done."
