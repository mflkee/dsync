from dsync.obsidian import (
    ObsidianStatus,
    _check_api_cmd,
    check_api,
)


def test_obsidian_status_ok():
    s = ObsidianStatus(api_responsive=True, service_active=True, http_code=200)
    assert s.api_responsive
    assert s.service_active
    assert s.http_code == 200
    assert s.error == ""


def test_obsidian_status_down():
    s = ObsidianStatus(
        api_responsive=False, service_active=False, http_code=0, error="timeout"
    )
    assert not s.api_responsive
    assert not s.service_active
    assert s.error == "timeout"


def test_obsidian_status_service_active_api_down():
    s = ObsidianStatus(api_responsive=False, service_active=True, http_code=500)
    assert not s.api_responsive
    assert s.service_active


def test_check_api_cmd_produces_curl():
    cmd = _check_api_cmd("test-key-123")
    assert "curl" in cmd
    assert "Authorization: Bearer test-key-123" in cmd
    assert "127.0.0.1:27123" in cmd


def test_check_api_mocked_success(monkeypatch):
    calls = []

    def fake_run(ip, cmd, **kwargs):
        calls.append(cmd)
        from dsync.ssh_client import SSHResult

        if "is-active obsidian.service" in cmd:
            return SSHResult(stdout="active", stderr="", returncode=0, success=True)
        if "27123" in cmd:
            return SSHResult(stdout="200", stderr="", returncode=0, success=True)
        return SSHResult(stdout="", stderr="fail", returncode=1, success=False)

    monkeypatch.setattr("dsync.obsidian.ssh_run", fake_run)
    status = check_api("1.2.3.4", user="test", api_key="k")

    assert status.api_responsive
    assert status.service_active
    assert status.http_code == 200
    assert len(calls) == 2


def test_check_api_mocked_service_inactive(monkeypatch):
    def fake_run(ip, cmd, **kwargs):
        from dsync.ssh_client import SSHResult

        if "is-active obsidian.service" in cmd:
            return SSHResult(stdout="inactive", stderr="", returncode=4, success=False)
        if "27123" in cmd:
            return SSHResult(stdout="000", stderr="", returncode=0, success=True)
        return SSHResult(stdout="", stderr="fail", returncode=1, success=False)

    monkeypatch.setattr("dsync.obsidian.ssh_run", fake_run)
    status = check_api("1.2.3.4", user="test", api_key="k")

    assert not status.api_responsive
    assert not status.service_active
