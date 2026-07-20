"""CLI команды hub (git-репозитории) и self (самообновление dsync)."""

import concurrent.futures
from pathlib import Path

from . import hub, selfupdate, ui
from .netbird import get_status
from .resolver import resolve_host
from .ssh_client import run as ssh_run


def _check_port(ip: str, port: int = 22, timeout: float = 2) -> bool:
    import socket

    try:
        with socket.create_connection((ip, port), timeout=timeout):
            return True
    except (socket.timeout, ConnectionRefusedError, OSError):
        return False


def cmd_self_status() -> int:
    ui.print_header()
    ui.print_section("🔄 dsync self")
    st = selfupdate.get_self_status(do_fetch=True)
    if st.error:
        ui.print_error(st.error)
        return 1
    ui.print_info(f"Источник: {st.source}")
    ui.print_info(f"Ветка: {st.branch}  коммит: {st.current_sha} (origin: {st.remote_sha})")
    if not st.is_clean:
        ui.print_warn("В репозитории есть незакоммиченные изменения")
    if st.behind > 0:
        ui.print_warn(f"Отстаёт от origin на {st.behind} коммитов — выполни: dsync self update")
    else:
        ui.print_ok("Версия актуальна")
    return 0


def cmd_self_update() -> int:
    ui.print_header()
    ui.print_section("🔄 Обновление dsync")
    with ui.spinner_ctx("Проверка и обновление..."):
        r = selfupdate.self_update()
    if not r.success:
        ui.print_error(r.stderr)
        return 1
    if r.stdout == "up-to-date":
        ui.print_ok("Уже актуальная версия")
    else:
        ui.print_ok(f"Обновлено: {r.stdout}")
        ui.print_info("Новая версия вступит в силу со следующего запуска dsync")
    return 0


def auto_self_update() -> bool:
    """Тихое самообновление для вызова из sync. True — обновлено или не требуется."""
    st = selfupdate.get_self_status(do_fetch=True)
    if st.error:
        ui.print_warn(f"self-update: {st.error}")
        return True
    if st.behind == 0:
        ui.print_ok(f"dsync актуален ({st.current_sha})")
        return True
    if not st.is_clean:
        ui.print_warn(f"dsync отстаёт на {st.behind}, но репозиторий грязный — пропускаю")
        return True
    ui.print_info(f"dsync отстаёт на {st.behind} коммитов, обновляю...")
    with ui.spinner_ctx("Обновление dsync..."):
        r = selfupdate.self_update()
    if r.success and r.stdout != "up-to-date":
        ui.print_ok(f"dsync обновлён: {r.stdout}")
        ui.print_info("Изменения вступят в силу со следующего запуска")
    elif not r.success:
        ui.print_warn(f"self-update не удался: {r.stderr}")
    return True


def cmd_hub_status(config, jobs: int = 4) -> int:
    ui.print_header()
    root = config.hub_root
    ui.print_section(f"📦 Hub: {root}")
    with ui.spinner_ctx("Сканирование репозиториев..."):
        repos = hub.collect_status(root, jobs=jobs)
    if not repos:
        ui.print_warn(f"Git-репозитории не найдены в {root}")
        return 0
    rows = []
    for hr in repos:
        rows.append([hr.name, hr.branch or "—", hr.status_text])
    ui.print_panel("📦 Репозитории", ui._make_table(["Репо", "Ветка", "Статус"], rows))
    dirty = sum(1 for hr in repos if not hr.is_clean)
    behind = sum(1 for hr in repos if hr.behind > 0)
    if dirty or behind:
        ui.print_info(f"dirty: {dirty}, отстают: {behind} — обновить: dsync hub pull")
    else:
        ui.print_ok("Все репозитории актуальны")
    return 0


def _hub_pull_local(root: Path, jobs: int) -> list[list[str]]:
    results = hub.pull_all(root, jobs=jobs)
    rows = []
    for repo, r in results:
        if r.success:
            note = "обновлён" if "Already up to date" not in r.stdout else "актуален"
            rows.append([repo.name, ui.result_badge("success"), note])
        elif r.stderr == "dirty":
            rows.append([repo.name, ui.result_badge("skipped"), "dirty — пропущен"])
        elif r.stderr == "no remote":
            rows.append([repo.name, ui.result_badge("skipped"), "нет remote"])
        else:
            rows.append([repo.name, ui.result_badge("failed"), r.stderr.replace("\n", " ")[:80]])
    return rows


def _parse_hub_output(stdout: str) -> tuple[list[list[str]], str]:
    rows = []
    error = ""
    for line in stdout.splitlines():
        if line.startswith("HUB_ERROR|"):
            error = line.split("|", 1)[1]
        elif line.startswith("HUB|"):
            parts = line.split("|", 3)
            if len(parts) < 4:
                continue
            _, name, status, detail = parts
            if status == "updated":
                rows.append([name, ui.result_badge("success"), "обновлён"])
            elif status == "uptodate":
                rows.append([name, ui.result_badge("success"), "актуален"])
            elif status == "dirty":
                rows.append([name, ui.result_badge("skipped"), "dirty — пропущен"])
            elif status == "noremote":
                rows.append([name, ui.result_badge("skipped"), "нет remote"])
            else:
                rows.append([name, ui.result_badge("failed"), detail])
    return rows, error


def _hub_pull_machine(item: tuple[str, dict], nb, root: str, dry_run: bool) -> tuple[str, list[list[str]], str]:
    machine_name, machine_info = item
    host = machine_info["host"]
    user = machine_info.get("user", "mflkee")
    if host == nb.self_fqdn:
        return machine_name, [], "self"
    peer = next((p for p in nb.peers if p.fqdn == host), None)
    if peer and not peer.is_connected:
        return machine_name, [], "офлайн"
    ip = resolve_host(host)
    if not _check_port(ip):
        return machine_name, [], "SSH порт 22 недоступен"
    if dry_run:
        return machine_name, [], "dry-run"
    r = ssh_run(ip, hub.remote_hub_script(root), user=user, timeout=300)
    if not r.success:
        return machine_name, [], f"SSH: {r.stderr.replace(chr(10), '; ')[:120]}"
    rows, error = _parse_hub_output(r.stdout)
    if error:
        return machine_name, [], error
    return machine_name, rows, ""


def cmd_hub_pull(config, local_only: bool = False, dry_run: bool = False, jobs: int = 4) -> int:
    ui.print_header()
    if dry_run:
        ui.print_panel("🔍 Dry-run", "Изменения не применяются, только отчёт", style="yellow")

    root = config.hub_root
    ui.print_section(f"📦 Hub pull: {root} (локально)")
    if dry_run:
        repos = hub.discover_repos(root)
        ui.print_info(f"Найдено репозиториев: {len(repos)}")
        for repo in repos:
            ui.print_info(f"  {repo.name}")
    else:
        with ui.spinner_ctx("Pull локальных репозиториев..."):
            rows = _hub_pull_local(root, jobs)
        if rows:
            ui.print_result_table(rows)
        else:
            ui.print_warn(f"Git-репозитории не найдены в {root}")

    if local_only:
        return 0

    nb = get_status()
    if nb is None:
        ui.print_warn("NetBird недоступен — удалённые машины пропущены")
        return 0
    machines = config.machines
    if not machines:
        return 0

    ui.print_section("🌐 Hub pull: удалённые машины")
    root_str = str(root)
    items = list(machines.items())
    with ui.spinner_ctx("Pull на удалённых машинах..."):
        if jobs == 1:
            results = [_hub_pull_machine(it, nb, root_str, dry_run) for it in items]
        else:
            workers = max(1, min(jobs, len(items)))
            with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as ex:
                results = list(ex.map(lambda it: _hub_pull_machine(it, nb, root_str, dry_run), items))

    for machine_name, rows, note in results:
        if note == "self":
            continue
        if note:
            ui.print_info(f"  {machine_name}: {note}")
            continue
        ui.print_info(f"  {machine_name}:")
        if rows:
            ui.print_result_table(rows)
        else:
            ui.print_info("    репозитории не найдены")
    return 0
