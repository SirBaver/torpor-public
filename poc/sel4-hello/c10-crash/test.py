#!/usr/bin/env python3
"""Test runner C.10-crash — Atomicité crash dans la fenêtre de remap W→X (ADR-0047 §D7).

Deux runs QEMU sur le même disk.img :
  Phase A (PHASE=0) : K=1 commit + crash dans fenêtre de remap W→X
    → attend KP_WX_PASS (store intact après crash dans fenêtre de remap)
    → attend C10_CRASH_PASS
  Phase B (PHASE=1) : D-reopen (disk.img NON recréé)
    → attend C10_CRASH_REOPEN_PASS

C10-crash PASS = KP_WX_PASS + C10_CRASH_PASS + C10_CRASH_REOPEN_PASS

Usage : python3 test.py [phase-a | phase-b]
"""

import sys
import os
import subprocess
import pexpect

BUILD    = os.environ.get("BUILD",    "/poc/sel4-hello/c10-crash/build")
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
    print(f"[C10-crash] Phase A — K=1 commit + crash dans fenêtre de remap W→X")
    # disk.img créé ici (et UNIQUEMENT ici — pas recréé avant Phase B)
    subprocess.run(
        ["dd", "if=/dev/zero", f"of={DISK_IMG}", "bs=1M", "count=8"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    print(f"[C10-crash] disk.img créé (8 MB)")

    child = pexpect.spawn(qemu_cmd(IMAGE_A), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        child.expect("KP_WX_PASS")
        print("\n[C10-crash] KP_WX_PASS : store intact apres crash dans fenetre de remap")

        child.expect("C10_CRASH_PASS")
        print("\n[C10-crash] C10_CRASH_PASS : atomicite crash confirmee")

        child.terminate(force=True)
        child.close()
        print("[C10-crash] Phase A : C10_CRASH_PASS OK")
        print("[C10-crash] disk.img conserve (requis pour Phase B)")

    except pexpect.EOF:
        child.close()
        print("\n[C10-crash] Phase A FAIL: QEMU termine sans resultat attendu", file=sys.stderr)
        sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C10-crash] Phase A FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)

elif mode == "phase-b":
    print(f"[C10-crash] Phase B — D-reopen (disk.img NON recree)")

    child = pexpect.spawn(qemu_cmd(IMAGE_B), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout

    try:
        idx = child.expect(["C10_CRASH_REOPEN_PASS", "C10_CRASH_REOPEN_FAIL"])
        child.terminate(force=True)
        child.close()
        if idx == 0:
            print("\nC10_CRASH_REOPEN_PASS OK")
            sys.exit(0)
        else:
            print("\nC10_CRASH_REOPEN_FAIL: verification reopen echouee", file=sys.stderr)
            sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print(f"\n[C10-crash] Phase B FAIL: timeout {TIMEOUT}s", file=sys.stderr)
        sys.exit(1)
    except pexpect.EOF:
        print("\n[C10-crash] Phase B FAIL: QEMU exited avant resultat", file=sys.stderr)
        sys.exit(1)

else:
    print(f"Usage: test.py [phase-a | phase-b]", file=sys.stderr)
    sys.exit(2)
