import argparse
import subprocess
import os
import shlex
import socket
import sys
import json
from pathlib import Path
from datetime import datetime

from . import ui
from .config import Config
from .netbird import get_status
from .ssh_client import run as ssh_run, check_connectivity
from .chezmoi import get_status as git_status, commit, fetch, pull, push, chezmoi_apply, diverts_check
from .resolver import resolve_host
from .conflict import (
    safe_pull, has_conflict, is_rebasing, rebase_abort,
    get_conflicted_files, show_diff_summary, resolve_interactive
)
from .zen import export_zen, import_zen, find_profile
import lz4.block


def _check_port(ip: str, port: int = 22, timeout: float = 2) -> bool:
    try:
        with socket.create_connection((ip, port), timeout=timeout):
            return True
    except (socket.timeout, ConnectionRefusedError, OSError):
        return False


def resolve_conflict(repo: Path, branch: str, strategy: str) -> bool | None:
    """Resolve git conflict.
    Returns True (keep local), False (keep remote), None (abort).
    """
    from .chezmoi import _git

    if strategy == "theirs":
        r = _git(repo, ["pull", "-X", "theirs", "origin", branch], timeout=60)
        if r.success:
            ui.print_ok("Оставлена удалённая версия (theirs)")
            return False
        ui.print_error(f"Ошибка: {r.stderr[:200]}")
        return None

    if strategy == "ours":
        r = _git(repo, ["fetch", "origin"])
        if r.success:
            r = _git(repo, ["pull", "-X", "ours", "origin", branch], timeout=60)
        if r.success:
            ui.print_ok("Оставлена локальная версия (ours)")
            return True
        ui.print_error(f"Ошибка: {r.stderr[:200]}")
        return None

    if strategy == "abort":
        ui.print_warn("Синхронизация отменена из-за конфликта")
        return None

    return resolve_interactive(repo, branch)


def cmd_status(config: Config):
    ui.print_header()

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    ui.print_status_line("🖥", ui.bold(nb.self_hostname_short), nb.daemon_status)
    ui.print_info(f"IP: {nb.self_ip}  FQDN: {nb.self_fqdn}")

    gs = git_status(config.git_source)
    ui.print_section("📁 Состояние chezmoi")
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

    ui.print_section("🌐 Машины в NetBird")
    table_cols = ["Имя", "Статус", "FQDN", "IP", "Лат.", "Тип"]
    table_rows = []
    for peer in nb.peers:
        name = peer.hostname_short
        s = ui.status_dot(peer.status)
        table_rows.append([
            name,
            f"{s} {peer.status}",
            peer.fqdn,
            peer.netbird_ip,
            peer.latency_ms,
            peer.connection_type,
        ])
    ui.print_table(table_cols, table_rows)

    ui.print_section("⚙ Целевые машины (из конфига)")
    machines = config.machines
    if not machines:
        ui.print_warn("Машины не настроены. Используй: dsync add <имя> <host>")
    else:
        online = []
        offline = []
        for name, info in machines.items():
            host = info["host"]
            ui.print_info(f"  {name} → {host}")
            if host == nb.self_fqdn:
                online.append(name)
                ui.print_ok(f"    🟢 Connected (текущая машина)  IP: {nb.self_ip}")
            else:
                peer = next((p for p in nb.peers if p.fqdn == host), None)
                if peer:
                    if peer.is_connected:
                        online.append(name)
                        ui.print_ok(f"    🟢 {peer.status}  IP: {peer.netbird_ip}")
                    else:
                        offline.append(name)
                        ui.print_warn(f"    🔴 {peer.status}")
                else:
                    offline.append(name)
                    ui.print_warn(f"    ⚫ Не найден в NetBird")

    return 0


def cmd_sync(config: Config, strategy: str = ""):
    ui.print_header()
    hostname = os.uname().nodename
    repo = config.git_source
    branch = config.git_branch

    gs = git_status(repo)
    if gs.error:
        ui.print_error(f"Ошибка git: {gs.error}")
        return 1

    if not gs.is_clean:
        ui.print_section("📤 Локальные изменения")
        with ui.spinner_ctx("Коммит..."):
            msg = f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
            r = commit(repo, msg)
        if r.success:
            ui.print_ok("Закоммичено")
        else:
            ui.print_error(f"Ошибка коммита: {r.stderr}")
            return 1
    else:
        ui.print_ok("Локально чисто")

    ui.print_section("📥 Синхронизация с GitHub")
    with ui.spinner_ctx("Fetch..."):
        fetch(repo)

    ahead, behind = diverts_check(repo, branch)
    if behind > 0:
        ui.print_info(f"Удалённых коммитов: {behind}")
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
        else:
            ui.print_warn(f"Pull: {r.stderr[:200]}")
    else:
        ui.print_ok("Pull не требуется")

    if ahead > 0 or gs.ahead > 0:
        with ui.spinner_ctx("Push..."):
            r = push(repo, branch)
        if r.success:
            ui.print_ok("Запущено на GitHub")
        else:
            ui.print_error(f"Ошибка push: {r.stderr}")
            return 1

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен — не могу синхронизировать удалённые машины")
        return 1

    machines = config.machines
    if not machines:
        ui.print_warn("Нет машин в конфиге для удалённой синхронизации")
        return 0

    ui.print_section("🔄 Синхронизация с удалёнными машинами")
    success_count = 0
    fail_count = 0
    skip_count = 0

    for name, info in machines.items():
        host = info["host"]
        user = info.get("user", "mflkee")

        if host == nb.self_fqdn:
            continue

        peer = next((p for p in nb.peers if p.fqdn == host), None)
        if peer and not peer.is_connected:
            ui.print_warn(f"  {name} ({host}) — офлайн, пропускаю")
            skip_count += 1
            continue
        if not peer:
            ui.print_warn(f"  {name} ({host}) — не найден в NetBird, пробую подключиться...")

        ui.print_info(f"  → {name} ({host})")

        ip = resolve_host(host)
        if not _check_port(ip):
            ui.print_warn(f"    SSH порт 22 недоступен, пропускаю")
            skip_count += 1
            continue

        repo_path = str(config.git_source)
        rcmd = f"""export PATH="$HOME/.local/bin:$PATH"
C="$(command -v chezmoi)"
if [ -d {shlex.quote(repo_path)}/.git ]; then
  cd {shlex.quote(repo_path)}
  git fetch origin {shlex.quote(branch)} 2>&1 || true
  git reset HEAD . 2>/dev/null
  git checkout -- . 2>/dev/null
  git clean -fd 2>/dev/null
  if ! git pull --rebase origin {shlex.quote(branch)} 2>&1; then
    git rebase --abort 2>/dev/null
    echo "GIT_CONFLICT"
    exit 0
  fi
else
  rm -rf {shlex.quote(repo_path)} && git clone https://github.com/mflkee/dotfiles.git {shlex.quote(repo_path)}
fi
if [ -z "$C" ]; then echo "NO_CHEZMOI"; exit 0; fi
cd {shlex.quote(repo_path)} && "$C" apply --no-sudo 2>/dev/null || "$C" apply 2>/dev/null || true"""
        with ui.spinner_ctx(f"SSH: {host} ({ip})..."):
            r = ssh_run(ip, rcmd, user=user, timeout=300)

        if r.success:
            if "GIT_CONFLICT" in r.stdout:
                ui.print_warn(f"{name}: конфликт git, chezmoi не применялся")
                fail_count += 1
            elif "NO_CHEZMOI" in r.stdout:
                ui.print_warn(f"{name}: chezmoi не установлен, пропускаю")
                skip_count += 1
            else:
                ui.print_ok(f"{name}: синхронизировано")
                success_count += 1
        else:
            err = r.stderr.replace("\n", "; ")
            ui.print_error(f"{name}: {err[:200]}")
            fail_count += 1

    ui.print_section("📊 Итог")
    if success_count:
        ui.print_ok(f"Успешно: {success_count}")
    if skip_count:
        ui.print_info(f"Пропущено (офлайн): {skip_count}")
    if fail_count:
        ui.print_error(f"Ошибок: {fail_count}")

    return 0 if fail_count == 0 else 1


def cmd_push(config: Config, strategy: str = ""):
    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    if not config.machines:
        ui.print_error("Нет машин в конфиге")
        ui.print_info("Добавь: dsync add <имя> <host>")
        return 1

    hostname = os.uname().nodename
    repo = config.git_source
    branch = config.git_branch

    ui.print_header()

    gs = git_status(repo)
    if gs.error:
        ui.print_error(f"Git: {gs.error}")
        return 1

    if not gs.is_clean:
        with ui.spinner_ctx("Коммит..."):
            msg = f"sync: {hostname} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
            r = commit(repo, msg)
        if r.success:
            ui.print_ok("Закоммичено")
        else:
            ui.print_error(f"Ошибка: {r.stderr}")
            return 1
    else:
        ui.print_ok("Изменений нет")

    ui.print_section("📥 Синхронизация с GitHub")
    with ui.spinner_ctx("Fetch..."):
        fetch(repo)

    ahead, behind = diverts_check(repo, branch)
    if behind > 0:
        ui.print_info(f"Удалённых коммитов: {behind}")
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
    else:
        ui.print_ok("Pull не требуется")

    if ahead > 0 or gs.ahead > 0:
        with ui.spinner_ctx("Push..."):
            r = push(repo, branch)
        if r.success:
            ui.print_ok("Запущено на GitHub")
        else:
            ui.print_error(f"Ошибка push: {r.stderr}")
            return 1

    ui.print_section("🔄 Рассылка на машины")
    for name, info in config.machines.items():
        host = info["host"]
        user = info.get("user", "mflkee")

        if host == nb.self_fqdn:
            continue

        peer = next((p for p in nb.peers if p.fqdn == host), None)
        if peer and not peer.is_connected:
            ui.print_warn(f"  {name} — офлайн")
            continue
        ui.print_info(f"  → {name}")
        ip = resolve_host(host)
        if not _check_port(ip):
            ui.print_warn(f"    SSH порт 22 недоступен, пропускаю")
            continue
        repo_path = str(repo)
        rcmd = f"""export PATH="$HOME/.local/bin:$PATH"
C="$(command -v chezmoi)"
if [ -d {shlex.quote(repo_path)}/.git ]; then
  cd {shlex.quote(repo_path)}
  git fetch origin {shlex.quote(branch)} 2>&1 || true
  git reset HEAD . 2>/dev/null
  git checkout -- . 2>/dev/null
  git clean -fd 2>/dev/null
  if ! git pull --rebase origin {shlex.quote(branch)} 2>&1; then
    git rebase --abort 2>/dev/null
    echo "GIT_CONFLICT"
    exit 0
  fi
else
  rm -rf {shlex.quote(repo_path)} && git clone https://github.com/mflkee/dotfiles.git {shlex.quote(repo_path)}
fi
if [ -z "$C" ]; then echo "NO_CHEZMOI"; exit 0; fi
cd {shlex.quote(repo_path)} && "$C" apply --no-sudo 2>/dev/null || "$C" apply 2>/dev/null || true"""
        with ui.spinner_ctx(f"SSH: {host} ({ip})..."):
            r = ssh_run(ip, rcmd, user=user, timeout=300)
        if r.success:
            if "GIT_CONFLICT" in r.stdout:
                ui.print_warn(f"{name}: конфликт git, chezmoi не применялся")
            elif "NO_CHEZMOI" in r.stdout:
                ui.print_warn(f"{name}: chezmoi не установлен, пропускаю")
            else:
                ui.print_ok(f"{name}: OK")
        else:
            ui.print_error(f"{name}: {r.stderr[:150]}")

    return 0


def cmd_pull(config: Config, strategy: str = ""):
    ui.print_header()
    repo = config.git_source
    branch = config.git_branch

    with ui.spinner_ctx("Fetch..."):
        fetch(repo)

    ahead, behind = diverts_check(repo, branch)
    if behind > 0:
        ui.print_info(f"Удалённых коммитов: {behind}")
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
    else:
        ui.print_ok("Уже актуально")

    with ui.spinner_ctx("chezmoi apply..."):
        r = chezmoi_apply()
    if r.success:
        ui.print_ok("chezmoi apply — OK")
    else:
        ui.print_warn(f"chezmoi apply: {r.stderr[:200]}" if r.stderr else "chezmoi apply: предупреждения")

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
        subprocess.run(["ssh-keygen", "-t", "ed25519", "-f", identity, "-N", ""], check=True)
        ui.print_ok("SSH-ключ создан")

    all_ok = True
    for name, info in machines.items():
        host = info["host"]
        user = info.get("user", "mflkee")
        ip = resolve_host(host)
        ui.print_info(f"\n  {name} ({user}@{host} → {ip})")

        if host == nb.self_fqdn or ip == nb.self_ip:
            ui.print_ok("Текущая машина, пропускаю")
            continue

        if not _check_port(ip):
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
                capture_output=True, text=True, timeout=30,
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

    ui.print_section("🌐 DNS / hosts")

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
                ui.print_info(f"  Добавить в /etc/hosts: sudo sh -c 'echo \"{entry}\" >> /etc/hosts'")

    if all_ok:
        ui.print_section("✅ SSH настроен")
        ui.print_info("Теперь можно запустить: dsync sync")
    else:
        ui.print_section("⚠ Некоторые машины не настроены")
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


def cmd_timer(config: Config, enable: bool, disable: bool):
    unit_dir = Path.home() / ".config" / "systemd" / "user"
    unit_dir.mkdir(parents=True, exist_ok=True)

    service = unit_dir / "dsync.service"
    timer = unit_dir / "dsync.timer"

    if disable:
        subprocess.run(["systemctl", "--user", "disable", "--now", "dsync.timer"],
                       capture_output=True)
        if service.exists():
            service.unlink()
        if timer.exists():
            timer.unlink()
        ui.print_ok("Таймер отключён")
        return 0

    dsync_bin = "dsync"
    service_content = f"""[Unit]
Description=dsync — chezmoi dotfiles sync

[Service]
Type=oneshot
ExecStart={dsync_bin} sync
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
    subprocess.run(["systemctl", "--user", "enable", "--now", "dsync.timer"],
                   capture_output=True)
    ui.print_ok("Таймер включён (каждые 30 минут)")
    ui.print_info("Проверить: systemctl --user status dsync.timer")
    return 0


def main():
    config = Config.ensure_default()

    parser = argparse.ArgumentParser(prog="dsync", description="Decentralized chezmoi dotfiles sync via NetBird")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("status", help="Показать статус всех машин")

    sync_p = sub.add_parser("sync", help="Полный цикл синхронизации")
    sync_p.add_argument("--strategy", choices=["ours", "theirs", "abort"],
                        help="Стратегия разрешения конфликтов (иначе — интерактивный режим)")

    push_p = sub.add_parser("push", help="Отправить изменения на удалённые машины")
    push_p.add_argument("--strategy", choices=["ours", "theirs", "abort"],
                        help="Стратегия разрешения конфликтов")

    pull_p = sub.add_parser("pull", help="Получить изменения и применить chezmoi")
    pull_p.add_argument("--strategy", choices=["ours", "theirs", "abort"],
                        help="Стратегия разрешения конфликтов")
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

    # zen subcommand
    zen_p = sub.add_parser("zen", help="Zen Browser: экспорт/импорт профиля")
    zen_sub = zen_p.add_subparsers(dest="zen_action")
    zen_sub.add_parser("export", help="Экспорт профиля Zen в dotfiles")
    zen_sub.add_parser("import", help="Импорт профиля Zen из dotfiles")
    zen_sub.add_parser("info", help="Показать информацию о профиле Zen")

    args = parser.parse_args()

    if args.command == "status":
        return cmd_status(config)
    elif args.command == "sync":
        return cmd_sync(config, getattr(args, "strategy", ""))
    elif args.command == "push":
        return cmd_push(config, getattr(args, "strategy", ""))
    elif args.command == "pull":
        return cmd_pull(config, getattr(args, "strategy", ""))
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
        return cmd_timer(config, args.enable, args.disable)
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
                        data = json.loads(lz4.block.decompress(session.read_bytes()[8:]))
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
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
