#!/usr/bin/env python3
"""Test runner C.7-A — attend C7-A_PASS sur UART QEMU (pexpect, timeout 300 s).

Timeout élevé car Wasmtime AOT compile le module WASM dans le build,
mais l'instanciation au runtime peut prendre du temps sur QEMU.
"""

import sys
import os
import pexpect

IMAGE = os.environ.get("IMAGE", "/work/build/image.elf")
EXPECTED = os.environ.get("EXPECTED", "C7-A_PASS")
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

print(f"[C.7] IMAGE={IMAGE}")
print(f"[C.7] QEMU : {qemu_cmd}")
print(f"[C.7] Attente de {EXPECTED} (timeout {TIMEOUT} s)...")
sys.stdout.flush()

child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=TIMEOUT)
child.logfile_read = sys.stdout

idx = child.expect([EXPECTED, pexpect.EOF, pexpect.TIMEOUT])
child.close()

if idx == 0:
    print(f"\n[C.7] PASS : {EXPECTED} reçu")
    sys.exit(0)
elif idx == 1:
    print(f"\n[C.7] FAIL : QEMU terminé sans {EXPECTED}", file=sys.stderr)
    sys.exit(1)
else:
    print(f"\n[C.7] FAIL : timeout {TIMEOUT} s sans {EXPECTED}", file=sys.stderr)
    sys.exit(1)
