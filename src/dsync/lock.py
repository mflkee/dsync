"""File lock to prevent concurrent dsync runs."""

import fcntl
import os
from contextlib import contextmanager
from pathlib import Path

LOCK_FILE = Path.home() / ".local" / "share" / "dsync" / "dsync.lock"


class AlreadyRunningError(Exception):
    """Raised when another dsync process holds the lock."""


@contextmanager
def acquire_lock(timeout: float = 0):
    """Acquire an exclusive non-blocking lock.

    Yields the lock file descriptor. Raises AlreadyRunningError if locked.
    """
    LOCK_FILE.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(LOCK_FILE, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        os.write(fd, str(os.getpid()).encode())
        yield fd
    except BlockingIOError:
        raise AlreadyRunningError(
            "dsync уже запущен (другой процесс держит lock). Подожди или удали "
            f"{LOCK_FILE} если процесс завис."
        ) from None
    finally:
        os.close(fd)
