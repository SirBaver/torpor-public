#!/usr/bin/env python3
"""Test runner C.5 — attend C5_PASS sur UART QEMU (pexpect, timeout 120 s)."""

import sys
import os
import pexpect

IMAGE = os.environ.get("IMAGE", "/work/build/image.elf")
DISK  = os.environ.get("DISK",  "/work/disk.img")
TIMEOUT = 120

qemu_cmd = (
    f"qemu-system-aarch64 "
    f"-machine virt,virtualization=on "
    f"-cpu cortex-a57 "
    f"-m 1G "
    f"-serial mon:stdio "
    f"-nographic "
    f"-drive if=none,id=blk0,file={DISK},format=raw "
    f"-device virtio-blk-device,drive=blk0 "
    f"-kernel {IMAGE}"
)

print(f"[C.5] IMAGE={IMAGE}")
print(f"[C.5] DISK={DISK}")
print(f"[C.5] QEMU : {qemu_cmd}")
print(f"[C.5] Attente de C5_PASS (timeout {TIMEOUT} s)...")
sys.stdout.flush()

child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=TIMEOUT)
child.logfile_read = sys.stdout

idx = child.expect(["C5_PASS", pexpect.EOF, pexpect.TIMEOUT])
child.close()

if idx == 0:
    print("\n[C.5] PASS")
    sys.exit(0)
elif idx == 1:
    print("\n[C.5] FAIL : QEMU terminé sans C5_PASS", file=sys.stderr)
    sys.exit(1)
else:
    print(f"\n[C.5] FAIL : timeout {TIMEOUT} s sans C5_PASS", file=sys.stderr)
    sys.exit(1)
