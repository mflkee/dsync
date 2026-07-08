"""Project repository CLI commands for dsync."""

import concurrent.futures
import logging
from pathlib import Path

from . import projects, ui
from .netbird import get_status
from .resolver import resolve_host
from .ssh_client import run as ssh_run

logger = logging.getLogger(__name__)


def _filter_projects(projects_dict: dict, requested: list[str]) -> dict | None:
    if not requested:
        return projects_dict
    unknown = [name for name in requested if name not in projects_dict]
    if unknown:
        ui.print_error(f"Неизвестные проекты: {', '.join(unknown)}")
        return None
    return {name: info for name, info in projects_dict.items() if name in requested}


def _target_machines(project_info: dict, all_machines: dict) -> dict:
    wanted = project_info.get("machines")
    if not wanted:
        return all_machines
    return {name: info for name, info in all_machines.items() if name in wanted}


def _check_remote_project(machine_name: str, machine_info: dict, repo_path: str) -> tuple[str, str]:
    """Check if project exists on remote machine."""
    host = machine_info["host"]
    user = machine_info.get("user", "mflkee")
    ip = resolve_host(host)
    if not _check_port(ip):
        return "skipped", "SSH порт 22 недоступен"
    r = ssh_run(ip, f"test -d {Path(repo_path).as_posix()}/.git", user=user, timeout=10)
    if r.success:
        return "present", ""
    return "missing", ""


def _check_port(ip: str, port: int = 22, timeout: float = 2) -> bool:
    import socket

    try:
        with socket.create_connection((ip, port), timeout=timeout):
            return True
    except (socket.timeout, ConnectionRefusedError, OSError):
        return False


def _sync_project_remote(
    project_name: str,
    project_info: dict,
    nb,
    machine_name: str,
    machine_info: dict,
    dry_run: bool,
) -> tuple[str, str]:
    host = machine_info["host"]
    user = machine_info.get("user", "mflkee")
    repo_path = Path(project_info.get("path", "")).expanduser().as_posix()
    branch = project_info.get("branch", "main")
    remote_url = project_info.get("remote")

    if host == nb.self_fqdn:
        return "skipped", "текущая машина"

    peer = next((p for p in nb.peers if p.fqdn == host), None)
    if peer and not peer.is_connected:
        return "skipped", "офлайн"

    ip = resolve_host(host)
    if not _check_port(ip):
        return "skipped", "SSH порт 22 недоступен"

    if dry_run:
        return "success", "будет синхронизирован (dry-run)"

    script = projects.remote_sync_script(repo_path, branch, remote_url)
    with ui.spinner_ctx(f"SSH: {machine_name} ({host})..."):
        r = ssh_run(ip, script, user=user, timeout=300)

    if not r.success:
        logger.warning("Project %s sync to %s failed: %s", project_name, machine_name, r.stderr[:200])
        return "failed", r.stderr.replace("\n", "; ")[:200]
    if "GIT_CONFLICT" in r.stdout:
        return "failed", "конфликт git"
    if "NO_REMOTE" in r.stdout:
        return "failed", "не указан remote URL"
    return "success", ""


def _run_project_remote_sync(
    project_name: str,
    project_info: dict,
    machines: dict,
    nb,
    dry_run: bool,
    jobs: int,
) -> list[list[str]]:
    items = list(machines.items())

    def _sync_one(item: tuple[str, dict]) -> tuple[str, str, str]:
        machine_name, machine_info = item
        status, reason = _sync_project_remote(
            project_name, project_info, nb, machine_name, machine_info, dry_run
        )
        return machine_name, status, reason

    with ui.spinner_ctx("Синхронизация проекта..."):
        if jobs == 1:
            raw_results = [_sync_one(item) for item in items]
        else:
            workers = max(1, min(jobs, len(items)))
            with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as executor:
                raw_results = list(executor.map(_sync_one, items))

    rows: list[list[str]] = []
    for machine_name, status, reason in raw_results:
        if status == "success":
            rows.append([machine_name, ui.result_badge("success"), reason])
        elif status == "skipped":
            rows.append([machine_name, ui.result_badge("skipped"), reason])
        else:
            rows.append([machine_name, ui.result_badge("failed"), reason])
    return rows


def cmd_project_status(config) -> int:
    ui.print_header()
    projects_dict = config.projects
    if not projects_dict:
        ui.print_warn("Нет проектов в конфиге. Добавь секцию [projects]")
        return 0

    rows = []
    for name, info in projects_dict.items():
        ps = projects.get_project_status(name, info)
        rows.append([
            name,
            str(ps.path),
            ps.branch or "—",
            ps.sync_status,
        ])

    ui.print_panel(
        "📁 Проекты",
        ui._make_table(["Проект", "Путь", "Ветка", "Статус"], rows),
    )
    return 0


def cmd_project_sync(config, names: list[str], dry_run: bool = False, jobs: int = 4) -> int:
    ui.print_header()
    if dry_run:
        ui.print_panel("🔍 Dry-run", "Изменения не применяются, только отчёт", style="yellow")

    projects_dict = config.projects
    if not projects_dict:
        ui.print_warn("Нет проектов в конфиге")
        return 0

    selected = _filter_projects(projects_dict, names)
    if selected is None:
        return 1
    if not selected:
        return 0

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    all_machines = config.machines
    if not all_machines:
        ui.print_warn("Нет машин в конфиге")
        return 0

    for name, info in selected.items():
        ui.print_section(f"📁 {name}")
        path = Path(info.get("path", "")).expanduser()
        branch = info.get("branch", "main")
        remote = info.get("remote")

        if not path.exists():
            ui.print_error(f"Проект не найден локально: {path}")
            continue

        if dry_run:
            ui.print_info(f"Будет синхронизирован git: {path}")
        else:
            r = projects.sync_project_repo(path, branch, remote)
            if r.success:
                ui.print_ok("Локальный проект синхронизирован с GitHub")
            else:
                ui.print_error(f"Ошибка git: {r.stderr}")
                continue

        target_machines = _target_machines(info, all_machines)
        if not target_machines:
            ui.print_info("Нет целевых машин для проекта")
            continue

        ui.print_info(f"Рассылка на машины: {', '.join(target_machines)}")
        rows = _run_project_remote_sync(
            name, info, target_machines, nb, dry_run=dry_run, jobs=jobs
        )
        ui.print_result_table(rows)

    if dry_run:
        ui.print_info("Режим dry-run: изменения не применены")
    return 0


def cmd_project_clone(config, names: list[str], dry_run: bool = False, jobs: int = 4) -> int:
    ui.print_header()
    if dry_run:
        ui.print_panel("🔍 Dry-run", "Изменения не применяются, только отчёт", style="yellow")

    projects_dict = config.projects
    if not projects_dict:
        ui.print_warn("Нет проектов в конфиге")
        return 0

    selected = _filter_projects(projects_dict, names)
    if selected is None:
        return 1
    if not selected:
        return 0

    nb = get_status()
    if nb is None:
        ui.print_error("NetBird недоступен")
        return 1

    all_machines = config.machines
    if not all_machines:
        ui.print_warn("Нет машин в конфиге")
        return 0

    for name, info in selected.items():
        remote = info.get("remote")
        if not remote:
            ui.print_error(f"{name}: не указан remote URL")
            continue

        ui.print_section(f"📁 {name}")
        path = Path(info.get("path", "")).expanduser().as_posix()
        branch = info.get("branch", "main")
        target_machines = _target_machines(info, all_machines)

        items = list(target_machines.items())
        rows: list[list[str]] = []

        def _clone_one(item: tuple[str, dict]) -> tuple[str, str, str]:
            machine_name, machine_info = item
            host = machine_info["host"]
            user = machine_info.get("user", "mflkee")
            if host == nb.self_fqdn:
                return machine_name, "skipped", "текущая машина"
            peer = next((p for p in nb.peers if p.fqdn == host), None)
            if peer and not peer.is_connected:
                return machine_name, "skipped", "офлайн"
            ip = resolve_host(host)
            if not _check_port(ip):
                return machine_name, "skipped", "SSH порт 22 недоступен"
            if dry_run:
                return machine_name, "success", "будет клонирован (dry-run)"
            script = projects.remote_clone_script(path, remote, branch)
            with ui.spinner_ctx(f"SSH: {machine_name} ({host})..."):
                r = ssh_run(ip, script, user=user, timeout=300)
            if not r.success:
                return machine_name, "failed", r.stderr.replace("\n", "; ")[:200]
            if "ALREADY_CLONED" in r.stdout:
                return machine_name, "success", "уже клонирован"
            return machine_name, "success", "клонирован"

        with ui.spinner_ctx("Клонирование проекта..."):
            if jobs == 1:
                raw_results = [_clone_one(item) for item in items]
            else:
                workers = max(1, min(jobs, len(items)))
                with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as executor:
                    raw_results = list(executor.map(_clone_one, items))

        for machine_name, status, reason in raw_results:
            if status == "success":
                rows.append([machine_name, ui.result_badge("success"), reason])
            elif status == "skipped":
                rows.append([machine_name, ui.result_badge("skipped"), reason])
            else:
                rows.append([machine_name, ui.result_badge("failed"), reason])
        ui.print_result_table(rows)

    if dry_run:
        ui.print_info("Режим dry-run: изменения не применены")
    return 0
