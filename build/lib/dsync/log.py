"""Logging setup for dsync."""

import logging
from pathlib import Path

DEFAULT_LOG_FILE = Path.home() / ".local" / "share" / "dsync" / "dsync.log"


def setup_logging(log_file: str | None = None, level: str = "INFO") -> Path:
    """Configure file logging.

    Returns the path to the log file.
    """
    path = Path(log_file) if log_file else DEFAULT_LOG_FILE
    path.parent.mkdir(parents=True, exist_ok=True)

    effective_level = getattr(logging, level.upper(), logging.INFO)
    logging.basicConfig(
        level=effective_level,
        format="%(asctime)s [%(levelname)s] %(message)s",
        handlers=[logging.FileHandler(path, mode="a")],
        force=True,
    )
    return path
