from contextlib import contextmanager
from typing import Any

_console: Any = None

try:
    from rich import box as _box
    from rich.console import Console as _RichConsole
    from rich.markup import escape as rich_escape
    from rich.panel import Panel as _Panel
    from rich.progress import Progress, SpinnerColumn, TextColumn
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
        return green("●")
    elif s == "idle":
        return yellow("●")
    elif s == "disconnected":
        return red("●")
    return dim("○")


def print_header():
    if RICH:
        _console.print(_Panel.fit("[bold]dsync[/bold] — dotfiles sync", style="cyan", border_style="cyan"))
    else:
        _print(bold("╭──────────────────────────────────────╮"))
        _print(bold("│  dsync — dotfiles sync               │"))
        _print(bold("╰──────────────────────────────────────╯"))


def badge(text: str, color: str) -> str:
    if RICH:
        return f"[{color}]{text}[/{color}]"
    return text


def status_badge(status: str) -> str:
    s = status.lower()
    if s == "connected" or s == "online":
        return badge("● connected", "green")
    elif s == "idle":
        return badge("● idle", "yellow")
    elif s == "disconnected" or s == "offline":
        return badge("● offline", "red")
    return badge("○ " + status, "dim")


def result_badge(status: str, reason: str = "") -> str:
    if status == "success":
        label = badge("✓ OK", "green")
    elif status.startswith("skipped"):
        label = badge("⚠ skipped", "yellow")
    else:
        label = badge("✗ failed", "red")
    if reason:
        return f"{label} [dim]{reason}[/dim]" if RICH else f"{label} {reason}"
    return label


def print_panel(title: str, content=None, style: str = ""):
    if RICH:
        _console.print(_Panel(content if content is not None else "", title=title, border_style=style or "dim", expand=False))
    else:
        _print()
        _print(bold(title))
        _print(dim("─" * 50))
        if content is not None:
            _print(content)


def _print(*args, **kwargs):
    if RICH:
        _console.print(*args, **kwargs)
    else:
        print(*args, **kwargs)


def print_status_line(icon: str, text: str, status: str = ""):
    if status:
        _print(f"  {icon}  {text}  {status_dot(status)} {status}")
    else:
        _print(f"  {icon}  {text}")


def print_section(title: str):
    _print()
    _print(bold(title))
    _print(dim("─" * 50))


def print_error(msg: str):
    _print(f"  {red('✗')}  {msg}")


def print_ok(msg: str):
    _print(f"  {green('✓')}  {msg}")


def print_warn(msg: str):
    _print(f"  {yellow('⚠')}  {msg}")


def print_info(msg: str):
    _print(f"  {dim('ℹ')}  {msg}")


def print_result_table(rows: list[list[str]]):
    """Print a table of machine sync results."""
    if not rows:
        print_info("Нет машин для отображения")
        return
    if RICH:
        table = _RichTable(
            show_header=True,
            header_style="bold",
            box=_box.ROUNDED,
            border_style="dim",
        )
        table.add_column("Машина", style="cyan")
        table.add_column("Статус")
        table.add_column("Сообщение", style="dim")
        for row in rows:
            table.add_row(*[str(c) for c in row])
        _console.print(table)
    else:
        col_w = max(len(str(c)) for row in [["Машина", "Статус", "Сообщение"]] + rows for c in row)
        for row in rows:
            _print("  ".join(str(c).ljust(col_w) for c in row))


def print_table(columns, rows):
    _print(_make_table(columns, rows))


def _make_table(columns, rows) -> Any:
    if RICH:
        table = _RichTable(
            show_header=True,
            header_style="bold",
            box=_box.ROUNDED,
            border_style="dim",
        )
        for col in columns:
            table.add_column(col)
        for row in rows:
            table.add_row(*[str(c) for c in row])
        return table

    lines = []
    col_w = max(len(str(c)) for row in [[str(c) for c in columns]] + rows for c in row) if rows else max(len(str(c)) for c in columns)
    sep = "  "
    header = sep.join(str(c).ljust(col_w) for c in columns)
    lines.append(header)
    lines.append(dim("─" * len(header)))
    for row in rows:
        lines.append(sep.join(str(c).ljust(col_w) for c in row))
    return "\n".join(lines)


def _make_kv_table(rows: list[list[str]]) -> Any:
    """Build a key-value table."""
    if RICH:
        table = _RichTable(
            show_header=False,
            box=_box.ROUNDED,
            border_style="dim",
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
        progress = Progress(SpinnerColumn(), TextColumn("{task.description}"), transient=True)
        with progress:
            task = progress.add_task(message, total=None)
            yield
            progress.remove_task(task)
    else:
        print(f"  {dim('⟳')}  {message}", end="", flush=True)
        yield
        print("\r" + " " * (len(message) + 6) + "\r", end="", flush=True)
