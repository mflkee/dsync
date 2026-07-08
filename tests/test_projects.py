from pathlib import Path

from dsync import projects


def test_project_status_for_missing_repo():
    info = {"path": "/nonexistent/path", "remote": "https://example.com/repo.git"}
    ps = projects.get_project_status("myapp", info)
    assert ps.name == "myapp"
    assert ps.exists is False
    assert ps.sync_status == "not cloned locally"


def test_project_status_for_clean_repo(tmp_path: Path):
    import subprocess

    repo = tmp_path / "myapp"
    subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.email", "t@t.com"], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.name", "T"], check=True, capture_output=True)
    (repo / "file.txt").write_text("hello")
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(repo), "commit", "-m", "init"], check=True, capture_output=True)

    info = {"path": str(repo), "remote": "https://example.com/repo.git"}
    ps = projects.get_project_status("myapp", info)
    assert ps.exists is True
    assert ps.is_clean is True


def test_remote_sync_script_includes_pull_and_clone():
    script = projects.remote_sync_script("/home/user/myapp", "main", "https://example.com/repo.git")
    assert "git pull --rebase origin main" in script
    assert "git clone https://example.com/repo.git /home/user/myapp" in script


def test_remote_clone_script_checks_existing_repo():
    script = projects.remote_clone_script("/home/user/myapp", "https://example.com/repo.git", "main")
    assert "ALREADY_CLONED" in script
    assert "git clone https://example.com/repo.git /home/user/myapp" in script
