import subprocess
from pathlib import Path

from dsync import hub


def _init_repo(path: Path, dirty: bool = False):
    subprocess.run(["git", "init", str(path)], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(path), "config", "user.email", "t@t.com"], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(path), "config", "user.name", "T"], check=True, capture_output=True)
    (path / "file.txt").write_text("hello")
    subprocess.run(["git", "-C", str(path), "add", "."], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(path), "commit", "-m", "init"], check=True, capture_output=True)
    if dirty:
        (path / "file.txt").write_text("changed")


def test_discover_repos(tmp_path: Path):
    _init_repo(tmp_path / "repo-a")
    _init_repo(tmp_path / "repo-b")
    (tmp_path / "not-a-repo").mkdir()
    _init_repo(tmp_path / ".hidden")
    repos = hub.discover_repos(tmp_path)
    names = [r.name for r in repos]
    assert names == ["repo-a", "repo-b"]


def test_discover_repos_missing_root(tmp_path: Path):
    assert hub.discover_repos(tmp_path / "nope") == []


def test_check_repo_clean(tmp_path: Path):
    repo = tmp_path / "proj"
    _init_repo(repo)
    hr = hub.check_repo(repo, do_fetch=False)
    assert hr.name == "proj"
    assert hr.is_clean is True
    assert hr.status_text in ("clean", "no remote")


def test_check_repo_dirty(tmp_path: Path):
    repo = tmp_path / "proj"
    _init_repo(repo, dirty=True)
    hr = hub.check_repo(repo, do_fetch=False)
    assert hr.is_clean is False
    assert "dirty" in hr.status_text


def test_pull_repo_skips_dirty(tmp_path: Path):
    repo = tmp_path / "proj"
    _init_repo(repo, dirty=True)
    r = hub.pull_repo(repo)
    assert r.success is False
    assert r.stderr == "dirty"


def test_pull_repo_no_remote(tmp_path: Path):
    repo = tmp_path / "proj"
    _init_repo(repo)
    r = hub.pull_repo(repo)
    assert r.success is False
    assert r.stderr == "no remote"


def test_remote_hub_script_marks_dirty_and_ff_only():
    script = hub.remote_hub_script("/home/user/projects")
    assert "git pull --ff-only" in script
    assert "HUB|$name|dirty|" in script
    assert "HUB|$name|updated|" in script
    assert "HUB|$name|uptodate|" in script
