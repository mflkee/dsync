import json

from dsync.syncthing import (
    SyncthingStatus,
    check_conflicts,
    check_running,
    resolve_conflicts,
)


def test_syncthing_status_running():
    s = SyncthingStatus(running=True, uptime=3600, device_id="ABCD1234")
    assert s.running
    assert s.uptime == 3600
    assert s.device_id == "ABCD1234"
    assert s.conflicts is None
    assert s.error == ""


def test_syncthing_status_stopped():
    s = SyncthingStatus(running=False, error="syncthing not found")
    assert not s.running
    assert s.error == "syncthing not found"


def test_syncthing_status_with_conflicts():
    s = SyncthingStatus(
        running=True,
        uptime=100,
        conflicts=[{"id": "folder1", "path": "/data", "conflicts": 3}],
    )
    assert s.running
    assert len(s.conflicts) == 1
    assert s.conflicts[0]["conflicts"] == 3


def test_check_running_mocked_success(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        data = {"ok": True, "uptime": 7200, "myID": "abcdef123456"}
        return SSHResult(
            stdout=json.dumps(data), stderr="", returncode=0, success=True
        )

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    status = check_running("1.2.3.4")

    assert status.running
    assert status.uptime == 7200
    assert status.device_id == "abcdef123456"


def test_check_running_mocked_not_running(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(
            stdout='{"ok":false}', stderr="", returncode=0, success=True
        )

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    status = check_running("1.2.3.4")

    assert not status.running


def test_check_running_mocked_ssh_fail(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(
            stdout="", stderr="connection refused", returncode=1, success=False
        )

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    status = check_running("1.2.3.4")

    assert not status.running
    assert "refused" in status.error


def test_check_running_mocked_parse_error(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(
            stdout="not json at all", stderr="", returncode=0, success=True
        )

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    status = check_running("1.2.3.4")

    assert not status.running
    assert "parse error" in status.error


def test_check_conflicts_mocked_no_conflicts(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(stdout="[]", stderr="", returncode=0, success=True)

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    conflicts = check_conflicts("1.2.3.4")

    assert conflicts == []


def test_check_conflicts_mocked_has_conflicts(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        data = [{"id": "abc", "path": "/sync", "conflicts": 2}]
        return SSHResult(
            stdout=json.dumps(data), stderr="", returncode=0, success=True
        )

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    conflicts = check_conflicts("1.2.3.4")

    assert len(conflicts) == 1
    assert conflicts[0]["conflicts"] == 2


def test_check_conflicts_mocked_ssh_fail(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(stdout="", stderr="timeout", returncode=1, success=False)

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    conflicts = check_conflicts("1.2.3.4")

    assert conflicts == []


def test_resolve_conflicts_mocked_success(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(
            stdout='{"resolved":5}', stderr="", returncode=0, success=True
        )

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    resolved = resolve_conflicts("1.2.3.4")

    assert resolved == 5


def test_resolve_conflicts_mocked_ssh_fail(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult
        return SSHResult(stdout="", stderr="fail", returncode=1, success=False)

    monkeypatch.setattr("dsync.syncthing.ssh_run", fake_run)
    resolved = resolve_conflicts("1.2.3.4")

    assert resolved == 0
