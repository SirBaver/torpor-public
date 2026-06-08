#!/usr/bin/env python3
"""Test runner C.11 — WASM non confié sur JIT durci seL4 (ADR-0048).

Deux runs QEMU sur le même disk.img :
  Phase A (PHASE=0) : P-alpha (OOB trap) + P-beta (boucle infinie watchdog)
    → attend C11_ALPHA_PASS
    → attend C11_BETA_PASS
    → attend C11_AB_PASS
  Phase B (PHASE=1) : D-reopen (disk.img NON recréé)
    → attend C11_GAMMA_PASS
    → attend C11_PASS

C11_PASS = C11_AB_PASS + C11_GAMMA_PASS (ADR-0048 §D2).

Usage : python3 test.py [phase-a | phase-b]
"""

import sys
import os
import subprocess
import pexpect

BUILD    = os.environ.get("BUILD",    "/poc/sel4-hello/c11-untrusted/build")
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
    print(f"[C11] Phase A — P-alpha (OOB) + P-beta (LOOP watchdog)")
    # disk.img créé ici (et UNIQUEMENT ici — pas recréé avant Phase B)
    subprocess.run(
        ["dd", "if=/dev/zero", f"of={DISK_IMG}", "bs=1M", "count=8"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    print(f"[C11] disk.img créé (8 MB)")

    child = pexpect.spawn(qemu_cmd(IMAGE_A), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        child.expect("C11_ALPHA_PASS", timeout=TIMEOUT)
        print("\n[C11] C11_ALPHA_PASS (OOB trap isolé — P-alpha)")

        child.expect("C11_BETA_PASS", timeout=TIMEOUT)
        print("\n[C11] C11_BETA_PASS (boucle infinie stoppée par watchdog — P-beta)")

        child.expect("C11_AB_PASS", timeout=TIMEOUT)
        child.terminate(force=True)
        child.close()
        print("\n[C11] C11_AB_PASS — Phase A : P-alpha + P-beta PASS")
        print("[C11] disk.img conservé (pas de dd — requis pour Phase B)")

    except pexpect.EOF:
        child.close()
        print("\n[C11] Phase A FAIL: QEMU terminé sans résultat attendu", file=sys.stderr)
        sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C11] Phase A FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)

elif mode == "phase-b":
    print(f"[C11] Phase B — D-reopen (disk.img NON recréé)")

    child = pexpect.spawn(qemu_cmd(IMAGE_B), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        child.expect("C11_GAMMA_PASS", timeout=TIMEOUT)
        print("\n[C11] C11_GAMMA_PASS (K=2 commits vérifiés sur disk.img existant)")

        child.expect("C11_PASS", timeout=60)
        child.terminate(force=True)
        child.close()
        print("\nC11_PASS")
        sys.exit(0)

    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C11] Phase B FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)
    except pexpect.EOF:
        print("\n[C11] Phase B FAIL: QEMU exited avant résultat", file=sys.stderr)
        sys.exit(1)

else:
    print(f"Usage: test.py [phase-a | phase-b]", file=sys.stderr)
    sys.exit(2)
