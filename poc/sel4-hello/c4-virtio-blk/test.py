#!/usr/bin/env python3
# Jalon C.4 — test automatisé : attend C4_PASS sur UART QEMU (timeout 30 s)
import sys
import pexpect


def main() -> None:
    if len(sys.argv) < 2:
        print("Usage: test.py <qemu_command>", file=sys.stderr)
        sys.exit(1)

    qemu_cmd = sys.argv[1]
    success_pattern = "C4_PASS"
    timeout_s = 30

    print(f"[C.4] Lancement QEMU : {qemu_cmd}")
    print(f"[C.4] Attente de '{success_pattern}' (timeout {timeout_s} s)...")

    child = pexpect.spawn(qemu_cmd, encoding="utf-8", timeout=timeout_s)
    child.logfile_read = sys.stdout

    index = child.expect([success_pattern, pexpect.EOF, pexpect.TIMEOUT])

    child.close()

    if index == 0:
        print(f"\n[C.4] PASS : '{success_pattern}' reçu sur UART seL4")
        sys.exit(0)
    elif index == 1:
        print(f"\n[C.4] FAIL : QEMU a terminé sans '{success_pattern}'", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"\n[C.4] FAIL : timeout {timeout_s} s sans '{success_pattern}'", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
