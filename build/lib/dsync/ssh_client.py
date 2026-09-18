import logging
import os
import socket
import subprocess
from dataclasses import dataclass
from typing import Optional

from .retry import with_retry

logger = logging.getLogger(__name__)


def check_port(ip: str, port: int = 22, timeout: float = 2) -> bool:
    try:
        with socket.create_connection((ip, port), timeout=timeout):
            return True
    except (socket.timeout, ConnectionRefusedError, OSError):
        return False


@dataclass
class SSHResult:
    stdout: str
    stderr: str
    returncode: int
    success: bool

    @property
    def is_transient(self) -> bool:
        """True if the failure looks like a transient network issue worth retrying."""
        text = (self.stderr + self.stdout).lower()
        transient = (
            "timed out",
            "timeout",
            "connection refused",
            "no route to host",
            "connection reset",
            "temporarily unavailable",
        )
        return any(t in text for t in transient)


def run(
    host: str,
    command: str,
    user: str = "mflkee",
    port: int = 22,
    timeout: int = 30,
    identity_file: Optional[str] = None,
    retries: int = 1,
) -> SSHResult:
    def _attempt() -> SSHResult:
        ssh_cmd = [
            "ssh",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "BatchMode=yes",
            "-p",
            str(port),
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

    if retries <= 1:
        return _attempt()
    logger.debug("SSH to %s@%s:%s with %d retries", user, host, port, retries)
    return with_retry(
        _attempt, lambda r: not r.success and r.is_transient, attempts=retries
    )


def check_connectivity(host: str, user: str = "mflkee") -> bool:
    result = run(host, "echo ok", user=user, timeout=15)
    return result.success
