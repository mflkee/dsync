"""Self-update support: dsync tracks and updates its own editable install."""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .chezmoi import GitResult, _git

UV_TOOL_DIR = Path.home() / ".local" / "share" / "uv" / "tools" / "dsync"


@dataclass
class SelfStatus:
    source: Path | None = None
    branch: str = ""
    current_sha: str = ""
    remote_sha: str = ""
    behind: int = 0
    is_clean: bool = True
    editable: bool = False
    error: str = ""

    @property
    def up_to_date(self) -> bool:
        return not self.error and self.behind == 0


def find_install_source() -> Path | None:
    """Locate the editable source dir of the installed dsync (via uv direct_url.json)."""
    if not UV_TOOL_DIR.is_dir():
        return None
    for direct_url in UV_TOOL_DIR.glob(
        "lib/python*/site-packages/dsync-*.dist-info/direct_url.json"
    ):
        try:
            data = json.loads(direct_url.read_text())
        except (json.JSONDecodeError, OSError):
            continue
        url = data.get("url", "")
        if url.startswith("file://"):
            return Path(url[len("file://") :])
    return None


def get_self_status(do_fetch: bool = True) -> SelfStatus:
    st = SelfStatus()
    src = find_install_source()
    if src is None:
        st.error = "editable-установка dsync не найдена (uv tool)"
        return st
    st.source = src

    if not (src / ".git").is_dir():
        st.error = f"{src} не является git-репозиторием"
        return st

    br = _git(src, ["rev-parse", "--abbrev-ref", "HEAD"])
    if br.success:
        st.branch = br.stdout
    sha = _git(src, ["rev-parse", "--short", "HEAD"])
    if sha.success:
        st.current_sha = sha.stdout

    dirty = _git(src, ["status", "--porcelain"])
    st.is_clean = dirty.success and not dirty.stdout

    if do_fetch:
        _git(src, ["fetch", "origin", "--quiet"], timeout=30)

    branch = st.branch or "main"
    rsha = _git(src, ["rev-parse", "--short", f"origin/{branch}"])
    if not rsha.success and branch != "main":
        branch = "main"
        rsha = _git(src, ["rev-parse", "--short", f"origin/{branch}"])
    if rsha.success:
        st.remote_sha = rsha.stdout
    else:
        st.error = f"нет origin/{branch}"
        return st

    div = _git(src, ["rev-list", "--count", f"HEAD..origin/{branch}"])
    if div.success:
        try:
            st.behind = int(div.stdout)
        except ValueError:
            pass
    return st


def self_update() -> GitResult:
    """Pull the dsync source repo and reinstall the uv tool. Returns result with summary."""
    st = get_self_status(do_fetch=True)
    if st.error:
        return GitResult(success=False, stderr=st.error)
    if not st.is_clean:
        return GitResult(
            success=False, stderr=f"репозиторий {st.source} грязный, пропускаю"
        )
    if st.behind == 0:
        return GitResult(success=True, stdout="up-to-date")

    assert st.source is not None
    pull = _git(st.source, ["pull", "--ff-only"], timeout=60)
    if not pull.success:
        return GitResult(success=False, stderr=f"pull: {pull.stderr[:200]}")

    reinstall = subprocess.run(
        ["uv", "tool", "install", "--force", "--editable", str(st.source)],
        capture_output=True,
        text=True,
        timeout=300,
    )
    if reinstall.returncode != 0:
        return GitResult(
            success=False, stderr=f"uv reinstall: {reinstall.stderr.strip()[:200]}"
        )

    new_sha = _git(st.source, ["rev-parse", "--short", "HEAD"])
    return GitResult(success=True, stdout=f"{st.current_sha} -> {new_sha.stdout}")
