from dsync.ssh_client import SSHResult, check_port, run


def test_ssh_result_success():
    r = SSHResult(stdout="hello", stderr="", returncode=0, success=True)
    assert r.success
    assert r.stdout == "hello"
    assert r.returncode == 0
    assert not r.is_transient


def test_ssh_result_failure():
    r = SSHResult(stdout="", stderr="connection refused", returncode=1, success=False)
    assert not r.success
    assert r.stderr == "connection refused"


def test_ssh_result_is_transient_timeout():
    r = SSHResult(stdout="", stderr="ssh: connect timed out", returncode=-1, success=False)
    assert r.is_transient


def test_ssh_result_is_transient_refused():
    r = SSHResult(stdout="", stderr="connection refused", returncode=-1, success=False)
    assert r.is_transient


def test_ssh_result_is_transient_no_route():
    r = SSHResult(stdout="", stderr="no route to host", returncode=-1, success=False)
    assert r.is_transient


def test_ssh_result_not_transient_permission():
    r = SSHResult(stdout="", stderr="Permission denied", returncode=255, success=False)
    assert not r.is_transient


def test_check_port_closed_default():
    assert check_port("127.0.0.1", port=1, timeout=0.5) is False


def test_run_mocked_success(monkeypatch):
    def fake_run(cmd, **kwargs):
        class R:
            pass
        r = R()
        r.stdout = "output"
        r.stderr = ""
        r.returncode = 0
        return r
    monkeypatch.setattr("dsync.ssh_client.subprocess.run", fake_run)
    r = run("1.2.3.4", "echo ok", user="test", timeout=10)
    assert r.success
    assert r.stdout == "output"


def test_run_mocked_timeout(monkeypatch):
    import subprocess
    def fake_run(cmd, **kwargs):
        raise subprocess.TimeoutExpired(cmd=cmd, timeout=10)
    monkeypatch.setattr("dsync.ssh_client.subprocess.run", fake_run)
    r = run("1.2.3.4", "sleep 999", user="test", timeout=1)
    assert not r.success
    assert "timed out" in r.stderr


def test_run_mocked_ssh_not_found(monkeypatch):
    def fake_run(cmd, **kwargs):
        raise FileNotFoundError
    monkeypatch.setattr("dsync.ssh_client.subprocess.run", fake_run)
    r = run("1.2.3.4", "cmd", user="test")
    assert not r.success
    assert "not found" in r.stderr
