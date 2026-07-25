import logging
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

logger = logging.getLogger(__name__)


@dataclass
class GitStatus:
    is_clean: bool = True
    staged: int = 0
    unstaged: int = 0
    untracked: int = 0
    ahead: int = 0
    behind: int = 0
    current_branch: str = ""
    has_remote: bool = False
    error: str = ""


@dataclass
class GitResult:
    success: bool
    stdout: str = ""
    stderr: str = ""
    returncode: int = 0


def _git(repo_path: Path, args: list[str], timeout: int = 30) -> GitResult:
    logger.debug("git %s (cwd=%s)", " ".join(args), repo_path)
    try:
        env = os.environ.copy()
        env.update({"LC_ALL": "C", "LANG": "C"})
        result = subprocess.run(
            ["git"] + args,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=repo_path,
            env=env,
        )
        if result.returncode != 0:
            logger.debug("git %s failed: %s", args[0], result.stderr.strip()[:200])
        return GitResult(
            success=result.returncode == 0,
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )
    except subprocess.TimeoutExpired:
        logger.warning("git %s timed out after %ds", args[0], timeout)
        return GitResult(success=False, stderr="git command timed out", returncode=-1)
    except FileNotFoundError:
        return GitResult(success=False, stderr="git not found", returncode=-2)


def get_status(repo_path: Path) -> GitStatus:
    gs = GitStatus()

    br = _git(repo_path, ["rev-parse", "--abbrev-ref", "HEAD"])
    if br.success:
        gs.current_branch = br.stdout

    st = _git(repo_path, ["status", "--porcelain"])
    if st.success:
        for line in st.stdout.splitlines():
            if not line.strip():
                continue
            prefix = line[:2]
            if prefix == "??":
                gs.untracked += 1
            elif prefix[0] != " ":
                gs.staged += 1
            elif prefix[1] != " ":
                gs.unstaged += 1

    remote = _git(repo_path, ["rev-parse", "--abbrev-ref", "@{upstream}"])
    gs.has_remote = remote.success

    if remote.success:
        rev = _git(
            repo_path, ["rev-list", "--count", "--left-right", "HEAD...@{upstream}"]
        )
        if rev.success:
            parts = rev.stdout.split()
            if len(parts) == 2:
                try:
                    gs.ahead = int(parts[0])
                    gs.behind = int(parts[1])
                except ValueError:
                    pass

    gs.is_clean = (gs.staged + gs.unstaged + gs.untracked) == 0
    return gs


def commit(repo_path: Path, message: str) -> GitResult:
    add = _git(repo_path, ["add", "-A"])
    if not add.success:
        return add
    return _git(repo_path, ["commit", "-m", message])


def pull(repo_path: Path, branch: str = "main") -> GitResult:
    return _git(repo_path, ["pull", "--rebase", "origin", branch], timeout=60)


def fetch(repo_path: Path) -> GitResult:
    return _git(repo_path, ["fetch", "origin"], timeout=30)


def get_remote_url(repo_path: Path, remote: str = "origin") -> str | None:
    """Return the URL of the given git remote, or None if it does not exist."""
    r = _git(repo_path, ["remote", "get-url", remote])
    if r.success:
        return r.stdout
    return None


def push(repo_path: Path, branch: str = "main") -> GitResult:
    return _git(repo_path, ["push", "origin", branch], timeout=180)


def diverts_check(repo_path: Path, branch: str = "main") -> tuple[int, int]:
    """Returns (ahead, behind) vs origin/<branch>."""
    r = _git(
        repo_path, ["rev-list", "--count", "--left-right", f"HEAD...origin/{branch}"]
    )
    if not r.success:
        return (0, 0)
    parts = r.stdout.split()
    if len(parts) != 2:
        return (0, 0)
    try:
        return int(parts[0]), int(parts[1])
    except ValueError:
        return (0, 0)


def re_add_secrets() -> GitResult:
    """Re-add encrypted secrets so chezmoi source stays in sync with live files."""
    secrets = Path.home() / ".config" / "zsh" / "secrets.zsh"
    if not secrets.exists():
        return GitResult(success=True)
    try:
        result = subprocess.run(
            ["chezmoi", "re-add", str(secrets)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return GitResult(
            success=result.returncode == 0,
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )
    except subprocess.TimeoutExpired:
        return GitResult(
            success=False, stderr="chezmoi re-add timed out", returncode=-1
        )
    except FileNotFoundError:
        return GitResult(success=False, stderr="chezmoi not found", returncode=-2)


def _chezmoi_managed_set() -> set[str]:
    """Return set of chezmoi-managed target paths (e.g. '.config/noctalia/settings.json')."""
    try:
        result = subprocess.run(
            ["chezmoi", "managed", "--include", "files"],
            capture_output=True,
            text=True,
            timeout=15,
        )
        if result.returncode == 0 and result.stdout.strip():
            return set(result.stdout.strip().splitlines())
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    return set()


def _target_to_source_path(target: str) -> str:
    """Convert chezmoi target path to source path.

    chezmoi uses dot_ prefix for hidden dirs, tilde_ for home:
      .config/noctalia/settings.json -> dot_config/noctalia/settings.json
      .local/bin/foo -> dot_local/bin/foo
      ~/foo -> tilde_home/foo
    """
    parts = target.split("/")
    result = []
    for part in parts:
        if part.startswith("."):
            result.append("dot_" + part[1:])
        elif part == "~":
            result.append("tilde_home")
        else:
            result.append(part)
    return "/".join(result)


def re_add_noctalia() -> GitResult:
    """Re-add noctalia settings so chezmoi source stays in sync with live files."""
    noctalia_dir = Path.home() / ".config" / "noctalia"
    if not noctalia_dir.is_dir():
        return GitResult(success=True)

    managed = _chezmoi_managed_set()

    targets = []
    for f in noctalia_dir.rglob("*"):
        if not f.is_file():
            continue
        rel = str(f.relative_to(Path.home()))  # .config/noctalia/theme-profile.toml
        source_rel = _target_to_source_path(rel)
        if rel in managed or source_rel in managed:
            targets.append(str(f))

    if not targets:
        return GitResult(success=True)
    try:
        result = subprocess.run(
            ["chezmoi", "re-add"] + targets,
            capture_output=True,
            text=True,
            timeout=30,
        )
        return GitResult(
            success=result.returncode == 0,
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )
    except subprocess.TimeoutExpired:
        return GitResult(
            success=False, stderr="chezmoi re-add timed out", returncode=-1
        )
    except FileNotFoundError:
        return GitResult(success=False, stderr="chezmoi not found", returncode=-2)


def chezmoi_apply(timeout: int = 120) -> GitResult:
    try:
        result = subprocess.run(
            ["chezmoi", "apply", "--force"],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return GitResult(
            success=result.returncode == 0,
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )
    except subprocess.TimeoutExpired:
        return GitResult(success=False, stderr="chezmoi apply timed out", returncode=-1)
    except FileNotFoundError:
        return GitResult(success=False, stderr="chezmoi not found", returncode=-2)
