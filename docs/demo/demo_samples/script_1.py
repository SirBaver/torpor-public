import subprocess

def ping_host(hostname):
    return subprocess.check_output(f"ping -c 1 {hostname}", shell=True)