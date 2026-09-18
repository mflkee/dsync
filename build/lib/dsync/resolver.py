import socket

from .netbird import get_status

# Cache the netbird status to avoid repeated calls in the same sync
_nb_cache = None


def _get_nb():
    global _nb_cache
    if _nb_cache is None:
        _nb_cache = get_status()
    return _nb_cache


def resolve_host(host: str) -> str:
    """Resolve hostname to IP.
    For .netbird.cloud domains: use NetBird status (skip slow DNS).
    For others: try DNS, fall back to host as-is.
    """
    if host.endswith(".netbird.cloud"):
        nb = _get_nb()
        if nb:
            if host == nb.self_fqdn:
                return nb.self_ip
            for peer in nb.peers:
                if peer.fqdn == host:
                    return peer.netbird_ip
        return host

    try:
        return socket.gethostbyname(host)
    except socket.gaierror:
        return host


def clear_cache():
    global _nb_cache
    _nb_cache = None
