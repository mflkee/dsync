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


def _git_with_retry(repo_path: Path, args: list[str], timeout: int = 30, retries: int = 1) -> GitResult:
    """Run git with optional retry for transient network failures."""
    for attempt in range(retries):
        r = _git(repo_path, args, timeout=timeout)
        if r.success:
            return r
        err = r.stderr.lower()
        if "could not read username" in err or "no such device or address" in err:
            # HTTPS auth failure — no point retrying
            r.stderr = f"git auth failed (HTTPS without credentials): {r.stderr[:120]}"
            return r
        if "timed out" in err or "connection refused" in err or "no route to host" in err:
            if attempt < retries - 1:
                logger.info("git %s transient failure, retrying (%d/%d)", args[0], attempt + 1, retries)
                import time
                time.sleep(2 ** attempt)
                continue
        return r
    return r


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
    logger.info("git commit: %s (cwd=%s)", message, repo_path)
    add = _git(repo_path, ["add", "-A"])
    if not add.success:
        return add
    r = _git(repo_path, ["commit", "-m", message])
    if r.success:
        logger.info("git commit: ok — %s", r.stdout.strip()[:100])
    else:
        logger.warning("git commit: failed — %s", r.stderr.strip()[:200])
    return r


def pull(repo_path: Path, branch: str = "main") -> GitResult:
    return _git(repo_path, ["pull", "--rebase", "origin", branch], timeout=60)


def fetch(repo_path: Path) -> GitResult:
    logger.info("git fetch origin (cwd=%s)", repo_path)
    r = _git(repo_path, ["fetch", "origin"], timeout=30)
    if r.success:
        logger.info("git fetch: ok")
    else:
        logger.warning("git fetch: failed — %s", r.stderr.strip()[:200])
    return r


def get_remote_url(repo_path: Path, remote: str = "origin") -> str | None:
    """Return the URL of the given git remote, or None if it does not exist."""
    r = _git(repo_path, ["remote", "get-url", remote])
    if r.success:
        return r.stdout
    return None


def push(repo_path: Path, branch: str = "main") -> GitResult:
    logger.info("git push origin %s (cwd=%s)", branch, repo_path)
    r = _git(repo_path, ["push", "origin", branch], timeout=180)
    if r.success:
        logger.info("git push: ok")
    else:
        logger.warning("git push: failed — %s", r.stderr.strip()[:200])
    return r


def diverts_check(repo_path: Path, branch: str = "main") -> tuple[int, int]:
    """Returns (ahead, behind) vs origin/<branch>."""
    r = _git(
        repo_path, ["rev-list", "--count", "--left-right", f"HEAD...origin/{branch}"]
    )
    if not r.success:
        logger.warning("diverts_check: failed — %s", r.stderr.strip()[:100])
        return (0, 0)
    parts = r.stdout.split()
    if len(parts) != 2:
        return (0, 0)
    try:
        ahead, behind = int(parts[0]), int(parts[1])
        logger.info("diverts_check: ahead=%d behind=%d (branch=%s)", ahead, behind, branch)
        return ahead, behind
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


def chezmoi_apply(timeout: int = 300) -> GitResult:
    logger.info("chezmoi apply --force (timeout=%ds)", timeout)
    try:
        result = subprocess.run(
            ["chezmoi", "apply", "--force"],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if result.returncode == 0:
            logger.info("chezmoi apply: ok")
        else:
            logger.warning(
                "chezmoi apply: failed (rc=%d) — %s",
                result.returncode,
                result.stderr.strip()[:200],
            )
        return GitResult(
            success=result.returncode == 0,
            stdout=result.stdout.strip(),
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )
    except subprocess.TimeoutExpired:
        logger.warning("chezmoi apply: timed out after %ds", timeout)
        return GitResult(success=False, stderr="chezmoi apply timed out", returncode=-1)
    except FileNotFoundError:
        logger.warning("chezmoi apply: chezmoi not found")
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

    For tokyo-night-tmux: generates ~/.config/dsync/tokyo-night-theme-override.sh
    which overrides THEME colors after themes.sh runs, then runs
    tmux-tokyo-night-theme-apply to patch themes.sh and set tmux options.
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
        "background": m.get("mSurface", "#1a1b26"),
        "foreground": m.get("mOnSurface", "#c0caf5"),
        "black": m.get("mSurface", "#1a1b26"),
        "blue": m.get("mPrimary", "#7aa2f7"),
        "cyan": m.get("mSecondary", "#bb9af7"),
        "green": m.get("mTertiary", "#9ece6a"),
        "magenta": m.get("mSecondary", "#bb9af7"),
        "red": m.get("mError", "#f7768e"),
        "white": m.get("mOnSurface", "#c0caf5"),
        "yellow": m.get("mPrimary", "#7aa2f7"),
        "bblack": m.get("mSurfaceVariant", "#24283b"),
        "bblue": m.get("mPrimary", "#7aa2f7"),
        "bcyan": m.get("mSecondary", "#bb9af7"),
        "bgreen": m.get("mTertiary", "#9ece6a"),
        "bmagenta": m.get("mSecondary", "#bb9af7"),
        "bred": m.get("mError", "#f7768e"),
        "bwhite": m.get("mOnSurfaceVariant", "#9aa5ce"),
        "byellow": m.get("mPrimary", "#7aa2f7"),
        "ghgreen": "#3fb950",
        "ghmagenta": "#A371F7",
        "ghred": "#d73a4a",
        "ghyellow": "#d29922",
    }

    lines = [
        "#!/usr/bin/env bash",
        "# Auto-generated from Noctalia colors — overrides THEME after themes.sh runs",
        "declare -A THEME=(",
    ]
    for k, v in mapping.items():
        lines.append(f'    ["{k}"]="{v}"')
    lines.append(")")
    lines.append("")
    lines.append('RESET="#[fg=${THEME[foreground]},bg=${THEME[background]},nobold,noitalics,nounderscore,nodim]"')
    lines.append("")

    override = Path.home() / ".config" / "dsync" / "tokyo-night-theme-override.sh"
    override.parent.mkdir(parents=True, exist_ok=True)
    override.write_text("\n".join(lines))

    # Also write a marker to the old tmux-theme.conf location for backward compat
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text("# Tokyo Night theme is now managed by tmux-tokyo-night-theme-apply\n")

    # Run the apply script to patch themes.sh and set tmux options
    apply_script = Path.home() / ".local" / "bin" / "tmux-tokyo-night-theme-apply"
    if apply_script.exists():
        try:
            subprocess.run(
                [str(apply_script)],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass

    # Reload tokyo-night-tmux in the running tmux server so new colors apply immediately
    tokyo_script = Path.home() / ".tmux" / "plugins" / "tokyo-night-tmux" / "tokyo-night.tmux"
    if tokyo_script.exists():
        try:
            subprocess.run(
                ["tmux", "run-shell", str(tokyo_script)],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass

    return GitResult(success=True, stdout="tmux theme synced")


def starship_theme_sync(dest: Path) -> GitResult:
    """Generate starship.toml color theme from current Noctalia theme.

    Reads ~/.config/noctalia/colors.json and writes a starship.toml snippet
    that customizes the prompt colors to match the active Noctalia theme.
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
    primary = m.get("mPrimary", "#7aa2f7")
    secondary = m.get("mSecondary", "#bb9af7")
    tertiary = m.get("mTertiary", "#9ece6a")
    error = m.get("mError", "#f7768e")
    surface = m.get("mSurface", "#1a1b26")
    on_surface = m.get("mOnSurface", "#c0caf5")
    surface_variant = m.get("mSurfaceVariant", "#24283b")
    on_surface_variant = m.get("mOnSurfaceVariant", "#9aa5ce")

    lines = [
        "# Auto-generated by dsync from Noctalia colors",
        "# Do not edit manually — it will be overwritten on next sync",
        "",
        "palette = \"noctalia\"",
        "",
        "[palettes.noctalia]",
        f'primary = "{primary}"',
        f'secondary = "{secondary}"',
        f'tertiary = "{tertiary}"',
        f'error = "{error}"',
        f'surface = "{surface}"',
        f'on_surface = "{on_surface}"',
        f'surface_variant = "{surface_variant}"',
        f'on_surface_variant = "{on_surface_variant}"',
        "",
        "[character]",
        'success_symbol = "[>](bold primary)"',
        'error_symbol = "[>](bold error)"',
        'vicmd_symbol = "[<](bold secondary)"',
        "",
        "[directory]",
        "truncation_length = 3",
        "truncation_symbol = \"…/\"",
        'style = "bold primary"',
        "",
        "[git_branch]",
        f'style = "bold {secondary}"',
        'symbol = " "',
        "",
        "[git_status]",
        'style = "error"',
        'ahead = "⇡${count}"',
        'behind = "⇣${count}"',
        'diverged = "⇕${count}"',
        'conflicted = "=${count}"',
        'untracked = "?${count}"',
        'modified = "!${count}"',
        'staged = "+${count}"',
        'renamed = "»${count}"',
        'deleted = "✘${count}"',
        'stashed = "\\$${count}"',
        "",
        "[cmd_duration]",
        'style = "on_surface_variant"',
        'format = " took [$duration]($style) "',
        "",
        "[status]",
        'style = "error"',
        "symbol = \"✘\"",
        'success_symbol = ""',
        'format = "[$symbol$common_meaning$signal_name$maybe_int]($style) "',
        "map_symbol = true",
        "disabled = false",
        "",
        "[time]",
        'style = "on_surface_variant"',
        'format = "[$time]($style) "',
        "disabled = true",
        "",
        "[username]",
        "show_always = false",
        "disabled = true",
        "",
        "[hostname]",
        "ssh_only = true",
        'style = "secondary"',
        "disabled = false",
        "",
    ]

    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text("\n".join(lines))

    return GitResult(success=True, stdout="starship theme synced")
