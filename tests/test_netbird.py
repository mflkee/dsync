import json

from dsync.netbird import get_status


def _run_status(monkeypatch, stdout: str, returncode: int = 0):
    def fake_run(cmd, **kwargs):
        class R:
            pass

        r = R()
        r.stdout = stdout
        r.stderr = ""
        r.returncode = returncode
        return r

    monkeypatch.setattr("dsync.netbird.subprocess.run", fake_run)
    return get_status()


def test_get_status_parses_peers(monkeypatch):
    nb = _run_status(
        monkeypatch,
        json.dumps(
            {
                "fqdn": "self.netbird.cloud",
                "netbirdIp": "100.64.0.1/16",
                "daemonStatus": "connected",
                "peers": {
                    "details": [
                        {
                            "fqdn": "peer.netbird.cloud",
                            "netbirdIp": "100.64.0.2",
                            "status": "Connected",
                            "latency": 1_500_000,
                            "connectionType": "P2P",
                        }
                    ]
                },
            }
        ),
    )
    assert nb is not None
    assert nb.self_fqdn == "self.netbird.cloud"
    assert nb.self_ip == "100.64.0.1"
    assert nb.self_hostname_short == "self"
    assert len(nb.peers) == 1
    peer = nb.peers[0]
    assert peer.fqdn == "peer.netbird.cloud"
    assert peer.netbird_ip == "100.64.0.2"
    assert peer.is_connected
    assert peer.latency_ms == "2ms"


def test_get_status_returns_none_on_error(monkeypatch):
    nb = _run_status(monkeypatch, "not-json", returncode=0)
    assert nb is None


def test_get_status_peers_as_list(monkeypatch):
    nb = _run_status(
        monkeypatch,
        json.dumps(
            {
                "fqdn": "self.netbird.cloud",
                "netbirdIp": "100.64.0.1/16",
                "daemonStatus": "connected",
                "peers": [
                    {
                        "fqdn": "peer.netbird.cloud",
                        "netbirdIp": "100.64.0.2",
                        "status": "Connected",
                    }
                ],
            }
        ),
    )
    assert nb is not None
    assert len(nb.peers) == 1


def test_get_status_tolerates_missing_peers(monkeypatch):
    nb = _run_status(
        monkeypatch,
        json.dumps(
            {"fqdn": "self.netbird.cloud", "netbirdIp": "100.64.0.1/16", "daemonStatus": "connected"}
        ),
    )
    assert nb is not None
    assert nb.peers == []
