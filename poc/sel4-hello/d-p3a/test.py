#!/usr/bin/env python3
"""D-P3a — latence P3a sous seL4/NVMe (ADR-0045 amendement Q1, ADR-0046).

Attend D_P3A_PASS ou D_P3A_FAIL sur UART QEMU.
Timeout élevé : population 10^6 entrées via O_DIRECT (~15–30 min) + 3 passes mesure.

Critère recevabilité (ADR-0046) : disk.img ouvert avec cache=none,aio=native
(O_DIRECT sur NVMe hôte — page cache bypassed).
"""

import sys
import os
import re
import pexpect

IMAGE   = os.environ.get("IMAGE",   "/poc/sel4-hello/d-p3a/build/image.elf")
DISK    = os.environ.get("DISK",    "/poc/sel4-hello/d-p3a/disk.img")
TIMEOUT = 1800  # 30 min

qemu_cmd = (
    f"qemu-system-aarch64 "
    f"-machine virt,virtualization=on "
    f"-cpu cortex-a57 "
    f"-m 1G "
    f"-serial mon:stdio "
    f"-nographic "
    f"-drive if=none,id=blk0,file={DISK},format=raw,cache=none,aio=native "
    f"-device virtio-blk-device,drive=blk0 "
    f"-kernel {IMAGE}"
)

print(f"[d-p3a] IMAGE={IMAGE}")
print(f"[d-p3a] DISK={DISK}")
print(f"[d-p3a] timeout={TIMEOUT}s")
sys.stdout.flush()

child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=TIMEOUT)
child.logfile_read = sys.stdout

try:
    idx = child.expect(["D_P3A_PASS", "D_P3A_FAIL", pexpect.EOF, pexpect.TIMEOUT])
    output = child.before or ""
    child.terminate(force=True)
    child.close()

    # Extraire les p99 de chaque passe
    p99s = re.findall(r"p99=(\d+)us", output)

    if idx == 0:
        print(f"\n[d-p3a] D_P3A_PASS — p99 passes: {p99s} µs (cible ≤ 10000 µs)")
        sys.exit(0)
    elif idx == 1:
        print(f"\n[d-p3a] D_P3A_FAIL — p99 passes: {p99s} µs (cible ≤ 10000 µs)")
        print("[d-p3a] Note : FAIL attendu sous compaction redb active (cohérent ADR-0032).")
        print("[d-p3a] Voir VERDICT.md pour interprétation par dimension.")
        sys.exit(1)
    elif idx == 2:
        print("\n[d-p3a] FAIL : QEMU terminé sans verdict", file=sys.stderr)
        sys.exit(2)
    else:
        print(f"\n[d-p3a] FAIL : timeout {TIMEOUT}s sans verdict", file=sys.stderr)
        sys.exit(2)

except pexpect.EOF:
    child.close()
    print("\n[d-p3a] FAIL : EOF inattendu", file=sys.stderr)
    sys.exit(2)
