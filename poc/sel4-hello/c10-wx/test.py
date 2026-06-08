#!/usr/bin/env python3
"""Test runner C.10 — W^X du pool JIT Wasmtime sur seL4 (ADR-0047).

Deux runs QEMU sur le même disk.img :
  Phase A (PHASE=0) : K=1 commit avec W^X actif + test négatif
    → attend C10_HAPPY_PASS (happy path W^X)
    → attend C10_NEG_PASS (VM fault sur page RX confirmé par superviseur)
    → attend C10_PASS
  Phase B (PHASE=1) : D-reopen (disk.img NON recréé)
    → attend C10_REOPEN_PASS

C10_PASS = les quatre critères (ADR-0047 §D3).

Usage : python3 test.py [phase-a | phase-b]
"""

import sys
import os
import subprocess
import pexpect

BUILD    = os.environ.get("BUILD",    "/poc/sel4-hello/c10-wx/build")
DISK_IMG = os.environ.get("DISK_IMG", f"{BUILD}/disk.img")
IMAGE_A  = os.environ.get("IMAGE_A",  f"{BUILD}/image-phase-a.elf")
IMAGE_B  = os.environ.get("IMAGE_B",  f"{BUILD}/image-phase-b.elf")
TIMEOUT  = 300

def qemu_cmd(image):
    return (
        f"qemu-system-aarch64 "
        f"-machine virt,virtualization=on "
        f"-cpu cortex-a57 "
        f"-m 1G "
        f"-serial mon:stdio "
        f"-nographic "
        f"-drive if=none,id=blk0,file={DISK_IMG},format=raw,cache=none,aio=native "
        f"-device virtio-blk-device,drive=blk0 "
        f"-kernel {image}"
    )

mode = sys.argv[1] if len(sys.argv) > 1 else "phase-a"

if mode == "phase-a":
    print(f"[C10] Phase A — W^X + K=1 commit + test négatif")
    # disk.img créé ici (et UNIQUEMENT ici — pas recréé avant Phase B)
    subprocess.run(
        ["dd", "if=/dev/zero", f"of={DISK_IMG}", "bs=1M", "count=8"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    print(f"[C10] disk.img créé (8 MB)")

    child = pexpect.spawn(qemu_cmd(IMAGE_A), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        child.expect("C10_HAPPY_PASS")
        print("\n[C10] C10_HAPPY_PASS ✓ (K=1 commit sous W^X actif)")

        child.expect("C10_PASS")
        print("\n[C10] C10_PASS ✓ (happy path superviseur)")

        # Test négatif : le runtime tente d'écrire sur une page RX.
        # seL4 debug kernel imprime "vm fault on data at address 0x4..." (ADR-0047 §D3 critère 2).
        # test.py observe ce message pour valider C10_NEG_PASS.
        idx = child.expect(
            ["vm fault on data at address 0x4", "C10_NEG_FAIL"],
            timeout=60
        )
        child.terminate(force=True)
        child.close()
        if idx == 0:
            print("\n[C10] C10_NEG_PASS ✓ (VM fault observé sur page RX — W^X prouvé)")
            print("[C10] Phase A : C10_PASS (happy+neg) ✓")
            print("[C10] disk.img conservé (pas de dd — requis pour Phase B)")
        else:
            print("\n[C10] C10_NEG_FAIL: écriture réussie sur page RX — W^X non actif", file=sys.stderr)
            sys.exit(1)

    except pexpect.EOF:
        child.close()
        print("\n[C10] Phase A FAIL: QEMU terminé sans résultat attendu", file=sys.stderr)
        sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C10] Phase A FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)

elif mode == "phase-b":
    print(f"[C10] Phase B — D-reopen (disk.img NON recréé)")

    child = pexpect.spawn(qemu_cmd(IMAGE_B), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        idx = child.expect(["C10_REOPEN_PASS", "C10_REOPEN_FAIL"])
        child.terminate(force=True)
        child.close()
        if idx == 0:
            print("\nC10_REOPEN_PASS ✓")
            sys.exit(0)
        else:
            print("\nC10_REOPEN_FAIL: vérification reopen échouée", file=sys.stderr)
            sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C10] Phase B FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)
    except pexpect.EOF:
        print("\n[C10] Phase B FAIL: QEMU exited avant résultat", file=sys.stderr)
        sys.exit(1)

else:
    print(f"Usage: test.py [phase-a | phase-b]", file=sys.stderr)
    sys.exit(2)
