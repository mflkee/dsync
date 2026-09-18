"""Project (non-dotfiles) repository sync for dsync."""

import shlex
from dataclasses import dataclass
from pathlib import Path

from .chezmoi import GitResult, _git
from .chezmoi import get_status as git_status


@dataclass
class ProjectStatus:
    name: str
    path: Path
    remote: str | None
    exists: bool
    is_clean: bool = True
    ahead: int = 0
    behind: int = 0
    branch: str = ""
    error: str = ""

    @property
    def sync_status(self) -> str:
        if self.error:
            return f"error: {self.error}"
        if not self.exists:
            return "not cloned locally"
        if not self.is_clean:
            return "dirty"
        if self.ahead > 0 or self.behind > 0:
            return f"ahead {self.ahead}, behind {self.behind}"
        return "clean"


def get_project_status(name: str, info: dict) -> ProjectStatus:
    path = Path(info.get("path", "")).expanduser()
    remote = info.get("remote")
    ps = ProjectStatus(name=name, path=path, remote=remote, exists=path.exists())

    if not path.exists():
        return ps

    if not (path / ".git").is_dir():
        ps.error = "not a git repo"
        return ps

    gs = git_status(path)
    ps.is_clean = gs.is_clean
    ps.ahead = gs.ahead
    ps.behind = gs.behind
    ps.branch = gs.current_branch
    ps.error = gs.error
    return ps


def sync_project_repo(repo: Path, branch: str, remote: str | None = None) -> GitResult:
    """Commit local changes, pull and push a project repository."""
    gs = git_status(repo)
    if gs.error:
        return GitResult(success=False, stderr=f"git status: {gs.error}")

    if not gs.is_clean:
        add = _git(repo, ["add", "-A"])
        if not add.success:
            return add
        commit_msg = f"project sync: {repo.name}"
        commit = _git(repo, ["commit", "-m", commit_msg])
        if not commit.success:
            return commit

    fetch = _git(repo, ["fetch", "origin"], timeout=30)
    if not fetch.success:
        return fetch

    ahead, behind = _diverged(repo, branch)
    if behind > 0:
        pull = _git(repo, ["pull", "--rebase", "origin", branch], timeout=60)
        if not pull.success:
            return pull

    if ahead > 0 or gs.ahead > 0:
        push = _git(repo, ["push", "origin", branch], timeout=60)
        if not push.success:
            return push

    return GitResult(success=True)


def _diverged(repo: Path, branch: str) -> tuple[int, int]:
    r = _git(repo, ["rev-list", "--count", "--left-right", f"HEAD...origin/{branch}"])
    if not r.success:
        return (0, 0)
    parts = r.stdout.split()
    if len(parts) != 2:
        return (0, 0)
    try:
        return int(parts[0]), int(parts[1])
    except ValueError:
        return (0, 0)


def remote_sync_script(repo_path: str, branch: str, remote_url: str | None) -> str:
    """Build shell script to sync a project repo on a remote machine."""
    url = shlex.quote(remote_url) if remote_url else ""
    return f"""export PATH="$HOME/.local/bin:$PATH"
if [ -d {shlex.quote(repo_path)}/.git ]; then
  cd {shlex.quote(repo_path)}
  git fetch origin {shlex.quote(branch)} 2>&1 || true
  git pull --rebase origin {shlex.quote(branch)} 2>&1 || echo "GIT_CONFLICT"
elif [ -n {url} ]; then
  git clone {url} {shlex.quote(repo_path)} 2>&1
  cd {shlex.quote(repo_path)} && git checkout {shlex.quote(branch)} 2>&1 || true
else
  echo "NO_REMOTE"
  exit 0
fi"""


def remote_clone_script(repo_path: str, remote_url: str, branch: str) -> str:
    """Build shell script to clone a project repo on a remote machine."""
    return f"""export PATH="$HOME/.local/bin:$PATH"
if [ -d {shlex.quote(repo_path)}/.git ]; then
  echo "ALREADY_CLONED"
else
  git clone {shlex.quote(remote_url)} {shlex.quote(repo_path)} 2>&1
  cd {shlex.quote(repo_path)} && git checkout {shlex.quote(branch)} 2>&1 || true
fi"""
