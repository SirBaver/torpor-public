#!/usr/bin/env python3
"""Test runner C.11-prov — axe provenance P-δ (ADR-0048 §D1).

Phase A (PHASE=0) : P-δ (malformed) + valid (happy path)
    → attend C11PROV_DELTA_PASS
    → attend C11PROV_VALID_PASS
    → attend C11PROV_A_PASS
Phase B (PHASE=1) : D-reopen (disk.img NON recréé)
    → attend C11PROV_GAMMA_PASS
    → attend C11PROV_PASS

C11PROV_PASS = C11PROV_A_PASS + C11PROV_GAMMA_PASS.

Usage : python3 test.py [phase-a | phase-b]
"""

import sys
import os
import subprocess
import pexpect

BUILD    = os.environ.get("BUILD",    "/poc/sel4-hello/c11-prov/build")
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
    print("[C11prov] Phase A — P-delta (malformed) + valid (happy path)")
    subprocess.run(
        ["dd", "if=/dev/zero", f"of={DISK_IMG}", "bs=1M", "count=8"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    print("[C11prov] disk.img créé (8 MB)")

    child = pexpect.spawn(qemu_cmd(IMAGE_A), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        child.expect("C11PROV_DELTA_PASS", timeout=TIMEOUT)
        print("\n[C11prov] C11PROV_DELTA_PASS (Module::deserialize Err détecté — P-delta)")

        child.expect("C11PROV_VALID_PASS", timeout=TIMEOUT)
        print("\n[C11prov] C11PROV_VALID_PASS (cwasm valide depuis canal non-trusted)")

        child.expect("C11PROV_A_PASS", timeout=TIMEOUT)
        child.terminate(force=True)
        child.close()
        print("\n[C11prov] C11PROV_A_PASS — Phase A : P-delta + valid PASS")
        print("[C11prov] disk.img conservé (requis pour Phase B)")

    except pexpect.EOF:
        child.close()
        print("\n[C11prov] Phase A FAIL: QEMU terminé sans résultat", file=sys.stderr)
        sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C11prov] Phase A FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)

elif mode == "phase-b":
    print("[C11prov] Phase B — D-reopen (disk.img NON recréé)")

    child = pexpect.spawn(qemu_cmd(IMAGE_B), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        child.expect("C11PROV_GAMMA_PASS", timeout=TIMEOUT)
        print("\n[C11prov] C11PROV_GAMMA_PASS (K=1 commit vérifié sur disk.img existant)")

        child.expect("C11PROV_PASS", timeout=60)
        child.terminate(force=True)
        child.close()
        print("\nC11PROV_PASS")
        sys.exit(0)

    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C11prov] Phase B FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)
    except pexpect.EOF:
        print("\n[C11prov] Phase B FAIL: QEMU exited avant résultat", file=sys.stderr)
        sys.exit(1)

else:
    print("Usage: test.py [phase-a | phase-b]", file=sys.stderr)
    sys.exit(2)
