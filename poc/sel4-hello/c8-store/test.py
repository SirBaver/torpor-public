#!/usr/bin/env python3
"""Test runner C.8 — un kill_point à la fois.

Lance QEMU pour une image donnée (KP=1|2|3|4), attend KPx_PASS sur l'UART.
Timeout élevé : Wasmtime AOT + init redb/virtio-blk au démarrage.

Note : disk.img est recréé vide avant chaque run pour garantir un état propre
(ADR-0045 : chaque run KP est indépendant, pas de rollback cross-run requis).
"""

import sys
import os
import subprocess
import pexpect

IMAGE    = os.environ.get("IMAGE",    "/poc/sel4-hello/c8-store/build/image-kp1.elf")
KP       = os.environ.get("KP",       "1")
DISK_IMG = os.environ.get("DISK_IMG", "/poc/sel4-hello/c8-store/build/disk.img")
EXPECTED = f"KP{KP}_PASS"
TIMEOUT  = 300

# Recréer le disk.img vide avant chaque run (état propre pour redb)
subprocess.run(
    ["dd", "if=/dev/zero", f"of={DISK_IMG}", "bs=1M", "count=8"],
    check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
)

qemu_cmd = (
    f"qemu-system-aarch64 "
    f"-machine virt,virtualization=on "
    f"-cpu cortex-a57 "
    f"-m 1G "
    f"-serial mon:stdio "
    f"-nographic "
    f"-drive if=none,id=blk0,file={DISK_IMG},format=raw,cache=none,aio=native "
    f"-device virtio-blk-device,drive=blk0 "
    f"-kernel {IMAGE}"
)

print(f"[C8] KP={KP} IMAGE={IMAGE}")
print(f"[C8] Attente de {EXPECTED} (timeout {TIMEOUT}s)...")
sys.stdout.flush()

child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=TIMEOUT)
child.logfile_read = sys.stdout

idx = child.expect([EXPECTED, pexpect.EOF, pexpect.TIMEOUT])
child.close()

if idx == 0:
    print(f"\n[C8] KP{KP} PASS")
    sys.exit(0)
elif idx == 1:
    print(f"\n[C8] FAIL: QEMU terminé sans {EXPECTED}", file=sys.stderr)
    sys.exit(1)
else:
    print(f"\n[C8] FAIL: timeout {TIMEOUT}s sans {EXPECTED}", file=sys.stderr)
    sys.exit(1)
