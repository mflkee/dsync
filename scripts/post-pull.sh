#!/usr/bin/env bash
# post_pull для [projects.dsync]: пересобрать клиентский бинарь и, если на
# этой машине крутится хаб, аккуратно его перезапустить.
#
# Почему так:
# * клиенты обновляются автоматически (этот скрипт в post_pull), а хаб раньше
#   обновлялся только руками — за 6 недель он отстал на несколько коммитов, и
#   никто этого не видел (в протоколе не было версии);
# * перезапуск хаба убивает SSH-сессию, внутри которой мы сейчас работаем,
#   поэтому restart планируется detached-таймером systemd-run через 3 секунды,
#   а не выполняется синхронно.
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 0

if ! cargo build --release --quiet; then
    echo "dsync post_pull: build failed — бинарь не обновлён" >&2
    exit 1
fi

install -m 0755 target/release/dsync "$HOME/.local/bin/dsync"

# Хаб — тот же бинарь под другим именем (CLI-алиас `daemon`).
if systemctl --user is-active --quiet dsync-hub 2>/dev/null; then
    install -m 0755 target/release/dsync "$HOME/.local/bin/dsync-hub"
    if command -v systemd-run >/dev/null 2>&1; then
        systemd-run --user --on-active=3 --unit=dsync-hub-selfupdate \
            systemctl --user restart dsync-hub >/dev/null 2>&1 || true
    else
        (sleep 3 && systemctl --user restart dsync-hub) >/dev/null 2>&1 &
    fi
    echo "dsync post_pull: hub binary updated, restart scheduled"
fi
