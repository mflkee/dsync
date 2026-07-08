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


def test_filter_machines_selects_subset():
    machines = {
        "a": {"host": "a.netbird.cloud"},
        "b": {"host": "b.netbird.cloud"},
        "c": {"host": "c.netbird.cloud"},
    }
    selected = cli._filter_machines(machines, ["a", "c"])
    assert selected is not None
    assert list(selected.keys()) == ["a", "c"]


def test_filter_machines_all_when_no_request():
    machines = {"a": {"host": "a.netbird.cloud"}}
    selected = cli._filter_machines(machines, [])
    assert selected is not None
    assert selected == machines


def test_filter_machines_returns_none_on_unknown():
    machines = {"a": {"host": "a.netbird.cloud"}}
    selected = cli._filter_machines(machines, ["x"])
    assert selected is None


def test_setup_logging_creates_file(tmp_path: Path):
    log_file = tmp_path / "dsync.log"
    path = cli.setup_logging(log_file=str(log_file), level="DEBUG")
    assert path == log_file
    assert log_file.exists()


def test_help_command_runs_without_error():
    assert cli.cmd_help() == 0


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
