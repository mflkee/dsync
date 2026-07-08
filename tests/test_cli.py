from pathlib import Path
from unittest.mock import MagicMock

from dsync import cli
from dsync.config import Config


def test_remote_sync_script_uses_configured_url():
    script = cli._remote_sync_script("/home/user/dotfiles", "main", "https://example.com/repo.git")
    assert "git clone https://example.com/repo.git /home/user/dotfiles" in script
    assert "git pull --rebase origin main" in script


def test_is_ip_helper():
    assert cli._is_ip_value("192.168.1.1")
    assert cli._is_ip_value("::1")
    assert not cli._is_ip_value("host.netbird.cloud")
    assert not cli._is_ip_value("not-an-ip")


def test_config_remote_url_override(tmp_path: Path):
    path = tmp_path / "config.toml"
    path.write_text('[git]\nremote_url = "https://override.git"\n')
    cfg = Config(path=path)
    assert cfg.git_remote_url == "https://override.git"


def test_sync_remote_machine_dry_run_skips_ssh(monkeypatch, tmp_path: Path):
    nb = MagicMock()
    nb.self_fqdn = "self.netbird.cloud"
    info = {"host": "peer.netbird.cloud", "user": "u"}
    peer = MagicMock()
    peer.fqdn = "peer.netbird.cloud"
    peer.is_connected = True
    nb.peers = [peer]

    monkeypatch.setattr(cli, "resolve_host", lambda h: "100.64.0.2")
    monkeypatch.setattr(cli, "_check_port", lambda ip: True)
    ssh_called: list[tuple[tuple, dict]] = []
    def fake_ssh(*a, **k):
        ssh_called.append((a, k))
        return MagicMock(success=True)
    monkeypatch.setattr(cli, "ssh_run", fake_ssh)

    status, reason = cli._sync_remote_machine(
        tmp_path, "main", "https://example.com/repo.git", nb, "peer", info, dry_run=True
    )
    assert status == "success"
    assert "dry-run" in reason
    assert ssh_called == []


def test_run_remote_sync_respects_jobs(monkeypatch, tmp_path: Path):
    nb = MagicMock()
    nb.self_fqdn = "self.netbird.cloud"
    calls = []

    def fake_sync_remote(repo, branch, url, nb, name, info, dry_run, show_spinner):
        calls.append((name, show_spinner))
        return "success", ""

    monkeypatch.setattr(cli, "_sync_remote_machine", fake_sync_remote)

    machines = {"a": {"host": "a.netbird.cloud"}, "b": {"host": "b.netbird.cloud"}}
    rows, ok, fail, skip = cli._run_remote_sync(
        tmp_path, "main", "https://example.com/repo.git", nb, machines, dry_run=False, jobs=1
    )
    assert len(rows) == 2
    assert all(c[1] is True for c in calls)

    calls.clear()
    cli._run_remote_sync(
        tmp_path, "main", "https://example.com/repo.git", nb, machines, dry_run=False, jobs=4
    )
    assert all(c[1] is False for c in calls)
