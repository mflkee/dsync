import tomllib
from pathlib import Path

from dsync.config import Config


def test_load_default_config(tmp_path: Path):
    cfg = Config(path=tmp_path / "config.toml")
    assert cfg.machines == {}
    assert cfg.git_source == Path.home() / "dotfiles"
    assert cfg.git_branch == "main"
    assert cfg.git_remote_url is None
    assert cfg.discover_aliases == {}
    assert cfg.discover_prefixes == ("archlinux-", "mkair-")


def test_load_existing_config(tmp_path: Path):
    path = tmp_path / "config.toml"
    path.write_text(
        """
[machines]
notebook = { host = "nb.netbird.cloud", user = "u" }

[git]
source = "~/dotfiles"
branch = "master"
remote_url = "https://example.com/dotfiles.git"

[discover.aliases]
myhost = "short"
"""
    )
    cfg = Config(path=path)
    assert cfg.machines == {"notebook": {"host": "nb.netbird.cloud", "user": "u"}}
    assert cfg.git_source == Path.home() / "dotfiles"
    assert cfg.git_branch == "master"
    assert cfg.git_remote_url == "https://example.com/dotfiles.git"
    assert cfg.discover_aliases == {"myhost": "short"}


def test_add_machine_persists_config(tmp_path: Path):
    path = tmp_path / "config.toml"
    cfg = Config(path=path)
    cfg.add_machine("desktop", "d.netbird.cloud", "user")

    cfg2 = Config(path=path)
    assert cfg2.machines == {"desktop": {"host": "d.netbird.cloud", "user": "user"}}

    # Saved file must be valid TOML
    with path.open("rb") as f:
        data = tomllib.load(f)
    assert data["machines"]["desktop"]["host"] == "d.netbird.cloud"
    assert data["git"]["branch"] == "main"


def test_custom_prefixes(tmp_path: Path):
    path = tmp_path / "config.toml"
    path.write_text('[discover]\nprefixes = ["foo-"]\n')
    cfg = Config(path=path)
    assert cfg.discover_prefixes == ("foo-",)
