from contextlib import contextmanager
from typing import Any

_console: Any = None

try:
    from rich import box as _box
    from rich.console import Console as _RichConsole
    from rich.markup import escape as rich_escape
    from rich.table import Table as _RichTable

    RICH = True
    _console = _RichConsole()
except ImportError:
    RICH = False

    class _FallbackConsole:
        pass

    _console = _FallbackConsole()


def escape(text: str) -> str:
    if RICH:
        return rich_escape(text)
    return text


def green(text: str) -> str:
    if RICH:
        return f"[green]{escape(text)}[/green]"
    return f"\033[32m{escape(text)}\033[0m"


def red(text: str) -> str:
    if RICH:
        return f"[red]{escape(text)}[/red]"
    return f"\033[31m{escape(text)}\033[0m"


def yellow(text: str) -> str:
    if RICH:
        return f"[yellow]{escape(text)}[/yellow]"
    return f"\033[33m{escape(text)}\033[0m"


def dim(text: str) -> str:
    if RICH:
        return f"[dim]{escape(text)}[/dim]"
    return f"\033[2m{escape(text)}\033[0m"


def bold(text: str) -> str:
    if RICH:
        return f"[bold]{escape(text)}[/bold]"
    return f"\033[1m{escape(text)}\033[0m"


def status_dot(status: str) -> str:
    s = status.lower()
    if s == "connected":
        return green("ok")
    elif s == "idle":
        return yellow("idle")
    elif s == "disconnected":
        return red("down")
    return dim("--")


def print_header():
    _print(bold("dsync"))


def print_section(title: str):
    _print()
    _print(bold(title))


def print_ok(msg: str):
    _print(f"  {green('ok')}  {msg}")


def print_error(msg: str):
    _print(f"  {red('fail')}  {msg}")


def print_warn(msg: str):
    _print(f"  {yellow('warn')}  {msg}")


def print_info(msg: str):
    _print(f"  {dim('..')}  {msg}")


def print_status_line(icon: str, text: str, status: str = ""):
    if status:
        _print(f"  {icon}  {text}  {status_dot(status)} {status}")
    else:
        _print(f"  {icon}  {text}")


def print_panel(title: str, content=None, style: str = ""):
    _print()
    _print(bold(title))
    if content is not None:
        _print(content)


def result_badge(status: str, reason: str = "") -> str:
    if status == "success":
        label = green("ok")
    elif status.startswith("skipped"):
        label = yellow("skip")
    else:
        label = red("fail")
    if reason:
        return f"{label}  {dim(reason)}"
    return label


def status_badge(status: str) -> str:
    s = status.lower()
    if s in ("connected", "online"):
        return green("ok") + " " + status
    elif s == "idle":
        return yellow("idle")
    elif s in ("disconnected", "offline"):
        return red("down")
    return dim(status)


def badge(text: str, color: str) -> str:
    if RICH:
        return f"[{color}]{text}[/{color}]"
    return text


def _print(*args, **kwargs):
    if RICH:
        _console.print(*args, **kwargs)
    else:
        print(*args, **kwargs)


def print_result_table(rows: list[list[str]]):
    if not rows:
        print_info("net")
        return
    if RICH:
        table = _RichTable(
            show_header=True,
            header_style="bold",
            box=_box.SIMPLE,
            padding=(0, 2),
        )
        table.add_column("name", style="cyan")
        table.add_column("status")
        table.add_column("note", style="dim")
        for row in rows:
            table.add_row(*[str(c) for c in row])
        _console.print(table)
    else:
        col_w = max(
            len(str(c)) for row in [["name", "status", "note"]] + rows for c in row
        )
        sep = "  "
        header = sep.join(str(c).ljust(col_w) for c in ["name", "status", "note"])
        _print(header)
        _print(dim("-" * len(header)))
        for row in rows:
            _print(sep.join(str(c).ljust(col_w) for c in row))


def print_table(columns, rows):
    _print(_make_table(columns, rows))


def _make_table(columns, rows) -> Any:
    if RICH:
        table = _RichTable(
            show_header=True,
            header_style="bold",
            box=_box.SIMPLE,
            padding=(0, 2),
        )
        for col in columns:
            table.add_column(str(col))
        for row in rows:
            table.add_row(*[str(c) for c in row])
        return table

    lines = []
    col_w = (
        max(len(str(c)) for row in [[str(c) for c in columns]] + rows for c in row)
        if rows
        else max(len(str(c)) for c in columns)
    )
    sep = "  "
    header = sep.join(str(c).ljust(col_w) for c in columns)
    lines.append(header)
    lines.append(dim("-" * len(header)))
    for row in rows:
        lines.append(sep.join(str(c).ljust(col_w) for c in row))
    return "\n".join(lines)


def _make_kv_table(rows: list[list[str]]) -> Any:
    if RICH:
        table = _RichTable(
            show_header=False,
            box=_box.SIMPLE,
            padding=(0, 1),
        )
        table.add_column(style="bold")
        table.add_column()
        for row in rows:
            table.add_row(*[str(c) for c in row])
        return table

    lines = []
    key_w = max(len(str(row[0])) for row in rows) if rows else 0
    for row in rows:
        key, val = str(row[0]), str(row[1])
        lines.append(f"{key.rjust(key_w)}  {val}")
    return "\n".join(lines)


@contextmanager
def spinner_ctx(message: str = "Working..."):
    if RICH:
        from rich.progress import Progress, SpinnerColumn, TextColumn

        progress = Progress(
            SpinnerColumn(), TextColumn("{task.description}"), transient=True
        )
        with progress:
            task = progress.add_task(dim(message), total=None)
            yield
            progress.remove_task(task)
    else:
        print(f"  {dim(message)}", end="", flush=True)
        yield
        print("\r" + " " * (len(message) + 4) + "\r", end="", flush=True)


def print_dry_run_header():
    _print()
    _print(bold("dry-run") + "  " + dim("изменения не применяются"))
    _print()


def print_machine_result(name: str, status: str, note: str = "", dry_run: bool = False):
    """Print a per-machine sync result line."""
    if dry_run:
        if status == "success":
            _print(f"  {green('~')}  {name}: {dim('будет синхронизировано')}")
        elif status == "skipped":
            _print(f"  {dim('-')}  {name}: {dim('пропущен')}  {dim(note)}")
        else:
            _print(f"  {yellow('~')}  {name}: {dim('ошибка')}  {dim(note)}")
    else:
        if status == "success":
            print_ok(f"{name}: синхронизировано")
        elif status == "skipped":
            print_info(f"{name}: офлайн, пропускаю")
        else:
            print_error(f"{name}: {note}")


def print_error_summary(errors: list[tuple[str, str]]):
    """Print grouped error summary at the end."""
    if not errors:
        return
    _print()
    _print(bold("ошибки"))
    for name, msg in errors:
        _print(f"  {red('!')}  {bold(name)}: {msg}")


def print_progress_bar(current: int, total: int, name: str):
    """Print inline progress like [2/5] machine-name."""
    bar = f"[{current}/{total}]"
    _print(f"  {dim(bar)}  {bold(name)}")
