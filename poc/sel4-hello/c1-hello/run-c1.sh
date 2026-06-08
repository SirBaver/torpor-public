#!/usr/bin/env bash
# Jalon C.1 — seL4 hello world officiel (ADR-0039)
# Cible : QEMU AArch64 virt, Cortex-A57, seL4 15.0.0
# Critère : TEST_PASS sur UART QEMU dans les 3 secondes
# Prérequis : Docker
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/rust-root-task-demo"

echo "=== Jalon C.1 — seL4/rust-root-task-demo sur QEMU AArch64 ==="
echo "    ADR-0039 : cible PoC Phase 8"
echo ""

# Vérification Docker
if ! command -v docker &>/dev/null; then
    echo "ERREUR : Docker requis (non trouvé)"
    exit 1
fi

# Clonage du demo officiel
if [ ! -d "$DEMO_DIR/.git" ]; then
    echo "[C.1] Clonage de seL4/rust-root-task-demo (seL4 15.0.0)..."
    git clone --depth 1 https://github.com/seL4/rust-root-task-demo.git "$DEMO_DIR"
    echo "[C.1] Clone terminé."
else
    echo "[C.1] rust-root-task-demo : déjà cloné ($DEMO_DIR)"
fi

cd "$DEMO_DIR"

# Construction de l'image Docker
# Contient : seL4 15.0.0 compilé, Rust nightly, sel4-kernel-loader, QEMU AArch64
if ! docker image inspect rust-root-task-demo &>/dev/null; then
    echo ""
    echo "[C.1] Construction image Docker (seL4 kernel + Rust toolchain)..."
    echo "      ~10–20 min au premier build — seL4 15.0.0 compilé depuis les sources."
    echo ""
    make -C docker/ build
    echo "[C.1] Image Docker construite."
else
    echo "[C.1] Image Docker rust-root-task-demo : déjà disponible."
fi

# Test automatisé : QEMU AArch64 + pexpect cherche TEST_PASS (timeout 3 s)
echo ""
echo "[C.1] Lancement test QEMU AArch64 (critère : TEST_PASS dans les 3 s)..."
make -C docker/ test

echo ""
echo "=== Jalon C.1 : PASS ==="
echo "    seL4 15.0.0 démarre sur QEMU AArch64 virt."
echo "    Toolchain + Docker + QEMU validés."
echo "    Prochaine étape : make -C c2-root-task/ test  (Jalon C.2)"
