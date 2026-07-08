from pathlib import Path
from typing import Optional

from . import ui
from .chezmoi import GitResult, _git


def has_conflict(repo_path: Path) -> bool:
    r = _git(repo_path, ["status", "--porcelain"])
    if not r.success:
        return False
    for line in r.stdout.splitlines():
        if line.startswith("UU"):
            return True
    return False


def get_conflicted_files(repo_path: Path) -> list[str]:
    r = _git(repo_path, ["diff", "--name-only", "--diff-filter=U"])
    if not r.success:
        return []
    return [f for f in r.stdout.splitlines() if f.strip()]


def is_rebasing(repo_path: Path) -> bool:
    return (repo_path / ".git" / "REBASE_HEAD").exists()


def rebase_abort(repo_path: Path) -> GitResult:
    return _git(repo_path, ["rebase", "--abort"])


def rebase_skip(repo_path: Path) -> GitResult:
    return _git(repo_path, ["rebase", "--skip"])


def merge_abort(repo_path: Path) -> GitResult:
    return _git(repo_path, ["merge", "--abort"])


def stash_push(repo_path: Path) -> GitResult:
    return _git(repo_path, ["stash", "push", "-m", "dsync-auto-stash"])


def stash_pop(repo_path: Path) -> GitResult:
    return _git(repo_path, ["stash", "pop"])


def check_diverged(repo_path: Path, branch: str = "main") -> tuple[int, int]:
    """Returns (ahead, behind) counts vs origin/<branch>"""
    r = _git(repo_path, ["rev-list", "--count", "--left-right", f"HEAD...origin/{branch}"])
    if not r.success:
        return (0, 0)
    parts = r.stdout.split()
    if len(parts) != 2:
        return (0, 0)
    try:
        return int(parts[0]), int(parts[1])
    except ValueError:
        return (0, 0)


def safe_pull(repo_path: Path, branch: str = "main") -> tuple[GitResult, bool]:
    """Pull with conflict detection. Returns (result, had_conflict)."""
    r = _git(repo_path, ["pull", "--rebase", "origin", branch], timeout=60)

    if r.success:
        return (r, False)

    if is_rebasing(repo_path):
        rebase_abort(repo_path)
        return (r, True)

    if has_conflict(repo_path):
        if is_rebasing(repo_path):
            rebase_abort(repo_path)
        else:
            merge_abort(repo_path)
        return (r, True)

    return (r, False)


def show_diff_summary(repo_path: Path, files: list[str], branch: str = "main"):
    """Show diff between local and remote changes."""
    ui.print_section("📄 Различия между локальной и удалённой версией")

    ui.print_info("Локальные коммиты (чего нет на GitHub):")
    log_r = _git(repo_path, ["log", f"origin/{branch}..HEAD", "--oneline", "--no-decorate"])
    if log_r.success and log_r.stdout:
        for line in log_r.stdout.splitlines():
            ui.print_info(f"  {line}")
    else:
        ui.print_info("  (нет локальных изменений)")

    ui.print_info("\nУдалённые коммиты (чего нет локально):")
    log_r = _git(repo_path, ["log", f"HEAD..origin/{branch}", "--oneline", "--no-decorate"])
    if log_r.success and log_r.stdout:
        for line in log_r.stdout.splitlines():
            ui.print_info(f"  {line}")
    else:
        ui.print_info("  (нет удалённых изменений)")

    if files:
        ui.print_section("📄 Конфликтующие файлы")
        for f in files:
            ui.print_warn(f"  {f}")

        for f in files:
            ui._print()
            ui.print_info(f"Изменения в {ui.bold(f)}:")
            ui.print_info("  --- локальная версия (ours):")
            diff = _git(repo_path, ["diff", f"origin/{branch}..HEAD", "--", f])
            if diff.success and diff.stdout:
                for line in diff.stdout.splitlines()[-30:]:
                    ui._print(f"  {line}")
            ui.print_info("  --- удалённая версия (theirs):")
            diff = _git(repo_path, ["diff", f"HEAD..origin/{branch}", "--", f])
            if diff.success and diff.stdout:
                for line in diff.stdout.splitlines()[-30:]:
                    ui._print(f"  {line}")


def resolve_interactive(repo_path: Path, branch: str = "main") -> Optional[bool]:
    """Interactive conflict resolution.
    Returns: True (keep local/ours), False (keep remote/theirs), None (abort).
    """
    files = get_conflicted_files(repo_path)

    if not files and not is_rebasing(repo_path):
        return True

    if is_rebasing(repo_path):
        rebase_abort(repo_path)

    ui.print_warn("Обнаружен конфликт в git!")
    ui._print()

    ahead, behind = check_diverged(repo_path, branch)
    ui.print_info(f"Локальные коммиты (ahead): {ahead}")
    ui.print_info(f"Удалённые коммиты (behind): {behind}")
    ui._print()

    show_diff_summary(repo_path, get_conflicted_files(repo_path), branch)
    ui._print()

    ui.print_info("Выбери действие:")
    ui.print_info(f"  {ui.green('1')} — оставить {ui.bold('локальную')} версию (git pull -X ours)")
    ui.print_info(f"  {ui.green('2')} — оставить {ui.bold('удалённую')} версию (git reset --hard origin/{branch})")
    ui.print_info(f"  {ui.green('3')} — {ui.bold('отменить')} синхронизацию (ничего не менять)")

    try:
        choice = input(f"\n  {ui.bold('Выбор [1/2/3]')}: ").strip()
    except (EOFError, KeyboardInterrupt):
        ui._print()
        ui.print_warn("Отменено")
        return None

    if choice == "1":
        ui.print_info("Оставляем локальную версию...")
        r = _git(repo_path, ["fetch", "origin"])
        if not r.success:
            ui.print_error(f"fetch: {r.stderr}")
            return None
        r = _git(repo_path, ["pull", "-X", "ours", "origin", branch], timeout=60)
        if r.success:
            ui.print_ok("Конфликт разрешён: оставлена локальная версия")
            return True
        ui.print_error(f"Pull failed: {r.stderr}")
        return None

    elif choice == "2":
        ui.print_info("Оставляем удалённую версию...")
        r = _git(repo_path, ["fetch", "origin"])
        if not r.success:
            ui.print_error(f"fetch: {r.stderr}")
            return None
        r = _git(repo_path, ["reset", "--hard", f"origin/{branch}"])
        if r.success:
            ui.print_ok("Конфликт разрешён: оставлена удалённая версия")
            return False
        ui.print_error(f"Reset failed: {r.stderr}")
        return None

    elif choice == "3":
        ui.print_warn("Синхронизация отменена")
        return None

    ui.print_error("Неверный выбор")
    return None
