#!/usr/bin/env python3
"""Test runner C.6 — attend C6_PASS sur UART QEMU (pexpect, timeout 300 s).

Timeout élevé car Wasmtime AOT compile le module WASM dans le build,
mais l'instanciation au runtime peut prendre du temps sur QEMU.
"""

import sys
import os
import pexpect

IMAGE = os.environ.get("IMAGE", "/work/build/image.elf")
TIMEOUT = 300

qemu_cmd = (
    f"qemu-system-aarch64 "
    f"-machine virt,virtualization=on "
    f"-cpu cortex-a57 "
    f"-m 1G "
    f"-serial mon:stdio "
    f"-nographic "
    f"-kernel {IMAGE}"
)

print(f"[C.6] IMAGE={IMAGE}")
print(f"[C.6] QEMU : {qemu_cmd}")
print(f"[C.6] Attente de C6_PASS (timeout {TIMEOUT} s)...")
sys.stdout.flush()

child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=TIMEOUT)
child.logfile_read = sys.stdout

idx = child.expect(["C6_PASS", pexpect.EOF, pexpect.TIMEOUT])
child.close()

if idx == 0:
    print("\n[C.6] PASS")
    sys.exit(0)
elif idx == 1:
    print("\n[C.6] FAIL : QEMU terminé sans C6_PASS", file=sys.stderr)
    sys.exit(1)
else:
    print(f"\n[C.6] FAIL : timeout {TIMEOUT} s sans C6_PASS", file=sys.stderr)
    sys.exit(1)
