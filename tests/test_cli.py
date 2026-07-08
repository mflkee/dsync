from pathlib import Path

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
