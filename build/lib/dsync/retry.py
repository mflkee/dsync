"""Retry helpers for network operations."""

import logging
import time
from collections.abc import Callable
from typing import TypeVar

logger = logging.getLogger(__name__)

T = TypeVar("T")


def with_retry(
    fn: Callable[[], T],
    is_retryable: Callable[[T], bool],
    attempts: int = 3,
    base_delay: float = 1.0,
) -> T:
    """Run fn(), retrying up to `attempts` times when is_retryable(result) is True.

    Uses exponential backoff: base_delay * (2 ** attempt_number).
    Returns the first non-retryable result.
    """
    result = fn()
    for attempt in range(1, attempts):
        if not is_retryable(result):
            return result
        delay = base_delay * (2 ** (attempt - 1))
        logger.info("Retry %d/%d after %.1fs", attempt, attempts - 1, delay)
        time.sleep(delay)
        result = fn()
    return result
