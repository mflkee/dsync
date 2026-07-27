"""Logging setup for dsync."""

import logging
from pathlib import Path

DEFAULT_LOG_FILE = Path.home() / ".local" / "share" / "dsync" / "dsync.log"


def setup_logging(log_file: str | None = None, level: str = "INFO") -> Path:
    """Configure file + console logging.

    File gets the configured level (default INFO).
    Console gets WARNING+ to avoid spamming the terminal.
    Returns the path to the log file.
    """
    path = Path(log_file) if log_file else DEFAULT_LOG_FILE
    path.parent.mkdir(parents=True, exist_ok=True)

    effective_level = getattr(logging, level.upper(), logging.INFO)

    file_handler = logging.FileHandler(path, mode="a")
    file_handler.setLevel(effective_level)
    file_handler.setFormatter(
        logging.Formatter("%(asctime)s [%(levelname)s] %(name)s: %(message)s")
    )

    console_handler = logging.StreamHandler()
    console_handler.setLevel(logging.WARNING)
    console_handler.setFormatter(
        logging.Formatter("[%(levelname)s] %(name)s: %(message)s")
    )

    root = logging.getLogger()
    root.setLevel(effective_level)
    root.handlers.clear()
    root.addHandler(file_handler)
    root.addHandler(console_handler)

    logging.getLogger("dsync").info(
        "logging initialized: level=%s file=%s", level, path
    )
    return path
