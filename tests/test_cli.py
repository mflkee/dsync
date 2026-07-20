from pathlib import Path

from dsync import cli, hub_cli, project_cli
from dsync.config import Config
from dsync.log import setup_logging


def test_check_port_closed():
    assert cli._check_port("127.0.0.1", port=1, timeout=1) is False


def test_config_remote_url_override(tmp_path: Path):
    path = tmp_path / "config.toml"
    path.write_text('[git]\nremote_url = "https://override.git"\n')
    cfg = Config(path=path)
    assert cfg.git_remote_url == "https://override.git"


def test_config_hub_root_default(tmp_path: Path):
    path = tmp_path / "config.toml"
    path.write_text("")
    cfg = Config(path=path)
    assert cfg.hub_root == Path.home() / "projects"


def test_config_hub_root_override(tmp_path: Path):
    path = tmp_path / "config.toml"
    path.write_text('[hub]\nroot = "~/code"\n')
    cfg = Config(path=path)
    assert cfg.hub_root == Path.home() / "code"


def test_filter_projects_selects_subset():
    projects = {"a": {}, "b": {}, "c": {}}
    selected = project_cli._filter_projects(projects, ["a", "c"])
    assert selected is not None
    assert list(selected.keys()) == ["a", "c"]


def test_filter_projects_all_when_no_request():
    projects = {"a": {}}
    assert project_cli._filter_projects(projects, []) == projects


def test_filter_projects_returns_none_on_unknown():
    assert project_cli._filter_projects({"a": {}}, ["x"]) is None


def test_setup_logging_creates_file(tmp_path: Path):
    log_file = tmp_path / "dsync.log"
    path = setup_logging(log_file=str(log_file), level="DEBUG")
    assert path == log_file
    assert log_file.exists()


def test_parse_hub_output_all_statuses():
    stdout = (
        "HUB|alpha|updated|\n"
        "HUB|beta|uptodate|\n"
        "HUB|gamma|dirty|\n"
        "HUB|delta|noremote|\n"
        "HUB|eps|failed|merge conflict\n"
    )
    rows, error = hub_cli._parse_hub_output(stdout)
    assert error == ""
    assert len(rows) == 5
    by_name = {r[0]: r[2] for r in rows}
    assert by_name["alpha"] == "обновлён"
    assert by_name["beta"] == "актуален"
    assert by_name["gamma"] == "dirty — пропущен"
    assert by_name["delta"] == "нет remote"
    assert "merge conflict" in by_name["eps"]


def test_parse_hub_output_error_line():
    rows, error = hub_cli._parse_hub_output("HUB_ERROR|no root dir /x\n")
    assert rows == []
    assert error == "no root dir /x"
