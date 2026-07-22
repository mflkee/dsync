"""Syncthing health monitoring and auto-recovery.

Provides health checks, auto-restart, and conflict resolution
for Syncthing instances on remote machines via SSH.

All operations run via SSH - syncthing CLI is used instead of API
since the API may be bound to localhost only.
"""

import json
import logging
from dataclasses import dataclass

from .ssh_client import run as ssh_run

logger = logging.getLogger(__name__)

# Remote commands executed via SSH
_CHECK_RUNNING = (
    "pgrep -x syncthing &>/dev/null"
    " && syncthing cli show system 2>/dev/null | python3 -c"
    ' "import sys,json; d=json.load(sys.stdin);'
    " print(json.dumps({'ok':True,'uptime':d.get('uptime',0),"
    " 'myID':d.get('myID','')[:12]}))\""
    " || echo '{\"ok\":false}'"
)

_CHECK_CONFLICTS = (
    'syncthing cli show folders 2>/dev/null | python3 -c "'
    "import sys,json;"
    " data=json.load(sys.stdin);"
    " out=[];"
    " for f in data.get('folders',[]):"
    "   fs=f.get('filesystem',{});"
    "   path=fs.get('path','');"
    "   conflicts=fs.get('conflicts',0);"
    "   if conflicts>0:"
    "     out.append({'id':f.get('id',''),'path':path,'conflicts':conflicts});"
    ' print(json.dumps(out))"'
    " || echo '[]'"
)

_RESOLVE_CONFLICTS = (
    'syncthing cli show folders 2>/dev/null | python3 -c "'
    "import sys,json,subprocess,os,glob;"
    " data=json.load(sys.stdin);"
    " resolved=0;"
    " for f in data.get('folders',[]):"
    "   fs=f.get('filesystem',{});"
    "   path=fs.get('path','');"
    "   pattern=os.path.join(path,'**/*sync-conflict-*');"
    "   for conflict in glob.glob(pattern, recursive=True):"
    "     base=conflict.rsplit('.sync-conflict-',1)[0];"
    "     import time;"
    "     suffix=f'.sync-archived-{int(time.time())}';"
    "     os.rename(conflict, conflict+suffix);"
    "     resolved+=1;"
    " print(json.dumps({'resolved':resolved}))\""
    " || echo '{\"resolved\":0}'"
)

_RESTART = "pkill -x syncthing; sleep 1; nohup syncthing serve --no-browser --no-restart &>/dev/null & sleep 3; pgrep -x syncthing &>/dev/null && echo 'active' || echo 'inactive'"


@dataclass
class SyncthingStatus:
    """Status of a syncthing instance on a remote machine."""

    running: bool
    uptime: int = 0
    device_id: str = ""
    conflicts: list[dict] | None = None
    error: str = ""


def check_running(ip: str, user: str = "mflkee", timeout: int = 15) -> SyncthingStatus:
    """Check if syncthing is running on a remote machine."""
    logger.debug("check syncthing on %s", ip)
    r = ssh_run(ip, _CHECK_RUNNING, user=user, timeout=timeout)
    if not r.success:
        logger.info("syncthing not running on %s", ip)
        return SyncthingStatus(running=False, error=r.stderr[:200])
    try:
        data = json.loads(r.stdout.strip())
        running = data.get("ok", False)
        if running:
            logger.info("syncthing OK on %s (uptime %ds)", ip, data.get("uptime", 0))
        return SyncthingStatus(
            running=running,
            uptime=data.get("uptime", 0),
            device_id=data.get("myID", ""),
        )
    except json.JSONDecodeError:
        return SyncthingStatus(running=False, error="parse error")


def check_conflicts(ip: str, user: str = "mflkee", timeout: int = 15) -> list[dict]:
    """Check for sync conflicts on a remote machine."""
    logger.debug("check syncthing conflicts on %s", ip)
    r = ssh_run(ip, _CHECK_CONFLICTS, user=user, timeout=timeout)
    if not r.success:
        logger.warning("conflict check failed on %s: %s", ip, r.stderr[:100])
        return []
    try:
        conflicts = json.loads(r.stdout.strip())
        if conflicts:
            logger.warning(
                "syncthing conflicts on %s: %s",
                ip,
                [(c["id"], c["conflicts"]) for c in conflicts],
            )
        return conflicts
    except json.JSONDecodeError:
        return []


def resolve_conflicts(ip: str, user: str = "mflkee", timeout: int = 30) -> int:
    """Auto-resolve syncthing conflicts by archiving conflicting files.

    Returns the number of conflicts resolved.
    """
    logger.debug("resolve syncthing conflicts on %s", ip)
    r = ssh_run(ip, _RESOLVE_CONFLICTS, user=user, timeout=timeout)
    if not r.success:
        logger.warning("conflict resolve failed on %s: %s", ip, r.stderr[:100])
        return 0
    try:
        data = json.loads(r.stdout.strip())
        resolved = data.get("resolved", 0)
        if resolved:
            logger.info("resolved %d syncthing conflicts on %s", resolved, ip)
        return resolved
    except json.JSONDecodeError:
        return 0


def restart(ip: str, user: str = "mflkee", timeout: int = 30) -> bool:
    """Restart syncthing on a remote machine and verify it's running."""
    logger.info("restarting syncthing on %s", ip)
    r = ssh_run(ip, _RESTART, user=user, timeout=timeout)
    if r.success and "active" in r.stdout:
        logger.info("syncthing restarted OK on %s", ip)
        return True
    logger.warning("syncthing restart failed on %s: %s", ip, r.stderr[:100])
    return False


def health_check(
    ip: str,
    user: str = "mflkee",
    auto_restart: bool = True,
    auto_resolve: bool = True,
) -> SyncthingStatus:
    """Full health check: running status + conflicts + auto-recovery.

    Args:
        ip: Target machine IP
        user: SSH user
        auto_restart: Restart syncthing if not running
        auto_resolve: Auto-resolve conflicts by archiving

    Returns:
        SyncthingStatus with conflict info populated
    """
    status = check_running(ip, user=user)

    if not status.running and auto_restart:
        if restart(ip, user=user):
            status = check_running(ip, user=user)

    if status.running:
        status.conflicts = check_conflicts(ip, user=user)
        if status.conflicts and auto_resolve:
            resolved = resolve_conflicts(ip, user=user)
            if resolved:
                status.conflicts = check_conflicts(ip, user=user)

    return status


# Script for remote execution via SSH (used by dsync sync)
REMOTE_HEALTH_SCRIPT = """\
#!/usr/bin/env bash
set -euo pipefail

AUTO_RESTART="${1:-true}"
AUTO_RESOLVE="${2:-true}"

# Check running
if ! pgrep -x syncthing &>/dev/null; then
    if [ "$AUTO_RESTART" = "true" ]; then
        nohup syncthing serve --no-browser --no-restart &>/dev/null &
        sleep 3
        if ! pgrep -x syncthing &>/dev/null; then
            echo "HEALTH_ERROR|syncthing restart failed"
            exit 1
        fi
        echo "HEALTH_RESTARTED|syncthing restarted"
    else
        echo "HEALTH_ERROR|syncthing not running"
        exit 1
    fi
fi

# Check conflicts
CONFLICTS=$(syncthing cli show folders 2>/dev/null | python3 -c "
import sys,json,os,glob
data=json.load(sys.stdin)
total=0
for f in data.get('folders',[]):
    path=f.get('filesystem',{}).get('path','')
    for c in glob.glob(os.path.join(path,'**/*sync-conflict-*'), recursive=True):
        total+=1
print(total)
" 2>/dev/null || echo "0")

if [ "$CONFLICTS" -gt 0 ] && [ "$AUTO_RESOLVE" = "true" ]; then
    syncthing cli show folders 2>/dev/null | python3 -c "
import sys,json,os,glob,time
data=json.load(sys.stdin)
resolved=0
for f in data.get('folders',[]):
    path=f.get('filesystem',{}).get('path','')
    for c in glob.glob(os.path.join(path,'**/*sync-conflict-*'), recursive=True):
        os.rename(c, c+f'.sync-archived-{int(time.time())}')
        resolved+=1
print(resolved)
" 2>/dev/null
    echo "HEALTH_RESOLVED|$CONFLICTS conflicts archived"
elif [ "$CONFLICTS" -gt 0 ]; then
    echo "HEALTH_CONFLICTS|$CONFLICTS conflicts found"
else
    echo "HEALTH_OK|syncthing healthy"
fi
"""
