import os
import subprocess
from dataclasses import dataclass
from typing import Optional


@dataclass
class SSHResult:
    stdout: str
    stderr: str
    returncode: int
    success: bool


def run(
    host: str,
    command: str,
    user: str = "mflkee",
    port: int = 22,
    timeout: int = 30,
    identity_file: Optional[str] = None,
) -> SSHResult:
    ssh_cmd = [
        "ssh",
        "-o", "ConnectTimeout=10",
        "-o", "StrictHostKeyChecking=accept-new",
        "-o", "BatchMode=yes",
        "-p", str(port),
    ]
    if identity_file:
        ssh_cmd.extend(["-i", identity_file])
    ssh_cmd.append(f"{user}@{host}")
    ssh_cmd.append(command)

    env = os.environ.copy()
    env.update({"LC_ALL": "C", "LANG": "C"})

    try:
        result = subprocess.run(
            ssh_cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        return SSHResult(
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
            success=result.returncode == 0,
        )
    except subprocess.TimeoutExpired:
        return SSHResult(
            stdout="",
            stderr="SSH connection timed out",
            returncode=-1,
            success=False,
        )
    except FileNotFoundError:
        return SSHResult(
            stdout="",
            stderr="ssh command not found",
            returncode=-2,
            success=False,
        )


def check_connectivity(host: str, user: str = "mflkee") -> bool:
    result = run(host, "echo ok", user=user, timeout=15)
    return result.success
