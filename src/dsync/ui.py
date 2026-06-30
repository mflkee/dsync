import sys
from contextlib import contextmanager

try:
    from rich.console import Console as _RichConsole
    from rich.table import Table as _RichTable
    from rich.progress import Progress, SpinnerColumn, TextColumn
    from rich import box as _box
    from rich.markup import escape as rich_escape

    RICH = True
    _console = _RichConsole()
except ImportError:
    RICH = False

    class _RichConsole:
        pass

    _console = _RichConsole()


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
    _print(bold("╭──────────────────────────────────────╮"))
    _print(bold("│  dsync — dotfiles sync               │"))
    _print(bold("╰──────────────────────────────────────╯"))


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


def print_table(columns, rows):
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
        _console.print(table)
    else:
        col_w = max(len(c) for c in columns)
        sep = "  "
        header = sep.join(c.ljust(col_w) for c in columns)
        _print(header)
        _print(dim("─" * len(header)))
        for row in rows:
            _print(sep.join(str(c).ljust(col_w) for c in row))


@contextmanager
def spinner_ctx(message: str = "Working..."):
    if RICH:
        progress = Progress(SpinnerColumn(), TextColumn(f"{{task.description}}"), transient=True)
        with progress:
            task = progress.add_task(message, total=None)
            yield
            progress.remove_task(task)
    else:
        print(f"  {dim('⟳')}  {message}", end="", flush=True)
        yield
        print("\r", end="", flush=True)
