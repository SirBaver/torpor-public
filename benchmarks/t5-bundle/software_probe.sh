#!/usr/bin/env bash
# software_probe.sh — collecte software.json pour le protocole §3.2.
#
# Usage : software_probe.sh <repo_root> <output_json>
#
#   <repo_root>    : racine du repo git (pour git_commit + git_dirty).
#   <output_json>  : chemin du fichier software.json à écrire.
#
# Champs : os (distro + kernel), rustc, rocksdb_version (lu depuis Cargo.lock),
# git_commit, git_dirty, build_cxxflags. Pas de docker / ollama ici (T5 n'en
# dépend pas) — ces champs restent null.

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: $0 <repo_root> <output_json>" >&2
    exit 2
fi

REPO="$1"
OUT="$2"

# --- OS / kernel --------------------------------------------------------------
OS_STRING="unknown"
if [[ -r /etc/os-release ]]; then
    . /etc/os-release
    OS_STRING="${PRETTY_NAME:-${NAME:-unknown}}"
fi
KERNEL=$(uname -r 2>/dev/null || echo unknown)

# --- rustc --------------------------------------------------------------------
RUST_VERSION="null"
if command -v rustc >/dev/null 2>&1; then
    RV=$(rustc --version 2>/dev/null | tr -d '\n')
    RUST_VERSION=$(printf '"%s"' "$RV")
fi

# --- rocksdb (lu depuis Cargo.lock) -------------------------------------------
# Format attendu dans Cargo.lock :
#   [[package]]
#   name = "rocksdb"
#   version = "0.22.0"
ROCKSDB_VERSION="null"
LOCK="$REPO/poc/Cargo.lock"
if [[ -r "$LOCK" ]]; then
    RDB=$(awk '
        /^\[\[package\]\]/ { in_pkg=1; name=""; version=""; next }
        in_pkg && /^name = / { gsub(/"/, "", $3); name=$3; next }
        in_pkg && /^version = / { gsub(/"/, "", $3); version=$3; next }
        in_pkg && /^$/ { if (name=="rocksdb") { print version; exit } in_pkg=0 }
    ' "$LOCK" || true)
    if [[ -n "${RDB:-}" ]]; then
        ROCKSDB_VERSION=$(printf '"%s"' "$RDB")
    fi
fi

# --- git ----------------------------------------------------------------------
GIT_COMMIT="null"
GIT_DIRTY="false"
if [[ -d "$REPO/.git" ]] && command -v git >/dev/null 2>&1; then
    GC=$(git -C "$REPO" rev-parse HEAD 2>/dev/null || true)
    if [[ -n "${GC:-}" ]]; then
        GIT_COMMIT=$(printf '"%s"' "$GC")
    fi
    # Working tree dirty si `git status --porcelain` retourne quoi que ce soit.
    if [[ -n "$(git -C "$REPO" status --porcelain 2>/dev/null || true)" ]]; then
        GIT_DIRTY="true"
    fi
fi

# --- CXXFLAGS (workaround GCC 15 — cf. project_gcc15_cxxflags) ----------------
BUILD_CXXFLAGS="${CXXFLAGS:-}"
BUILD_CXXFLAGS_ESCAPED=$(printf '%s' "$BUILD_CXXFLAGS" | sed 's/\\/\\\\/g; s/"/\\"/g')

# --- Source tree SHA-256 -------------------------------------------------------
# Calcule un hash récursif de l'arbre source (hors target/, .git/).
# Permet de vérifier que le code exécuté correspond exactement à l'arbre local,
# même sans .git/ présent sur l'instance. Analogue à `git archive` côté intégrité.
SOURCE_TREE_SHA256="null"
if command -v sha256sum >/dev/null 2>&1 && command -v find >/dev/null 2>&1; then
    TREE_HASH=$(find "$REPO/poc" -type f \
        -not -path '*/target/*' \
        -not -path '*/.git/*' \
        | sort \
        | xargs sha256sum 2>/dev/null \
        | sha256sum \
        | awk '{print $1}' || true)
    if [[ -n "${TREE_HASH:-}" ]]; then
        SOURCE_TREE_SHA256=$(printf '"%s"' "$TREE_HASH")
    fi
fi

# --- Sortie -------------------------------------------------------------------
mkdir -p "$(dirname "$OUT")"
cat > "$OUT" <<EOF
{
  "os": "$OS_STRING",
  "kernel": "$KERNEL",
  "docker_version": null,
  "docker_image_hashes": null,
  "python_version": null,
  "rust_version": $RUST_VERSION,
  "rocksdb_version": $ROCKSDB_VERSION,
  "ollama_version": null,
  "git_commit": $GIT_COMMIT,
  "git_dirty": $GIT_DIRTY,
  "source_tree_sha256": $SOURCE_TREE_SHA256,
  "build_cxxflags": "$BUILD_CXXFLAGS_ESCAPED"
}
EOF

echo "software.json écrit : $OUT" >&2
