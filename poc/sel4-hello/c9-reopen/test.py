#!/usr/bin/env python3
"""Test runner C.9 — smoke test persistance seL4 (D-reopen, ADR-0046).

Deux runs QEMU sur le même disk.img :
  Phase A : K=100 commits → REOPEN_A_PASS (disk.img créé une seule fois)
  Phase B : reopen disk.img (sans dd) → vérification → C9_PASS

Usage interne : python3 test.py [phase-a | phase-b]
"""

import sys
import os
import subprocess
import pexpect

BUILD    = os.environ.get("BUILD",    "/poc/sel4-hello/c9-reopen/build")
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
    print(f"[C9] Phase A — écriture K=100 commits sur {DISK_IMG}")
    # disk.img créé ici (et UNIQUEMENT ici — pas recréé avant Phase B)
    subprocess.run(
        ["dd", "if=/dev/zero", f"of={DISK_IMG}", "bs=1M", "count=8"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    print(f"[C9] disk.img créé (8 MB)")

    child = pexpect.spawn(qemu_cmd(IMAGE_A), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout
    child.expect("REOPEN_A_PASS")
    child.terminate(force=True)
    child.close()
    print("\n[C9] Phase A terminée — disk.img conservé (pas de dd)")

elif mode == "phase-b":
    print(f"[C9] Phase B — vérification persistance sur {DISK_IMG} (disk.img NON recréé)")
    # disk.img N'EST PAS recréé ici — c'est le point du test.

    child = pexpect.spawn(qemu_cmd(IMAGE_B), encoding="utf-8", timeout=TIMEOUT)
    child.logfile = sys.stdout
    try:
        idx = child.expect(["C9_PASS", "C9_FAIL"])
        child.terminate(force=True)
        child.close()
        if idx == 0:
            print("\nC9_PASS")
            sys.exit(0)
        else:
            print("\nC9_FAIL: vérification reopen échouée")
            sys.exit(1)
    except pexpect.TIMEOUT:
        child.terminate(force=True)
        print("\nC9_FAIL: timeout Phase B")
        sys.exit(1)
    except pexpect.EOF:
        print("\nC9_FAIL: QEMU exited avant résultat")
        sys.exit(1)

else:
    print(f"Usage: test.py [phase-a | phase-b]", file=sys.stderr)
    sys.exit(2)
