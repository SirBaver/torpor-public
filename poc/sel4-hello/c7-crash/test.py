#!/usr/bin/env python3
"""Test runner C.7-crash — un kill_point à la fois.

Lance QEMU pour une image donnée (KP=1|2|3|4), attend KPx_PASS sur l'UART.
Timeout élevé car Wasmtime AOT instancie le module au runtime sur QEMU.
"""

import sys
import os
import pexpect

IMAGE = os.environ.get("IMAGE", "/poc/sel4-hello/c7-crash/build/image-kp1.elf")
KP    = os.environ.get("KP", "1")
EXPECTED = f"KP{KP}_PASS"
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

print(f"[C7-crash] KP={KP} IMAGE={IMAGE}")
print(f"[C7-crash] Attente de {EXPECTED} (timeout {TIMEOUT}s)...")
sys.stdout.flush()

child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=TIMEOUT)
child.logfile_read = sys.stdout

idx = child.expect([EXPECTED, pexpect.EOF, pexpect.TIMEOUT])
child.close()

if idx == 0:
    print(f"\n[C7-crash] KP{KP} PASS")
    sys.exit(0)
elif idx == 1:
    print(f"\n[C7-crash] FAIL: QEMU terminé sans {EXPECTED}", file=sys.stderr)
    sys.exit(1)
else:
    print(f"\n[C7-crash] FAIL: timeout {TIMEOUT}s sans {EXPECTED}", file=sys.stderr)
    sys.exit(1)
