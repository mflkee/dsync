import json
import logging
import os
import shutil
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


def re_add_modified() -> GitResult:
    """Re-add ALL chezmoi-managed files that differ from the source state.

    Uses `chezmoi status` to find modified files, then `chezmoi re-add`s them.
    This catches changes made through Noctalia, GUI settings, or direct edits.
    """
    try:
        status_r = subprocess.run(
            ["chezmoi", "status"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if status_r.returncode != 0:
            return GitResult(success=False, stderr=f"chezmoi status: {status_r.stderr[:200]}",
                             returncode=status_r.returncode)

        # chezmoi status outputs lines like: " M .config/niri/config.kdl"
        targets = []
        for line in status_r.stdout.splitlines():
            stripped = line.strip()
            if not stripped:
                continue
            # First char is status (M, A, D, R), skip only R (already removed)
            if len(stripped) < 3:
                continue
            flag = stripped[0]
            path = stripped[2:].strip()
            if flag in ("M", "A") and path:
                targets.append(Path.home() / path)

        if not targets:
            return GitResult(success=True)

        logger.info("re-add %d modified chezmoi files: %s", len(targets),
                     ", ".join(str(t.relative_to(Path.home())) for t in targets[:5]))
        result = subprocess.run(
            ["chezmoi", "re-add"] + [str(t) for t in targets],
            capture_output=True,
            text=True,
            timeout=60,
        )
        return GitResult(
            success=result.returncode == 0,
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )
    except subprocess.TimeoutExpired:
        return GitResult(success=False, stderr="chezmoi re-add timed out", returncode=-1)
    except FileNotFoundError:
        return GitResult(success=False, stderr="chezmoi not found", returncode=-2)


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


def tmux_export(dest: Path) -> GitResult:
    """Export current tmux session state to a file in the dotfiles repo.

    Copies the latest tmux-resurrect save to dest.
    Works only if tmux is running and tmux-resurrect is installed.
    """
    resurrect_dir = Path.home() / ".local" / "share" / "tmux" / "resurrect"
    last = resurrect_dir / "last"
    if not last.exists():
        return GitResult(success=True, stdout="no tmux-resurrect data")

    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(last.resolve(), dest)
    return GitResult(success=True, stdout="tmux session exported")


def tmux_import(source: Path) -> GitResult:
    """Import tmux session state from a dotfiles file to the resurrect directory.

    Copies source to the tmux-resurrect 'last' symlink target.
    The session will be restored on next tmux start by tmux-continuum.
    """
    if not source.exists():
        return GitResult(success=True, stdout="no tmux session to import")

    resurrect_dir = Path.home() / ".local" / "share" / "tmux" / "resurrect"
    resurrect_dir.mkdir(parents=True, exist_ok=True)

    last = resurrect_dir / "last"
    if last.is_symlink():
        last.unlink()
    elif last.exists():
        last.unlink()

    dest = resurrect_dir / source.name
    shutil.copy2(source, dest)
    last.symlink_to(dest.name)
    return GitResult(success=True, stdout="tmux session imported")


def tmux_theme_sync(dest: Path) -> GitResult:
    """Generate tmux color theme from current Noctalia theme.

    Reads ~/.config/noctalia/colors.json (Material Design 3 format),
    maps to tmux dracula color names, and writes an @dracula-colors
    config snippet that can be sourced by tmux.conf.
    """
    colors_path = Path.home() / ".config" / "noctalia" / "colors.json"
    if not colors_path.exists():
        return GitResult(success=True, stdout="no noctalia colors")

    try:
        with open(colors_path) as f:
            noctalia = json.load(f)
    except (json.JSONDecodeError, OSError):
        return GitResult(success=False, stderr="failed to read noctalia colors")

    m = noctalia
    mapping = {
        "dark_gray": m.get("mSurface", "#1a1b26"),
        "white": m.get("mOnSurface", "#c0caf5"),
        "gray": m.get("mSurfaceVariant", "#24283b"),
        "light_purple": m.get("mPrimary", "#7aa2f7"),
        "dark_purple": m.get("mSecondary", "#bb9af7"),
        "green": m.get("mTertiary", "#9ece6a"),
        "red": m.get("mError", "#f7768e"),
        "yellow": m.get("mPrimary", "#7aa2f7"),
        "cyan": m.get("mSecondary", "#bb9af7"),
        "orange": m.get("mError", "#f7768e"),
        "pink": m.get("mSecondary", "#bb9af7"),
    }

    lines = [f"{k}='{v}'" for k, v in mapping.items()]
    colors_joined = " ".join(lines)
    content = f"set -g @dracula-colors \"{colors_joined}\"\n"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(content)
    return GitResult(success=True, stdout="tmux theme synced")
