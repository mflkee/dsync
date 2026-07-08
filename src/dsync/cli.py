import argparse
import ipaddress
import json
import os
import re
import shlex
import socket
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
    get_remote_url,
    push,
)
from .chezmoi import (
    get_status as git_status,
)
from .config import Config
from .conflict import resolve_interactive, safe_pull
from .netbird import Peer, get_status
from .resolver import resolve_host
from .ssh_client import check_connectivity
from .ssh_client import run as ssh_run
from .zen import export_zen, find_profile, import_zen


def _check_port(ip: str, port: int = 22, timeout: float = 2) -> bool:
    try:
        with socket.create_connection((ip, port), timeout=timeout):
            return True
    except (socket.timeout, ConnectionRefusedError, OSError):
        return False


def _is_ip_value(value: str) -> bool:
    try:
        ipaddress.ip_address(value)
        return True
    except ValueError:
        return False


def _sync_git_repo(repo: Path, branch: str, strategy: str) -> bool:
    """Commit local changes, fetch, pull and push to origin.

    Returns True on success, False on failure.
    """
    gs = git_status(repo)
    if gs.error:
        ui.print_error(f"Ошибка git: {gs.error}")
        return False

    if not gs.is_clean:
        ui.print_section("📤 Локальные изменения")
        with ui.spinner_ctx("Коммит..."):
            msg = f"sync: {os.uname().nodename} {datetime.now().strftime('%Y-%m-%d %H:%M')}"
            r = commit(repo, msg)
        if r.success:
            ui.print_ok("Закоммичено")
        else:
            ui.print_error(f"Ошибка коммита: {r.stderr}")
            return False
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
                return False
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
            return False

    return True


def _remote_sync_script(repo_path: str, branch: str, remote_url: str) -> str:
    """Build the shell script run on remote machines via SSH."""
    return f"""export PATH="$HOME/.local/bin:$PATH"
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
  rm -rf {shlex.quote(repo_path)} && git clone {shlex.quote(remote_url)} {shlex.quote(repo_path)}
fi
if [ -z "$C" ]; then echo "NO_CHEZMOI"; exit 0; fi
cd {shlex.quote(repo_path)} && "$C" apply --no-sudo 2>/dev/null || "$C" apply 2>/dev/null || true"""


def _sync_remote_machine(
    repo: Path,
    branch: str,
    remote_url: str,
    nb,
    name: str,
    info: dict,
) -> tuple[str, str]:
    """Sync a single remote machine.

    Returns (status, message). Status is one of:
    success, skipped-offline, skipped-current, skipped-ssh, skipped-chezmoi,
    failed-conflict, failed-error.
    """
    host = info["host"]
    user = info.get("user", "mflkee")

    if host == nb.self_fqdn:
        return "skipped-current", "текущая машина"

    peer = next((p for p in nb.peers if p.fqdn == host), None)
    if peer and not peer.is_connected:
        return "skipped-offline", "офлайн"

    ip = resolve_host(host)
    if not _check_port(ip):
        return "skipped-ssh", "SSH порт 22 недоступен"

    rcmd = _remote_sync_script(str(repo), branch, remote_url)
    with ui.spinner_ctx(f"SSH: {host} ({ip})..."):
        r = ssh_run(ip, rcmd, user=user, timeout=300)

    if not r.success:
        return "failed-error", r.stderr.replace("\n", "; ")[:200]
    if "GIT_CONFLICT" in r.stdout:
        return "failed-conflict", "конфликт git, chezmoi не применялся"
    if "NO_CHEZMOI" in r.stdout:
        return "skipped-chezmoi", "chezmoi не установлен"
    return "success", ""


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
                    ui.print_warn("    ⚫ Не найден в NetBird")

    return 0


def cmd_sync(config: Config, strategy: str = ""):
    ui.print_header()
    repo = config.git_source
    branch = config.git_branch

    if not _sync_git_repo(repo, branch, strategy):
        return 1

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен — не могу синхронизировать удалённые машины")
        return 1

    machines = config.machines
    if not machines:
        ui.print_warn("Нет машин в конфиге для удалённой синхронизации")
        return 0

    remote_url = config.git_remote_url or get_remote_url(repo)
    if not remote_url:
        ui.print_error("Не удалось определить URL git remote. Укажи [git] remote_url в конфиге")
        return 1

    ui.print_section("🔄 Синхронизация с удалёнными машинами")
    success_count = 0
    fail_count = 0
    skip_count = 0

    for name, info in machines.items():
        ui.print_info(f"  → {name} ({info['host']})")
        status, reason = _sync_remote_machine(repo, branch, remote_url, nb, name, info)
        if status == "success":
            ui.print_ok(f"{name}: синхронизировано")
            success_count += 1
        elif status.startswith("skipped-"):
            ui.print_warn(f"{name}: {reason}, пропускаю")
            skip_count += 1
        else:
            ui.print_error(f"{name}: {reason}")
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

    repo = config.git_source
    branch = config.git_branch

    ui.print_header()

    if not _sync_git_repo(repo, branch, strategy):
        return 1

    remote_url = config.git_remote_url or get_remote_url(repo)
    if not remote_url:
        ui.print_error("Не удалось определить URL git remote. Укажи [git] remote_url в конфиге")
        return 1

    ui.print_section("🔄 Рассылка на машины")
    fail_count = 0

    for name, info in config.machines.items():
        ui.print_info(f"  → {name} ({info['host']})")
        status, reason = _sync_remote_machine(repo, branch, remote_url, nb, name, info)
        if status == "success":
            ui.print_ok(f"{name}: OK")
        elif status.startswith("skipped-"):
            ui.print_warn(f"{name}: {reason}")
        else:
            ui.print_error(f"{name}: {reason}")
            fail_count += 1

    return 0 if fail_count == 0 else 1


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


def cmd_discover(config: Config):
    ui.print_header()
    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    ui.print_section("🔍 Поиск машин в NetBird")
    peers_by_short: dict[str, Peer] = {}
    suffix_re = re.compile(r"-\d+-\d+$")
    aliases = config.discover_aliases
    prefixes = config.discover_prefixes

    def _register(key: str, peer):
        existing = peers_by_short.get(key)
        if existing is None:
            peers_by_short[key] = peer
        elif not existing.is_connected and peer.is_connected:
            peers_by_short[key] = peer

    for peer in nb.peers:
        if peer.fqdn == nb.self_fqdn:
            continue
        short = peer.hostname_short
        _register(short, peer)
        _register(short.replace("-", ""), peer)
        _register(suffix_re.sub("", short), peer)
        # Strip configured prefixes so short aliases can match
        for prefix in prefixes:
            if short.startswith(prefix):
                stripped = short[len(prefix):]
                _register(stripped, peer)
                _register(suffix_re.sub("", stripped), peer)

    machines = config.data.setdefault("machines", {})
    updated = []
    skipped = []

    for name, info in list(machines.items()):
        host = info.get("host", "")
        user = info.get("user", "mflkee")

        # already an IP
        if _is_ip_value(host):
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

        name = None
        normalized = suffix_re.sub("", short)
        # Try to match against aliases that ignore the NetBird -<digits> suffix
        for prefix, alias in aliases.items():
            if short.startswith(prefix) or normalized.startswith(prefix):
                name = alias
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

    sub.add_parser("discover", help="Обновить FQDN/IP машин из NetBird")

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
