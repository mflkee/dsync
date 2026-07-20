import json
import subprocess
from pathlib import Path

from dsync import selfupdate


def _init_repo(path: Path):
    subprocess.run(["git", "init", str(path)], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(path), "config", "user.email", "t@t.com"], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(path), "config", "user.name", "T"], check=True, capture_output=True)
    (path / "f.txt").write_text("x")
    subprocess.run(["git", "-C", str(path), "add", "."], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(path), "commit", "-m", "init"], check=True, capture_output=True)


def _fake_uv_tool(tmp_path: Path, source: Path) -> Path:
    tool = tmp_path / "uv" / "tools" / "dsync"
    dist = tool / "lib" / "python3.13" / "site-packages" / "dsync-0.1.0.dist-info"
    dist.mkdir(parents=True)
    (dist / "direct_url.json").write_text(json.dumps({
        "url": f"file://{source}", "dir_info": {"editable": True},
    }))
    return tool


def test_find_install_source(tmp_path: Path, monkeypatch):
    src = tmp_path / "dsync-src"
    src.mkdir()
    tool = _fake_uv_tool(tmp_path, src)
    monkeypatch.setattr(selfupdate, "UV_TOOL_DIR", tool)
    assert selfupdate.find_install_source() == src


def test_find_install_source_missing(tmp_path: Path, monkeypatch):
    monkeypatch.setattr(selfupdate, "UV_TOOL_DIR", tmp_path / "nope")
    assert selfupdate.find_install_source() is None


def test_get_self_status_not_a_repo(tmp_path: Path, monkeypatch):
    src = tmp_path / "dsync-src"
    src.mkdir()
    tool = _fake_uv_tool(tmp_path, src)
    monkeypatch.setattr(selfupdate, "UV_TOOL_DIR", tool)
    st = selfupdate.get_self_status(do_fetch=False)
    assert "не является git-репозиторием" in st.error


def test_get_self_status_no_origin(tmp_path: Path, monkeypatch):
    src = tmp_path / "dsync-src"
    src.mkdir()
    _init_repo(src)
    tool = _fake_uv_tool(tmp_path, src)
    monkeypatch.setattr(selfupdate, "UV_TOOL_DIR", tool)
    st = selfupdate.get_self_status(do_fetch=False)
    assert st.source == src
    assert st.current_sha
    assert st.is_clean is True
    assert "нет origin" in st.error
