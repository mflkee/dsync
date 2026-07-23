"""Obsidian REST API health monitoring and auto-recovery.

Provides health checks and auto-recovery for headless Obsidian
instances running on remote machines via SSH.
"""

import json
import logging
import os
from dataclasses import dataclass

from .ssh_client import run as ssh_run

logger = logging.getLogger(__name__)

# Default API key - can be overridden via OBSIDIAN_API_KEY env var
DEFAULT_API_KEY = "0acede40d4e8d3613627b5ad554c1cb6ca53aef79ca289f6dd78f0df468139bb"

# Check if Obsidian REST API is responding
def _check_api_cmd(api_key: str) -> str:
    return (
        f"curl -s -o /dev/null -w '%{{http_code}}' "
        f"-H 'Authorization: Bearer {api_key}' "
        f"http://127.0.0.1:27123/vault/ 2>/dev/null || echo '000'"
    )

# Check if obsidian.service is active
_CHECK_SERVICE = (
    "systemctl --user is-active obsidian.service 2>/dev/null || echo 'inactive'"
)

# Restart obsidian.service
_RESTART_SERVICE = (
    "systemctl --user restart obsidian.service && sleep 10"
    " && systemctl --user is-active obsidian.service 2>/dev/null || echo 'failed'"
)

# Pull vault changes and restart
_PULL_AND_RESTART = (
    "cd ~/obs_main && git pull 2>&1 && systemctl --user restart obsidian.service"
    " && sleep 10 && systemctl --user is-active obsidian.service 2>/dev/null || echo 'failed'"
)


@dataclass
class ObsidianStatus:
    """Status of Obsidian REST API on a remote machine."""

    api_responsive: bool
    service_active: bool
    http_code: int = 0
    error: str = ""


def check_api(
    ip: str,
    user: str = "mflkee",
    api_key: str = "",
    timeout: int = 15,
) -> ObsidianStatus:
    """Check if Obsidian REST API is responding on a remote machine."""
    logger.debug("check obsidian API on %s", ip)

    if not api_key:
        api_key = os.environ.get("OBSIDIAN_API_KEY", DEFAULT_API_KEY)
    cmd = _check_api_cmd(api_key)
    r = ssh_run(ip, cmd, user=user, timeout=timeout)
    if not r.success:
        logger.info("obsidian API check failed on %s: %s", ip, r.stderr[:100])
        return ObsidianStatus(
            api_responsive=False,
            service_active=False,
            error=r.stderr[:200],
        )

    try:
        http_code = int(r.stdout.strip())
        api_responsive = http_code == 200
    except (ValueError, TypeError):
        http_code = 0
        api_responsive = False

    # Also check service status
    r_svc = ssh_run(ip, _CHECK_SERVICE, user=user, timeout=timeout)
    service_active = (
        r_svc.success and r_svc.stdout.strip() == "active"
    )

    if api_responsive:
        logger.info("obsidian API OK on %s (HTTP %d)", ip, http_code)
    else:
        logger.warning(
            "obsidian API unhealthy on %s: HTTP %d, service=%s",
            ip,
            http_code,
            "active" if service_active else "inactive",
        )

    return ObsidianStatus(
        api_responsive=api_responsive,
        service_active=service_active,
        http_code=http_code,
    )


def restart(
    ip: str,
    user: str = "mflkee",
    pull: bool = False,
    timeout: int = 30,
) -> bool:
    """Restart Obsidian service on a remote machine.

    Args:
        ip: Target machine IP
        user: SSH user
        pull: If True, pull vault changes before restart
        timeout: SSH timeout in seconds

    Returns:
        True if service is active after restart
    """
    logger.info("restarting obsidian on %s (pull=%s)", ip, pull)
    cmd = _PULL_AND_RESTART if pull else _RESTART_SERVICE
    r = ssh_run(ip, cmd, user=user, timeout=timeout)
    if r.success and "active" in r.stdout:
        logger.info("obsidian restarted OK on %s", ip)
        return True
    logger.warning("obsidian restart failed on %s: %s", ip, r.stderr[:100])
    return False


def health_check(
    ip: str,
    user: str = "mflkee",
    api_key: str = "",
    auto_restart: bool = True,
    pull: bool = False,
) -> ObsidianStatus:
    """Full health check: API responsiveness + auto-recovery.

    Args:
        ip: Target machine IP
        user: SSH user
        api_key: Obsidian API key for auth
        auto_restart: Restart service if not responding
        pull: Pull vault changes before restart

    Returns:
        ObsidianStatus with current state
    """
    status = check_api(ip, user=user, api_key=api_key)

    if not status.api_responsive and auto_restart:
        if restart(ip, user=user, pull=pull):
            status = check_api(ip, user=user, api_key=api_key)

    return status
