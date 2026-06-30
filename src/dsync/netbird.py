import json
import os
import subprocess
from dataclasses import dataclass, field
from typing import Optional


@dataclass
class Peer:
    fqdn: str
    netbird_ip: str
    status: str
    latency_ns: int = 0
    connection_type: str = "-"
    transfer_received: int = 0
    transfer_sent: int = 0

    @property
    def is_connected(self) -> bool:
        return self.status.lower() == "connected"

    @property
    def hostname_short(self) -> str:
        return self.fqdn.removesuffix(".netbird.cloud")

    @property
    def latency_ms(self) -> str:
        if self.latency_ns <= 0:
            return "-"
        ms = self.latency_ns / 1_000_000
        if ms < 1:
            return "<1ms"
        return f"{ms:.0f}ms"


@dataclass
class NetBirdStatus:
    peers: list = field(default_factory=list)
    self_fqdn: str = ""
    self_ip: str = ""
    daemon_status: str = ""

    @property
    def self_hostname_short(self) -> str:
        return self.self_fqdn.removesuffix(".netbird.cloud")


def get_status() -> Optional[NetBirdStatus]:
    try:
        env = os.environ.copy()
        env.update({"LC_ALL": "C", "LANG": "C"})
        result = subprocess.run(
            ["netbird", "status", "--json"],
            capture_output=True,
            text=True,
            timeout=10,
            env=env,
        )
        if result.returncode != 0:
            return None
        data = json.loads(result.stdout)
    except (FileNotFoundError, subprocess.TimeoutExpired, json.JSONDecodeError):
        return None

    status = NetBirdStatus()
    status.self_fqdn = data.get("fqdn", "")
    status.self_ip = data.get("netbirdIp", "").split("/")[0]
    status.daemon_status = data.get("daemonStatus", "")

    peers_data = data.get("peers", {})
    details = peers_data.get("details") or []
    for p in details:
        peer = Peer(
            fqdn=p.get("fqdn", ""),
            netbird_ip=p.get("netbirdIp", ""),
            status=p.get("status", "Unknown"),
            latency_ns=p.get("latency", 0),
            connection_type=p.get("connectionType", "-"),
            transfer_received=p.get("transferReceived", 0),
            transfer_sent=p.get("transferSent", 0),
        )
        status.peers.append(peer)

    return status
