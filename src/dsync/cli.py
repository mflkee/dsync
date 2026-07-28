import argparse
import concurrent.futures
import json
import logging
import os
import shlex
import subprocess
from datetime import datetime
from pathlib import Path

import lz4.block

from . import ui
from .chezmoi import (
    chezmoi_apply,
    commit,
    diverts_check,
    fetch,
    push,
    re_add_modified,
    tmux_export,
    tmux_import,
    tmux_theme_sync,
)
from .chezmoi import get_status as git_status
from .config import Config
from .conflict import (
    resolve_interactive,
    safe_pull,
)
from .hub_cli import (
    auto_self_update,
    cmd_hub_pull,
    cmd_hub_status,
    cmd_self_status,
    cmd_self_update,
)
from .log import setup_logging
from .netbird import Peer, get_status
from .project_cli import cmd_project_clone, cmd_project_status, cmd_project_sync
from .projects import remote_sync_script as project_remote_sync_script
from .projects import sync_project_repo
from .resolver import clear_cache, resolve_host
from .ssh_client import check_connectivity, check_port
from .ssh_client import run as ssh_run
from .zen import export_zen, find_profile, import_zen

logger = logging.getLogger(__name__)


def _run_script(name: str, cmd: list[str], timeout: int = 10) -> bool:
    """Run an external script, log result, return success."""
    logger.info("%s: starting %s", name, cmd[0])
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        if r.returncode == 0:
            logger.info("%s: ok — %s", name, r.stdout.strip()[:200])
        else:
            logger.warning(
                "%s: failed (rc=%d) — %s",
                name,
                r.returncode,
                r.stderr.strip()[:200],
            )
        return r.returncode == 0
    except OSError as e:
        logger.warning("%s: command not found — %s", name, e)
        return False
    except subprocess.TimeoutExpired:
        logger.warning("%s: timed out after %ds", name, timeout)
        return False


def _theme_export() -> bool:
    ok = _run_script("noctalia-export", ["noctalia-theme-export"])
    _run_script("zen-theme-export", ["zen-theme-export"])
    return ok


def _theme_apply() -> bool:
    ok = _run_script("noctalia-apply", ["noctalia-theme-apply"])
    _run_script("zen-theme-apply", ["zen-theme-apply"])
    return ok


def _log_theme_profile(repo: Path) -> None:
    """Log current theme profile state for diagnostics."""
    profile = repo / "dot_config" / "noctalia" / "theme-profile.toml"
    if profile.exists():
        content = profile.read_text()
        import hashlib

        h = hashlib.sha256(content.encode()).hexdigest()[:12]
        # Extract theme name if present
        theme_name = "unknown"
        for line in content.splitlines():
            if line.strip().startswith("name"):
                theme_name = line.split("=", 1)[-1].strip().strip('"')
                break
        logger.info(
            "theme profile: %s (hash=%s, lines=%d)",
            profile,
            h,
            len(content.splitlines()),
        )
        logger.info("theme profile name: %s", theme_name)
    else:
        logger.warning("theme profile not found: %s", profile)


def _sync_prep(config: Config, dry_run: bool) -> int:
    """Shared preparation: theme export + re-add + git commit. Returns 0 on success."""
    hostname = os.uname().nodename
    repo = config.git_source
    logger.info("sync_prep: hostname=%s repo=%s", hostname, repo)

    if not dry_run:
        _log_theme_profile(repo)
        if _theme_export():
            ui.print_ok("Тема экспортирована")
            _log_theme_profile(repo)
        else:
            ui.print_info("noctalia-theme-export: пропускаю (не найден или ошибка)")

        ui.print_section("zen browser")
        zen_dest = repo / "dot_config" / "dsync" / "zen.json"
        with ui.spinner_ctx("Экспорт Zen Browser..."):
            if export_zen(zen_dest):
                ui.print_ok("Zen профиль экспортирован")
            else:
                ui.print_info("Zen Browser не найден — пропускаю")

        ui.print_section("tmux session")
        tmux_dest = repo / "dot_config" / "dsync" / "tmux-session.txt"
        with ui.spinner_ctx("Экспорт tmux сессии..."):
            r = tmux_export(tmux_dest)
        if r.success and "no tmux" not in r.stdout:
            ui.print_ok("Tmux сессия экспортирована")
        else:
            ui.print_info("Tmux не запущен — пропускаю")

        ui.print_section("tmux theme")
        theme_dest = repo / "dot_config" / "dsync" / "tmux-theme.conf"
        with ui.spinner_ctx("Синхронизация темы tmux..."):
            r = tmux_theme_sync(theme_dest)
        if r.success:
            ui.print_ok("Tmux тема синхронизирована")
        else:
            ui.print_info(f"Tmux тема: {r.stderr[:100]}")

    if not dry_run:
        with ui.spinner_ctx("chezmoi re-add..."):
            r = re_add_modified()
        if r.success:
            ui.print_ok("chezmoi re-add — OK")
        elif r.stderr:
            ui.print_warn(f"chezmoi re-add: {r.stderr[:200]}")

    gs = git_status(repo)
    if gs.error:
        ui.print_error(f"Ошибка git: {gs.error}")
        return 1

    logger.info(
        "sync_prep: git status — clean=%s, ahead=%d, behind=%d, files_changed=%d",
        gs.is_clean,
        gs.ahead,
        gs.behind,
        gs.staged + gs.unstaged + gs.untracked,
    )

    if gs.is_clean:
        ui.print_ok("Локально чисто")
    elif not dry_run:
        ui.print_section("local changes")
        with ui.spinner_ctx("Коммит..."):
            msg = f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
            r = commit(repo, msg)
        if r.success:
            ui.print_ok("Закоммичено")
        elif r.stderr:
            ui.print_error(f"Ошибка коммита: {r.stderr}")
            return 1
    else:
        ui.print_info("Есть локальные изменения — будут закоммичены")
    return 0


def _filter_machines(config: Config, only: list[str]) -> dict:
    """Return machines from config filtered by names; empty filter = all."""
    machines = config.machines
    if not only:
        return machines
    unknown = [n for n in only if n not in machines]
    for n in unknown:
        ui.print_warn(f"Машина '{n}' не найдена в конфиге")
    return {n: i for n, i in machines.items() if n in only}


def _remote_sync_script(repo_path: str, branch: str, remote_url: str) -> str:
    q_repo = shlex.quote(repo_path)
    q_branch = shlex.quote(branch)
    q_url = shlex.quote(remote_url) if remote_url else ""
    age_key_src = shlex.quote(
        os.path.join(repo_path, "private_dot_config", "chezmoi", "key.txt")
    )
    return f"""export PATH="$HOME/.local/bin:$PATH"
exec 2>&1
C="$(command -v chezmoi)"
if [ -d {q_repo}/.git ]; then
  cd {q_repo}
  git fetch {q_url} {q_branch} || true
  git reset HEAD . 2>/dev/null || true
  git checkout -- . 2>/dev/null || true
  git clean -fd 2>/dev/null || true
  if ! git pull --rebase {q_url} {q_branch}; then
    git rebase --abort 2>/dev/null || true
    git reset --hard FETCH_HEAD || true
  fi
else
  rm -rf {q_repo} && git clone {q_url} {q_repo} || {{ echo "CLONE_FAIL"; exit 0; }}
fi
if [ -z "$C" ]; then echo "NO_CHEZMOI"; exit 0; fi
# Ensure chezmoi sourceDir points to the right place
if [ "$("$C" source-path 2>/dev/null)" != "{repo_path}" ]; then
  mkdir -p "$HOME/.config/chezmoi"
  CFG="$HOME/.config/chezmoi/chezmoi.toml"
  python3 -c "
cfg = open('$CFG').read() if __import__('os').path.exists('$CFG') else ''
lines = cfg.split(chr(10))
# Remove any existing sourceDir from anywhere
lines = [l for l in lines if not l.startswith('sourceDir ')]
# Prepend sourceDir at the top
lines.insert(0, 'sourceDir = \"{repo_path}\"')
open('$CFG', 'w').write(chr(10).join([l for l in lines if l]))
" 2>/dev/null || true
fi
# Copy age key from source state if missing
if [ ! -f "$HOME/.config/chezmoi/key.txt" ]; then
  if [ -f {age_key_src} ]; then
    cp {age_key_src} "$HOME/.config/chezmoi/key.txt"
    chmod 600 "$HOME/.config/chezmoi/key.txt"
  fi
fi
dsync self update 2>/dev/null || true
# Apply chezmoi changes
CHEZMOI_ERR=""
if ! out=$(cd {q_repo} && "$C" apply --force 2>&1); then
  CHEZMOI_ERR="chezmoi: $out"
elif ! out=$(cd {q_repo} && "$C" apply 2>&1); then
  CHEZMOI_ERR="chezmoi: $out"
fi
# Apply noctalia + zen themes
THEME_ERR=""
if ! out=$(noctalia-theme-apply 2>&1); then
  THEME_ERR="theme: $out"
fi
if ! out=$(zen-theme-apply 2>&1); then
  THEME_ERR="${{THEME_ERR:+$THEME_ERR; }}zen-theme: $out"
fi
# Apply noctalia overrides
if ! out=$(~/.local/bin/noctalia-overrides.sh 2>&1); then
  THEME_ERR="${{THEME_ERR:+$THEME_ERR; }}overrides: $out"
fi
# Import Zen Browser profile
dsync zen import 2>/dev/null || true
# Import tmux session
dsync tmux import 2>/dev/null || true
# Report errors
if [ -n "$CHEZMOI_ERR" ] || [ -n "$THEME_ERR" ]; then
  echo "SYNC_PARTIAL"
  [ -n "$CHEZMOI_ERR" ] && echo "  $CHEZMOI_ERR"
  [ -n "$THEME_ERR" ] && echo "  $THEME_ERR"
fi
exit 0"""


def _sync_one_machine(
    item: tuple[str, dict],
    nb,
    repo_path: str,
    branch: str,
    remote_url: str,
    dry_run: bool,
) -> tuple[str, str, str]:
    """Sync a single machine. Returns (name, status, note)."""
    name, info = item
    host = info.get("host", "")
    user = info.get("user", "mflkee")

    if nb.is_self(host):
        return name, "skipped", "это эта машина"

    ip = resolve_host(host)
    if not ip or not check_port(ip):
        logger.info("remote sync[%s]: offline (ip=%s)", name, ip)
        return name, "skipped", "офлайн"

    if dry_run:
        return name, "success", "будет синхронизировано"

    logger.info("remote sync[%s]: starting (host=%s, ip=%s)", name, host, ip)
    rcmd = _remote_sync_script(repo_path, branch, remote_url)
    r = ssh_run(ip, rcmd, user=user, timeout=300, retries=3)

    if r.stdout:
        for line in r.stdout.strip().splitlines():
            logger.info("remote sync[%s] stdout: %s", name, line[:200])

    if r.success and "NO_CHEZMOI" not in r.stdout:
        if "SYNC_PARTIAL" in r.stdout:
            note = "синхронизировано (с предупреждениями)"
            logger.info("remote sync[%s]: ok with warnings", name)
            return name, "success", note
        logger.info("remote sync[%s]: ok", name)
        return name, "success", ""

    if "NO_CHEZMOI" in r.stdout:
        return name, "skipped", "chezmoi не установлен"

    note = (
        r.stderr.replace("\n", "; ")[:200]
        if r.stderr
        else r.stdout.replace("\n", "; ")[:200]
    )
    if not note:
        note = f"код {r.returncode}"
    logger.warning("remote sync[%s]: failed — %s", name, note[:200])
    return name, "failed", note


def _sync_machines_parallel(
    machines: dict,
    nb,
    repo_path: str,
    branch: str,
    remote_url: str,
    dry_run: bool,
    jobs: int,
):
    """Sync machines in parallel, print per-machine results. Returns (ok, skip, fail)."""
    items = list(machines.items())
    total = len(items)

    if dry_run:
        ui.print_dry_run_header()

    def _sync(item: tuple[str, dict]) -> tuple[str, str, str]:
        return _sync_one_machine(item, nb, repo_path, branch, remote_url, dry_run)

    if jobs == 1 or total <= 1:
        results = [_sync(it) for it in items]
    else:
        workers = max(1, min(jobs, total))
        with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as ex:
            results = list(ex.map(_sync, items))

    ok = skip = fail = 0
    errors: list[tuple[str, str]] = []

    for idx, (name, status, note) in enumerate(results, 1):
        if not dry_run and status not in ("skipped",):
            ui.print_progress_bar(idx, total, name)
        ui.print_machine_result(name, status, note, dry_run)
        if status == "success":
            ok += 1
        elif status == "skipped":
            skip += 1
        else:
            errors.append((name, note))
            fail += 1

    ui.print_error_summary(errors)
    return ok, skip, fail


def resolve_conflict(repo: Path, branch: str, strategy: str) -> bool | None:
    """Resolve git conflict.
    Returns True (keep local), False (keep remote), None (abort).
    """
    from .chezmoi import _git

    if strategy == "theirs":
        r = _git(repo, ["fetch", "origin"])
        if r.success:
            r = _git(repo, ["merge", "-X", "theirs", f"origin/{branch}"], timeout=60)
        if r.success:
            ui.print_ok("Оставлена удалённая версия (theirs)")
            return False
        ui.print_error(f"Ошибка: {r.stderr[:200]}")
        return None

    if strategy == "abort":
        ui.print_warn("Синхронизация отменена из-за конфликта")
        return None

    if strategy == "ask":
        return resolve_interactive(repo, branch)

    # Default: "ours" — keep local version (fetch + merge -X ours)
    r = _git(repo, ["fetch", "origin"])
    if r.success:
        r = _git(repo, ["merge", "-X", "ours", f"origin/{branch}"], timeout=60)
    if r.success:
        ui.print_ok("Оставлена локальная версия (ours)")
        return True
    ui.print_error(f"Ошибка: {r.stderr[:200]}")
    return None


def cmd_status(config: Config):
    clear_cache()
    ui.print_header()

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    ui.print_status_line("host", ui.bold(nb.self_hostname_short), nb.daemon_status)
    ui.print_info(f"IP: {nb.self_ip}  FQDN: {nb.self_fqdn}")

    gs = git_status(config.git_source)
    ui.print_section("chezmoi")
    if gs.error:
        ui.print_error(f"Git: {gs.error}")
    elif gs.is_clean:
        ui.print_ok("Чисто")
        if gs.ahead > 0:
            ui.print_info(f"Впереди на {gs.ahead} коммитов (нужен push)")
        if gs.behind > 0:
            ui.print_info(f"Позади на {gs.behind} коммитов (нужен pull)")
    else:
        parts = []
        if gs.staged:
            parts.append(f"{gs.staged} staged")
        if gs.unstaged:
            parts.append(f"{gs.unstaged} unstaged")
        if gs.untracked:
            parts.append(f"{gs.untracked} untracked")
        ui.print_warn(f"Изменения: {', '.join(parts)}")
        if gs.ahead > 0:
            ui.print_info(f"Впереди на {gs.ahead} коммитов")

    ui.print_section("netbird peers")
    table_cols = ["Имя", "Статус", "FQDN", "IP", "Лат.", "Тип"]
    table_rows = []
    for peer in nb.peers:
        name = peer.hostname_short
        s = ui.status_dot(peer.status)
        table_rows.append(
            [
                name,
                f"{s} {peer.status}",
                peer.fqdn,
                peer.netbird_ip,
                peer.latency_ms,
                peer.connection_type,
            ]
        )
    ui.print_table(table_cols, table_rows)

    ui.print_section("target machines")
    machines = config.machines
    if not machines:
        ui.print_warn("Машины не настроены. Используй: dsync add <имя> <host>")
    else:
        from .obsidian import check_api as obs_check

        online = []
        offline = []
        for name, info in machines.items():
            host = info["host"]
            ui.print_info(f"  {name} → {host}")
            if nb.is_self(host):
                online.append(name)
                ui.print_ok(f"    ok  Connected (self)  IP: {nb.self_ip}")
            else:
                peer = next((p for p in nb.peers if p.fqdn == host), None)
                if peer:
                    if peer.is_connected:
                        online.append(name)
                        ui.print_ok(f"    ok  {peer.status}  IP: {peer.netbird_ip}")
                    else:
                        offline.append(name)
                        ui.print_warn(f"    down  {peer.status}")
                else:
                    offline.append(name)
                    ui.print_warn("    --  not in netbird")

        # Obsidian health on online machines
        ui.print_section("obsidian REST API")
        obs_ok = obs_warn = 0
        for name in online:
            info = machines[name]
            host = info["host"]
            ip = resolve_host(host)
            if not ip or not check_port(ip):
                continue
            status = obs_check(ip, user=info.get("user", "mflkee"))
            if status.api_responsive:
                ui.print_ok(f"  {name}: OK (HTTP {status.http_code})")
                obs_ok += 1
            elif status.service_active:
                ui.print_warn(f"  {name}: REST API не отвечает (сервис запущен)")
                obs_warn += 1
            else:
                ui.print_info(f"  {name}: сервис не установлен")
        if not obs_ok and not obs_warn:
            ui.print_info("  нет доступных машин для проверки")

    return 0


def cmd_sync(
    config: Config,
    strategy: str = "",
    self_update: bool = True,
    run_hub: bool = True,
    only: list[str] | None = None,
    dry_run: bool = False,
    jobs: int = 4,
):
    clear_cache()
    ui.print_header()
    logger.info("sync started (dry=%s, strategy=%s)", dry_run, strategy or "ours")
    if dry_run:
        ui.print_panel("dry-run", "Изменения не применяются, только отчёт", style="yellow")

    hostname = os.uname().nodename
    repo = config.git_source
    branch = config.git_branch
    remote_url = config.git_remote_url or "https://github.com/mflkee/dotfiles.git"

    if self_update and not dry_run:
        ui.print_section("self update")
        auto_self_update()

    if _sync_prep(config, dry_run) != 0:
        return 1

    ui.print_section("github sync")
    with ui.spinner_ctx("Fetch..."):
        fetch(repo)

    ahead, behind = diverts_check(repo, branch)
    logger.info("github sync: ahead=%d behind=%d", ahead, behind)
    if behind > 0:
        ui.print_info(f"Удалённых коммитов: {behind}")
        if dry_run:
            ui.print_info("Будет выполнен pull")
        else:
            _log_theme_profile(repo)
            with ui.spinner_ctx("Pull..."):
                r, had_conflict = safe_pull(repo, branch)
            if r.success:
                ui.print_ok("Pull выполнен")
                logger.info("pull ok — stdout: %s", r.stdout.strip()[:200])
                _log_theme_profile(repo)
                # Re-export theme after pull — other machines may have pushed
                # their version; capture current local state so it's not lost.
                logger.info("post-pull: re-exporting theme to preserve local state")
                if _theme_export():
                    ui.print_ok("Тема ре-экспортирована после pull")
                    logger.info("post-pull: theme re-exported")
                else:
                    logger.info("post-pull: theme re-export skipped (noctalia not available)")
                with ui.spinner_ctx("chezmoi re-add..."):
                    r2 = re_add_modified()
                if r2.success:
                    ui.print_ok("chezmoi re-add — OK")
                gs_recheck = git_status(repo)
                if not gs_recheck.is_clean:
                    msg = f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
                    logger.info("post-pull: committing re-export changes")
                    commit(repo, msg)
            elif had_conflict:
                ui._print()
                ui.print_warn("Конфликт при pull!")
                res = resolve_conflict(repo, branch, strategy)
                if res is None:
                    return 1
                # Re-check after conflict resolution
                ahead, behind = diverts_check(repo, branch)
                # Auto-commit any uncommitted changes from merge resolution
                gs_after = git_status(repo)
                if not gs_after.is_clean:
                    msg = (
                        f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
                    )
                    commit(repo, msg)
            else:
                ui.print_warn(f"Pull: {r.stderr[:200]}")
    else:
        ui.print_ok("Pull не требуется")

    if ahead > 0:
        if dry_run:
            ui.print_info(f"Локальных коммитов: {ahead} — будут запушены")
        else:
            with ui.spinner_ctx("Push..."):
                r = push(repo, branch)
            if r.success:
                ui.print_ok("Запущено на GitHub")
                logger.info("push ok")
            else:
                ui.print_error(f"Ошибка push: {r.stderr}")
                logger.error("push failed: %s", r.stderr[:200])
                return 1

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен — не могу синхронизировать удалённые машины")
        return 1

    machines = _filter_machines(config, only or [])
    if not machines:
        ui.print_warn("Нет машин в конфиге для удалённой синхронизации")
        return 0

    ui.print_section("remote sync")
    success_count, skip_count, fail_count = _sync_machines_parallel(
        machines, nb, str(repo), branch, remote_url, dry_run, jobs
    )

    # Syncthing health check (after remote sync, only for successfully synced machines)
    if not dry_run:
        from .syncthing import health_check as st_health

        ui.print_section("syncthing health")
        st_ok = st_warn = 0
        for name, info in machines.items():
            host = info.get("host", "")
            ip = resolve_host(host)
            if not ip or not check_port(ip):
                continue
            st_status = st_health(
                ip,
                user=info.get("user", "mflkee"),
                auto_restart=True,
                auto_resolve=True,
            )
            if st_status.running:
                if st_status.conflicts:
                    ui.print_ok(
                        f"  {name}: OK ({len(st_status.conflicts)} конфликтов решено)"
                    )
                    st_ok += 1
                else:
                    ui.print_ok(f"  {name}: OK")
                    st_ok += 1
            else:
                ui.print_warn(f"  {name}: syncthing не работает")
                st_warn += 1
        if not st_ok and not st_warn:
            ui.print_info("  нет доступных машин для проверки")

    # Obsidian REST API health check
    if not dry_run:
        from .obsidian import health_check as obs_health

        ui.print_section("obsidian health")
        obs_ok = obs_warn = 0
        for name, info in machines.items():
            host = info.get("host", "")
            ip = resolve_host(host)
            if not ip or not check_port(ip):
                continue
            obs_status = obs_health(
                ip,
                user=info.get("user", "mflkee"),
                auto_restart=True,
            )
            if obs_status.api_responsive:
                ui.print_ok(f"  {name}: OK (HTTP {obs_status.http_code})")
                obs_ok += 1
            elif obs_status.service_active:
                ui.print_warn(
                    f"  {name}: REST API не отвечает"
                    + (f" ({obs_status.error[:60]})" if obs_status.error else "")
                )
                obs_warn += 1
            else:
                ui.print_info(f"  {name}: сервис не установлен")
        if not obs_ok and not obs_warn:
            ui.print_info("  нет доступных машин для проверки")

    # Project sync
    projects_dict = config.projects
    if projects_dict:
        ui.print_section("project sync")
        proj_ok = proj_fail = 0
        for pname, pinfo in projects_dict.items():
            ppath = Path(pinfo.get("path", "")).expanduser()
            pbranch = pinfo.get("branch", "main")
            if not ppath.exists() or not (ppath / ".git").is_dir():
                continue
            if dry_run:
                ui.print_info(f"{pname}: будет синхронизирован")
                proj_ok += 1
                continue
            r = sync_project_repo(ppath, pbranch)
            if r.success:
                ui.print_ok(f"{pname}: локально OK")
                proj_ok += 1
            else:
                ui.print_warn(f"{pname}: {r.stderr[:120]}")
                proj_fail += 1
                continue
            # Push project to remote machines
            ptarget = pinfo.get("machines")
            p_machines = {
                n: i for n, i in machines.items() if not ptarget or n in ptarget
            }
            for mname, minfo in p_machines.items():
                mhost = minfo["host"]
                if nb.is_self(mhost):
                    continue
                mpeer = next((p for p in nb.peers if p.fqdn == mhost), None)
                if mpeer and not mpeer.is_connected:
                    continue
                mip = resolve_host(mhost)
                if not check_port(mip):
                    continue
                script = project_remote_sync_script(
                    ppath.as_posix(), pbranch, pinfo.get("remote")
                )
                sr = ssh_run(mip, script, user=minfo.get("user", "mflkee"), timeout=300)
                if sr.success:
                    ui.print_ok(f"  {mname}: OK")
                else:
                    ui.print_warn(f"  {mname}: {sr.stderr[:80]}")

    ui.print_section("summary")
    if success_count:
        ui.print_ok(f"Успешно: {success_count}")
    if skip_count:
        ui.print_info(f"Пропущено (офлайн): {skip_count}")
    if fail_count:
        ui.print_error(f"Ошибок: {fail_count}")
    logger.info(
        "sync done: ok=%d skip=%d fail=%d",
        success_count,
        skip_count,
        fail_count,
    )

    if run_hub:
        cmd_hub_pull(config, local_only=False, dry_run=dry_run, jobs=jobs)

    if dry_run:
        ui.print_info("Режим dry-run: изменения не применены")
        return 0

    return 0 if fail_count == 0 else 1


def cmd_push(
    config: Config,
    strategy: str = "",
    only: list[str] | None = None,
    dry_run: bool = False,
    jobs: int = 4,
):
    clear_cache()
    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    machines = _filter_machines(config, only or [])
    if not machines:
        ui.print_error("Нет машин в конфиге")
        ui.print_info("Добавь: dsync add <имя> <host>")
        return 1

    ui.print_header()
    if dry_run:
        ui.print_panel("dry-run", "Изменения не применяются, только отчёт", style="yellow")

    hostname = os.uname().nodename
    repo = config.git_source
    branch = config.git_branch
    remote_url = config.git_remote_url or "https://github.com/mflkee/dotfiles.git"

    if _sync_prep(config, dry_run) != 0:
        return 1

    ui.print_section("github sync")
    with ui.spinner_ctx("Fetch..."):
        fetch(repo)

    ahead, behind = diverts_check(repo, branch)
    if behind > 0:
        ui.print_info(f"Удалённых коммитов: {behind}")
        if dry_run:
            ui.print_info("Будет выполнен pull")
        else:
            with ui.spinner_ctx("Pull..."):
                r, had_conflict = safe_pull(repo, branch)
            if r.success:
                ui.print_ok("Pull выполнен")
            elif had_conflict:
                ui._print()
                ui.print_warn("Конфликт при pull!")
                res = resolve_conflict(repo, branch, strategy)
                if res is None:
                    return 1
                ahead, behind = diverts_check(repo, branch)
                gs_after = git_status(repo)
                if not gs_after.is_clean:
                    msg = (
                        f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
                    )
                    commit(repo, msg)
    else:
        ui.print_ok("Pull не требуется")

    if ahead > 0:
        if dry_run:
            ui.print_info(f"Локальных коммитов: {ahead} — будут запушены")
        else:
            with ui.spinner_ctx("Push..."):
                r = push(repo, branch)
            if r.success:
                ui.print_ok("Запущено на GitHub")
            else:
                ui.print_error(f"Ошибка push: {r.stderr}")
                return 1

    ui.print_section("push to machines")
    ok_count, skip_count, fail_count = _sync_machines_parallel(
        machines, nb, str(repo), branch, remote_url, dry_run, jobs
    )

    if dry_run:
        ui.print_info("Режим dry-run: изменения не применены")

    return 0 if fail_count == 0 else 1


def cmd_pull(config: Config, strategy: str = "", dry_run: bool = False):
    ui.print_header()
    if dry_run:
        ui.print_panel(
            "dry-run", "Изменения не применяются, только отчёт", style="yellow"
        )
    repo = config.git_source
    branch = config.git_branch

    with ui.spinner_ctx("Fetch..."):
        fetch(repo)

    ahead, behind = diverts_check(repo, branch)
    if behind > 0:
        ui.print_info(f"Удалённых коммитов: {behind}")
        if dry_run:
            ui.print_info("Будет выполнен pull")
        else:
            with ui.spinner_ctx("Pull..."):
                r, had_conflict = safe_pull(repo, branch)
            if r.success:
                ui.print_ok("Git pull выполнен")
            elif had_conflict:
                ui._print()
                ui.print_warn("Конфликт при pull!")
                res = resolve_conflict(repo, branch, strategy)
                if res is None:
                    return 1
                # Re-check and auto-commit after resolution
                ahead, behind = diverts_check(repo, branch)
                gs_after = git_status(repo)
                if not gs_after.is_clean:
                    hostname = os.uname().nodename
                    msg = (
                        f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
                    )
                    commit(repo, msg)
    else:
        ui.print_ok("Уже актуально")

    if dry_run:
        ui.print_info("Будет выполнен chezmoi apply (dry-run)")
        return 0

    _log_theme_profile(repo)
    with ui.spinner_ctx("chezmoi apply..."):
        r = chezmoi_apply()
    if r.success:
        ui.print_ok("chezmoi apply — OK")
        logger.info("cmd_pull: chezmoi apply ok")
    else:
        ui.print_warn(
            f"chezmoi apply: {r.stderr[:200]}"
            if r.stderr
            else "chezmoi apply: предупреждения"
        )
        logger.warning("cmd_pull: chezmoi apply — %s", r.stderr[:200] if r.stderr else "warnings")

    if _theme_apply():
        ui.print_ok("Noctalia тема применена")
    _log_theme_profile(repo)

    zen_src = config.git_source / "dot_config" / "dsync" / "zen.json"
    if zen_src.exists():
        with ui.spinner_ctx("Импорт Zen Browser..."):
            if import_zen(zen_src):
                ui.print_ok("Zen профиль импортирован")
            else:
                ui.print_info("Zen Browser не найден — пропускаю")

    tmux_src = config.git_source / "dot_config" / "dsync" / "tmux-session.txt"
    if tmux_src.exists():
        with ui.spinner_ctx("Импорт tmux сессии..."):
            r = tmux_import(tmux_src)
        if r.success:
            ui.print_ok("Tmux сессия импортирована")
        else:
            ui.print_info("Tmux не запущен — пропускаю")

    return 0


def cmd_setup(config: Config):
    ui.print_header()
    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    machines = config.machines
    if not machines:
        ui.print_warn("Нет машин в конфиге. Сначала добавь: dsync add <имя> <host>")
        return 1

    ui.print_section("🔑 Настройка SSH доступа")
    identity = os.path.expanduser("~/.ssh/id_ed25519")
    if not os.path.exists(identity):
        ui.print_info("Генерация SSH-ключа...")
        subprocess.run(
            ["ssh-keygen", "-t", "ed25519", "-f", identity, "-N", ""], check=True
        )
        ui.print_ok("SSH-ключ создан")

    all_ok = True
    for name, info in machines.items():
        host = info["host"]
        user = info.get("user", "mflkee")
        ip = resolve_host(host)
        ui.print_info(f"\n  {name} ({user}@{host} → {ip})")

        if nb.is_self(host) or ip == nb.self_ip:
            ui.print_ok("Текущая машина, пропускаю")
            continue

        if not check_port(ip):
            ui.print_warn("  SSH порт 22 недоступен, пропускаю")
            continue

        # check if already accessible
        if check_connectivity(ip, user):
            ui.print_ok("Уже есть доступ")
            continue

        ui.print_info("  Копируем ключ...")
        try:
            r = subprocess.run(
                ["ssh-copy-id", "-i", identity, f"{user}@{ip}"],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except subprocess.TimeoutExpired:
            ui.print_warn("  Таймаут ssh-copy-id, пропускаю")
            all_ok = False
            continue
        if r.returncode == 0:
            ui.print_ok("Ключ скопирован")
        else:
            ui.print_error(f"Не удалось: {r.stderr[:200]}")
            all_ok = False

    ui.print_section("dns / hosts")

    home_ip = nb.self_ip
    ui.print_info(f"Текущая машина: {nb.self_fqdn} ({home_ip})")

    hosts_lines = []
    hosts_path = Path("/etc/hosts")
    if hosts_path.exists():
        hosts_lines = hosts_path.read_text().splitlines()

    for name, info in machines.items():
        host = info["host"]
        ip = resolve_host(host)
        if ip != host:
            entry = f"{ip}\t{host} {name}"
            if not any(host in line for line in hosts_lines):
                ui.print_info(
                    f"  Добавить в /etc/hosts: sudo sh -c 'echo \"{entry}\" >> /etc/hosts'"
                )

    if all_ok:
        ui.print_section("ssh configured")
        ui.print_info("Теперь можно запустить: dsync sync")
    else:
        ui.print_section("warning: some machines not configured")
        ui.print_info("Проверь пароль и доступность машин в NetBird")

    return 0


def cmd_add(config: Config, name: str, host: str, user: str):
    config.add_machine(name, host, user)
    ui.print_ok(f"Машина '{name}' → {user}@{host} добавлена")
    return 0


def cmd_remove(config: Config, name: str):
    machines = config.data.setdefault("machines", {})
    if name in machines:
        del machines[name]
        config._save()
        ui.print_ok(f"Машина '{name}' удалена")
    else:
        ui.print_error(f"Машина '{name}' не найдена")
    return 0


def cmd_timer(config: Config, enable: bool, disable: bool, mode: str = "pull"):
    unit_dir = Path.home() / ".config" / "systemd" / "user"
    unit_dir.mkdir(parents=True, exist_ok=True)

    service = unit_dir / "dsync.service"
    timer = unit_dir / "dsync.timer"

    if disable:
        subprocess.run(
            ["systemctl", "--user", "disable", "--now", "dsync.timer"],
            capture_output=True,
        )
        if service.exists():
            service.unlink()
        if timer.exists():
            timer.unlink()
        ui.print_ok("Таймер отключён")
        return 0

    dsync_bin = "dsync"
    exec_cmd = f"{dsync_bin} {mode}"
    service_content = f"""[Unit]
Description=dsync — dotfiles {mode}

[Service]
Type=oneshot
ExecStart={exec_cmd}
"""
    timer_content = """[Unit]
Description=Run dsync every 30 minutes

[Timer]
OnBootSec=5min
OnUnitActiveSec=30min
RandomizedDelaySec=5min

[Install]
WantedBy=timers.target
"""
    service.write_text(service_content)
    timer.write_text(timer_content)

    subprocess.run(["systemctl", "--user", "daemon-reload"], capture_output=True)
    subprocess.run(
        ["systemctl", "--user", "enable", "--now", "dsync.timer"], capture_output=True
    )
    ui.print_ok(f"Таймер включён ({mode} каждые 30 минут)")
    ui.print_info("Проверить: systemctl --user status dsync.timer")
    return 0


def cmd_discover(config: Config):
    ui.print_header()
    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    ui.print_section("discover netbird")
    peers_by_ip: dict[str, Peer] = {}
    peers_by_short: dict[str, Peer] = {}
    import re

    suffix_re = re.compile(r"-\d+-\d+$")

    def _register(key: str, peer):
        existing = peers_by_short.get(key)
        if existing is None:
            peers_by_short[key] = peer
        elif not existing.is_connected and peer.is_connected:
            peers_by_short[key] = peer

    for peer in nb.peers:
        if peer.fqdn == nb.self_fqdn:
            continue
        ip = peer.netbird_ip
        short = peer.hostname_short
        peers_by_ip[ip] = peer
        _register(short, peer)
        _register(short.replace("-", ""), peer)
        _register(suffix_re.sub("", short), peer)
        # Strip well-known prefixes so short aliases like "desktop" can match
        for prefix in config.discover_prefixes:
            if short.startswith(prefix):
                stripped = short[len(prefix) :]
                _register(stripped, peer)
                _register(suffix_re.sub("", stripped), peer)

    machines = config.data.setdefault("machines", {})
    updated = []
    skipped = []

    for name, info in list(machines.items()):
        host = info.get("host", "")
        user = info.get("user", "mflkee")

        # already an IP
        if host.replace(".", "").isdigit():
            skipped.append(name)
            continue

        short_host = host.removesuffix(".netbird.cloud")
        peer = peers_by_short.get(short_host, None)
        if peer is None:
            for short, p in peers_by_short.items():
                if name in short or short in name:
                    peer = p
                    break

        if peer is None:
            ui.print_warn(f"{name}: не найден в NetBird")
            continue

        new_host = peer.fqdn
        if host != new_host:
            ui.print_info(f"{name}: {host} → {new_host}")
            machines[name] = {"host": new_host, "user": user}
            updated.append(name)
        else:
            skipped.append(name)

    # optionally add new peers by short name
    for peer in nb.peers:
        short = peer.hostname_short
        if short in machines:
            continue
        if peer.fqdn == nb.self_fqdn:
            continue
        # Use aliases from config, fallback to empty dict
        aliases = config.discover_aliases
        prefixes = config.discover_prefixes

        name = None
        # Try to match against aliases that ignore the NetBird -<digits> suffix
        for prefix, alias in aliases.items():
            if short.startswith(prefix):
                name = alias
                break
            # also match if peer short is the new suffixed form (e.g. archlinux-desktop-12-158)
            normalized = re.sub(r"-\d+-\d+$", "", short)
            if normalized.startswith(prefix):
                name = alias
                break
        # Strip well-known prefixes so short aliases like "desktop" can match
        if name is None:
            for pfx in prefixes:
                if short.startswith(pfx):
                    stripped = short[len(pfx) :]
                    if stripped in machines:
                        continue
                    name = stripped
                    break
        if name is None:
            name = short
        if name not in machines:
            ui.print_info(f"Найдена новая машина: {name} → {peer.fqdn}")
            machines[name] = {"host": peer.fqdn, "user": "mflkee"}
            updated.append(name)

    if updated:
        config._save()
        ui.print_ok(f"Обновлено/добавлено: {', '.join(updated)}")
    else:
        ui.print_ok("Изменений не требуется")

    if skipped:
        ui.print_info(f"Без изменений: {', '.join(skipped)}")

    return 0


def _resolve_targets(config: Config, machines: list[str]) -> dict[str, dict]:
    """Resolve machine names to {name: {host, user, ...}} dict."""
    all_machines = config.machines
    if not machines:
        return {n: i for n, i in all_machines.items()}
    return {n: i for n, i in all_machines.items() if n in machines}


def cmd_syncthing(config: Config, action: str, machines: list[str]) -> int:
    from .syncthing import (
        check_conflicts,
        check_running,
    )
    from .syncthing import (
        resolve_conflicts as st_resolve,
    )
    from .syncthing import (
        restart as st_restart,
    )

    targets = _resolve_targets(config, machines)
    if not targets:
        ui.print_error("Нет доступных машин")
        return 1

    ui.print_header()

    if action == "status":
        ui.print_section("Syncthing status")
        for name, info in targets.items():
            ip = resolve_host(info["host"])
            if not ip or not check_port(ip):
                ui.print_warn(f"  {name}: офлайн")
                continue
            status = check_running(ip, user=info.get("user", "mflkee"))
            if status.running:
                ui.print_ok(f"  {name}: работает (uptime {status.uptime}s)")
                conflicts = check_conflicts(ip, user=info.get("user", "mflkee"))
                if conflicts:
                    for c in conflicts:
                        ui.print_warn(f"    {c['id']}: {c['conflicts']} конфликтов")
                else:
                    ui.print_ok("    конфликтов нет")
            else:
                ui.print_error(f"  {name}: не работает ({status.error[:60]})")
        return 0

    if action == "resolve":
        ui.print_section("Syncthing conflict resolution")
        total = 0
        for name, info in targets.items():
            ip = resolve_host(info["host"])
            if not ip or not check_port(ip):
                ui.print_warn(f"  {name}: офлайн")
                continue
            resolved = st_resolve(ip, user=info.get("user", "mflkee"))
            if resolved:
                ui.print_ok(f"  {name}: {resolved} конфликтов решено")
                total += resolved
            else:
                ui.print_ok(f"  {name}: конфликтов нет")
        if total:
            ui.print_ok(f"\nИтого: {total} конфликтов решено")
        return 0

    if action == "restart":
        ui.print_section("Syncthing restart")
        for name, info in targets.items():
            ip = resolve_host(info["host"])
            if not ip or not check_port(ip):
                ui.print_warn(f"  {name}: офлайн")
                continue
            ok = st_restart(ip, user=info.get("user", "mflkee"))
            if ok:
                ui.print_ok(f"  {name}: перезапущен")
            else:
                ui.print_error(f"  {name}: ошибка перезапуска")
        return 0

    return 1


def cmd_doctor(config: Config, jobs: int = 4) -> int:
    """Run comprehensive diagnostics on the dsync infrastructure."""
    ui.print_header()
    ui.print_section("doctor")
    ok = warn = fail = 0

    # --- NetBird ---
    ui.print_section("netbird")
    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        fail += 1
    else:
        ui.print_ok(f"Daemon: {nb.daemon_status}")
        ui.print_info(f"Self: {nb.self_fqdn} ({nb.self_ip})")
        connected = sum(1 for p in nb.peers if p.is_connected)
        ui.print_info(f"Peers: {len(nb.peers)} total, {connected} connected")
        ok += 1

    # --- Config ---
    ui.print_section("config")
    errors = config.validate()
    if errors:
        for e in errors:
            ui.print_error(e)
        fail += len(errors)
    else:
        ui.print_ok("Конфигурация валидна")
        ok += 1

    # --- chezmoi ---
    ui.print_section("chezmoi")
    import subprocess as _sp

    try:
        r = _sp.run(["chezmoi", "--version"], capture_output=True, text=True, timeout=5)
        if r.returncode == 0:
            ui.print_ok(f"chezmoi: {r.stdout.strip().splitlines()[0]}")
            ok += 1
        else:
            ui.print_error("chezmoi не найден")
            fail += 1
    except (OSError, _sp.TimeoutExpired):
        ui.print_error("chezmoi не найден")
        fail += 1

    gs = git_status(config.git_source)
    if gs.error:
        ui.print_error(f"dotfiles git: {gs.error}")
        fail += 1
    elif gs.is_clean:
        ui.print_ok("dotfiles: чисто")
        ok += 1
    else:
        parts = []
        if gs.staged:
            parts.append(f"{gs.staged} staged")
        if gs.unstaged:
            parts.append(f"{gs.unstaged} unstaged")
        if gs.untracked:
            parts.append(f"{gs.untracked} untracked")
        ui.print_warn(f"dotfiles: {', '.join(parts)}")
        warn += 1
    if gs.ahead > 0:
        ui.print_info(f"  ahead: {gs.ahead}")
    if gs.behind > 0:
        ui.print_info(f"  behind: {gs.behind}")

    # --- SSH ---
    ui.print_section("ssh")
    machines = config.machines
    if not machines:
        ui.print_warn("Нет машин в конфиге")
        warn += 1
    else:
        for name, info in machines.items():
            host = info["host"]
            user = info.get("user", "mflkee")
            if nb is not None and nb.is_self(host):
                ui.print_info(f"  {name}: текущая машина")
                ok += 1
                continue
            ip = resolve_host(host)
            if not ip or ip == host:
                ui.print_warn(f"  {name}: IP не найден ({host})")
                warn += 1
                continue
            if not check_port(ip):
                ui.print_warn(f"  {name}: порт 22 недоступен ({ip})")
                warn += 1
                continue
            if check_connectivity(ip, user):
                ui.print_ok(f"  {name}: SSH OK ({user}@{ip})")
                ok += 1
            else:
                ui.print_warn(f"  {name}: SSH аутентификация не удалась ({user}@{ip})")
                warn += 1

    # --- Syncthing ---
    ui.print_section("syncthing")
    if not machines:
        ui.print_info("  нет машин")
    else:
        from .syncthing import check_running as st_check

        for name, info in machines.items():
            host = info["host"]
            user = info.get("user", "mflkee")
            if nb is not None and nb.is_self(host):
                ip = nb.self_ip
            else:
                ip = resolve_host(host)
            if not ip or not check_port(ip):
                continue
            st_status = st_check(ip, user=user)
            if st_status.running:
                ui.print_ok(f"  {name}: работает (uptime {st_status.uptime}s)")
                ok += 1
            else:
                ui.print_warn(f"  {name}: не работает")
                warn += 1

    # --- Obsidian ---
    ui.print_section("obsidian REST API")
    if not machines:
        ui.print_info("  нет машин")
    else:
        from .obsidian import check_api as obs_check

        for name, info in machines.items():
            host = info["host"]
            user = info.get("user", "mflkee")
            if nb is not None and nb.is_self(host):
                ip = nb.self_ip
            else:
                ip = resolve_host(host)
            if not ip or not check_port(ip):
                continue
            status = obs_check(ip, user=user)
            if status.api_responsive:
                ui.print_ok(f"  {name}: OK (HTTP {status.http_code})")
                ok += 1
            elif status.service_active:
                ui.print_warn(f"  {name}: сервис активен, API не отвечает")
                warn += 1
            else:
                ui.print_info(f"  {name}: сервис не установлен")

    # --- Summary ---
    ui.print_section("итог")
    ui.print_ok(f"Пройдено: {ok}")
    if warn:
        ui.print_warn(f"Предупреждений: {warn}")
    if fail:
        ui.print_error(f"Ошибок: {fail}")

    return 0 if fail == 0 else 1


def _print_help():
    ui.print_header()
    ui.print_info("Децентрализованная синхронизация dotfiles через NetBird")
    ui._print()
    ui.print_section("команды")
    ui._print(
        ui._make_kv_table(
            [
                ["dsync sync", "полный цикл синхронизации: chezmoi → github → все машины"],
                ["dsync status", "статус NetBird, машин и chezmoi"],
                ["dsync push", "закоммитить и отправить dotfiles на все машины"],
                ["dsync pull", "получить dotfiles с GitHub и применить chezmoi"],
                ["dsync doctor", "полная диагностика инфраструктуры"],
                ["dsync setup", "настроить SSH-доступ ко всем машинам"],
                ["dsync add <имя> <host>", "добавить машину в конфиг"],
                ["dsync remove <имя>", "удалить машину из конфига"],
                ["dsync discover", "обновить FQDN/IP машин из NetBird"],
                ["dsync hub pull", "pull всех git-репозиториев (локально + удалённо)"],
                ["dsync hub status", "статус всех git-репозиториев"],
                ["dsync project sync", "синхронизировать проекты на всех машинах"],
                ["dsync project status", "статус проектов"],
                ["dsync project clone", "клонировать проекты на удалённые машины"],
                ["dsync syncthing status", "статус Syncthing на всех машинах"],
                ["dsync syncthing resolve", "авто-разрешение конфликтов Syncthing"],
                ["dsync zen export|import|info", "Zen Browser: экспорт/импорт профиля"],
                ["dsync tmux export|import", "tmux: экспорт/импорт сессий"],
                ["dsync timer --enable", "авто-синхронизация каждые 30 минут"],
            ]
        )
    )
    ui._print()
    ui.print_section("примеры")
    ui.print_info("dsync sync                        # полная синхронизация")
    ui.print_info("dsync sync --dry-run              # посмотреть что будет")
    ui.print_info("dsync sync --only notebook        # только одна машина")
    ui.print_info("dsync sync --jobs 8               # 8 параллельных SSH")
    ui.print_info("dsync doctor                      # диагностика всего")
    ui.print_info("dsync hub pull --local            # pull только локально")
    ui.print_info("dsync project sync metroLog       # синхронизировать один проект")


def main():
    config = Config.ensure_default()

    for err in config.validate():
        ui.print_warn(f"config: {err}")

    parser = argparse.ArgumentParser(
        prog="dsync", description="Decentralized chezmoi dotfiles sync via NetBird"
    )
    parser.add_argument(
        "--version", action="version", version="dsync 0.1.0"
    )
    parser.add_argument(
        "--debug", action="store_true", help="Включить DEBUG логирование"
    )
    sub = parser.add_subparsers(dest="command")

    sub.add_parser("status", help="Показать статус всех машин")
    sync_p = sub.add_parser("sync", help="Полный цикл синхронизации")
    sync_p.add_argument(
        "machines", nargs="*", help="Синхронизировать только эти машины"
    )
    sync_p.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="ИМЯ",
        help="Синхронизировать только указанную машину (можно несколько раз)",
    )
    sync_p.add_argument(
        "--strategy",
        choices=["ours", "theirs", "abort", "ask"],
        help="Стратегия разрешения конфликтов (по умолчанию ours)",
    )
    sync_p.add_argument("--dry-run", action="store_true", help="Режим сухого прогона")
    sync_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных задач"
    )
    sync_p.add_argument(
        "--no-self-update",
        action="store_true",
        help="Не обновлять dsync перед синхронизацией",
    )
    sync_p.add_argument(
        "--no-hub",
        action="store_true",
        help="Не выполнять hub pull репозиториев в конце",
    )

    push_p = sub.add_parser("push", help="Отправить изменения на удалённые машины")
    push_p.add_argument("machines", nargs="*", help="Отправить только на эти машины")
    push_p.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="ИМЯ",
        help="Отправить только на указанную машину (можно несколько раз)",
    )
    push_p.add_argument(
        "--strategy",
        choices=["ours", "theirs", "abort", "ask"],
        help="Стратегия разрешения конфликтов (по умолчанию ours)",
    )
    push_p.add_argument("--dry-run", action="store_true", help="Режим сухого прогона")
    push_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных задач"
    )

    pull_p = sub.add_parser("pull", help="Получить изменения и применить chezmoi")
    pull_p.add_argument(
        "--strategy",
        choices=["ours", "theirs", "abort", "ask"],
        help="Стратегия разрешения конфликтов (по умолчанию ours)",
    )
    pull_p.add_argument("--dry-run", action="store_true", help="Режим сухого прогона")
    sub.add_parser("setup", help="Настроить SSH доступ к машинам")

    add_p = sub.add_parser("add", help="Добавить машину в конфиг")
    add_p.add_argument("name", help="Короткое имя машины")
    add_p.add_argument("host", help="FQDN (user@host или host)")
    add_p.add_argument("--user", default="mflkee", help="SSH пользователь")

    rm_p = sub.add_parser("remove", help="Удалить машину из конфига")
    rm_p.add_argument("name", help="Имя машины")

    timer_p = sub.add_parser("timer", help="Управление systemd таймером")
    timer_p.add_argument("--enable", action="store_true", help="Включить таймер")
    timer_p.add_argument("--disable", action="store_true", help="Выключить таймер")
    timer_p.add_argument(
        "--mode",
        choices=["pull", "sync"],
        default="pull",
        help="Команда для запуска таймера (по умолчанию pull)",
    )

    sub.add_parser("discover", help="Обновить FQDN/IP машин из NetBird")

    # zen subcommand
    zen_p = sub.add_parser("zen", help="Zen Browser: экспорт/импорт профиля")
    zen_sub = zen_p.add_subparsers(dest="zen_action")
    zen_sub.add_parser("export", help="Экспорт профиля Zen в dotfiles")
    zen_sub.add_parser("import", help="Импорт профиля Zen из dotfiles")
    zen_sub.add_parser("info", help="Показать информацию о профиле Zen")

    # project subcommand
    project_p = sub.add_parser("project", help="Синхронизация проектов")
    project_sub = project_p.add_subparsers(dest="project_action", required=True)
    project_sub.add_parser("status", help="Статус проектов")
    project_sync_p = project_sub.add_parser("sync", help="Синхронизировать проекты")
    project_sync_p.add_argument(
        "names", nargs="*", help="Имена проектов (если не указаны — все)"
    )
    project_sync_p.add_argument(
        "--dry-run", action="store_true", help="Режим сухого прогона"
    )
    project_sync_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных задач"
    )
    project_clone_p = project_sub.add_parser(
        "clone", help="Клонировать проекты на удалённые машины"
    )
    project_clone_p.add_argument("names", nargs="*", help="Имена проектов")
    project_clone_p.add_argument(
        "--dry-run", action="store_true", help="Режим сухого прогона"
    )
    project_clone_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных задач"
    )

    # self subcommand
    self_p = sub.add_parser("self", help="Самообновление dsync")
    self_sub = self_p.add_subparsers(dest="self_action", required=True)
    self_sub.add_parser("status", help="Статус версии dsync")
    self_sub.add_parser("update", help="Обновить dsync до последней версии")

    # hub subcommand
    hub_p = sub.add_parser("hub", help="Массовый pull всех git-репозиториев")
    hub_sub = hub_p.add_subparsers(dest="hub_action", required=True)
    hub_status_p = hub_sub.add_parser("status", help="Статус всех репозиториев в hub")
    hub_status_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных задач"
    )
    hub_pull_p = hub_sub.add_parser(
        "pull", help="Pull всех репозиториев (ff-only, dirty пропускаются)"
    )
    hub_pull_p.add_argument(
        "--local", action="store_true", help="Только локально, без SSH на машины"
    )
    hub_pull_p.add_argument(
        "--dry-run", action="store_true", help="Режим сухого прогона"
    )
    hub_pull_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных задач"
    )

    # syncthing subcommand
    st_p = sub.add_parser("syncthing", help="Мониторинг и управление Syncthing")
    st_sub = st_p.add_subparsers(dest="st_action", required=True)
    st_status_p = st_sub.add_parser(
        "status", help="Проверить здоровье Syncthing на машинах"
    )
    st_status_p.add_argument(
        "machines",
        nargs="*",
        help="Машины (по умолчанию все online)",
    )
    st_resolve_p = st_sub.add_parser(
        "resolve", help="Автоматически разрешить конфликты Syncthing"
    )
    st_resolve_p.add_argument(
        "machines",
        nargs="*",
        help="Машины (по умолчанию все online)",
    )
    st_restart_p = st_sub.add_parser("restart", help="Перезапустить Syncthing")
    st_restart_p.add_argument(
        "machines",
        nargs="*",
        help="Машины (по умолчанию все online)",
    )

    # doctor subcommand
    doctor_p = sub.add_parser("doctor", help="Полная диагностика инфраструктуры")
    doctor_p.add_argument(
        "--jobs", "-j", type=int, default=4, help="Количество параллельных проверок"
    )

    # tmux subcommand (for remote sync script)
    tmux_p = sub.add_parser("tmux", help="Экспорт/импорт tmux сессий")
    tmux_sub = tmux_p.add_subparsers(dest="tmux_action")
    tmux_sub.add_parser("export", help="Экспорт tmux сессии в dotfiles")
    tmux_sub.add_parser("import", help="Импорт tmux сессии из dotfiles")

    # help subcommand — friendly help page
    sub.add_parser("help", help="Показать справку с примерами")

    args = parser.parse_args()

    log_level = "DEBUG" if getattr(args, "debug", False) else config.log_level
    setup_logging(str(config.log_file), log_level)

    if not args.command:
        _print_help()
        return 0

    if args.command == "help":
        _print_help()
        return 0

    if args.command == "status":
        return cmd_status(config)
    elif args.command in ("sync", "push", "pull"):
        from .lock import AlreadyRunningError, acquire_lock

        try:
            with acquire_lock():
                if args.command == "sync":
                    return cmd_sync(
                        config,
                        getattr(args, "strategy", ""),
                        self_update=not args.no_self_update,
                        run_hub=not args.no_hub,
                        only=list(args.machines) + list(args.only),
                        dry_run=args.dry_run,
                        jobs=args.jobs,
                    )
                elif args.command == "push":
                    return cmd_push(
                        config,
                        getattr(args, "strategy", ""),
                        only=list(args.machines) + list(args.only),
                        dry_run=args.dry_run,
                        jobs=args.jobs,
                    )
                else:  # pull
                    return cmd_pull(
                        config, getattr(args, "strategy", ""), dry_run=args.dry_run
                    )
        except AlreadyRunningError as e:
            ui.print_error(str(e))
            return 1
    elif args.command == "setup":
        return cmd_setup(config)
    elif args.command == "add":
        host = args.host
        user = args.user
        if "@" in host:
            user, host = host.split("@", 1)
        return cmd_add(config, args.name, host, user)
    elif args.command == "remove":
        return cmd_remove(config, args.name)
    elif args.command == "timer":
        return cmd_timer(config, args.enable, args.disable, mode=args.mode)
    elif args.command == "discover":
        return cmd_discover(config)
    elif args.command == "zen":
        if args.zen_action == "export":
            zen_dest = config.git_source / "dot_config" / "dsync" / "zen.json"
            ui.print_header()
            ui.print_section("Экспорт Zen Browser")
            result = export_zen(zen_dest)
            if result:
                ui.print_ok(f"Экспорт сохранён: {result}")
                ui.print_info("Запусти dsync sync, чтобы отправить на другие машины")
                return 0
            return 1
        elif args.zen_action == "import":
            zen_src = config.git_source / "dot_config" / "dsync" / "zen.json"
            ui.print_header()
            ui.print_section("Импорт Zen Browser")
            ui.print_warn("Закрой Zen Browser перед импортом!")
            if import_zen(zen_src):
                ui.print_ok("Импорт выполнен")
                return 0
            return 1
        elif args.zen_action == "info":
            profile = find_profile()
            if profile:
                ui.print_header()
                ui.print_section("Zen Browser профиль")
                ui.print_ok(f"Профиль: {profile}")
                session = profile / "zen-sessions.jsonlz4"
                if session.exists():
                    try:
                        data = json.loads(
                            lz4.block.decompress(session.read_bytes()[8:])
                        )
                        tabs = len(data.get("tabs", []))
                        spaces = len(data.get("spaces", []))
                        groups = len(data.get("groups", []))
                        ui.print_info(f"  Вкладок: {tabs}")
                        ui.print_info(f"  Рабочих пространств: {spaces}")
                        ui.print_info(f"  Групп: {groups}")
                        for s in data.get("spaces", []):
                            ui.print_info(f"    • {s.get('name', '?')}")
                    except Exception as e:
                        ui.print_warn(f"Не удалось прочитать сессию: {e}")
                return 0
            ui.print_error("Профиль не найден")
            return 1
    elif args.command == "project":
        if args.project_action == "status":
            return cmd_project_status(config)
        elif args.project_action == "sync":
            return cmd_project_sync(
                config, args.names, dry_run=args.dry_run, jobs=args.jobs
            )
        elif args.project_action == "clone":
            return cmd_project_clone(
                config, args.names, dry_run=args.dry_run, jobs=args.jobs
            )
    elif args.command == "self":
        if args.self_action == "status":
            return cmd_self_status()
        elif args.self_action == "update":
            return cmd_self_update()
    elif args.command == "hub":
        if args.hub_action == "status":
            return cmd_hub_status(config, jobs=args.jobs)
        elif args.hub_action == "pull":
            return cmd_hub_pull(
                config, local_only=args.local, dry_run=args.dry_run, jobs=args.jobs
            )
    elif args.command == "syncthing":
        return cmd_syncthing(config, args.st_action, args.machines)
    elif args.command == "doctor":
        return cmd_doctor(config, jobs=args.jobs)
    elif args.command == "tmux":
        repo = config.git_source
        if args.tmux_action == "export":
            tmux_dest = repo / "dot_config" / "dsync" / "tmux-session.txt"
            r = tmux_export(tmux_dest)
            if r.success:
                ui.print_ok("Tmux сессия экспортирована")
                return 0
            ui.print_error(f"Ошибка: {r.stderr}")
            return 1
        elif args.tmux_action == "import":
            tmux_src = repo / "dot_config" / "dsync" / "tmux-session.txt"
            r = tmux_import(tmux_src)
            if r.success:
                ui.print_ok("Tmux сессия импортирована")
                return 0
            ui.print_error(f"Ошибка: {r.stderr}")
            return 1
        return 1
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
