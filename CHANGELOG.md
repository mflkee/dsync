# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **State sync hardening** (`state-sync-hardening`), по итогам аудита 11.11.2026:
  - *Pagination*: `state_pull` отдаёт состояние страницами (`max_bytes`,
    `next_seq`, `has_more`). Раньше весь стор (61 элемент ≈ 66 МБ
    opencode-сессий) уезжал одним JSON-сообщением: пик RSS хаба 512 МБ и
    риск OOM. Старые клиенты (`max_bytes = 0`) получают всё как раньше.
  - *Channel filter*: клиент сообщает нужные каналы, хаб не присылает чужие
    (машина с выключенным `opencode` не скачивает мегабайты сессий).
  - *Idempotent apply*: уже применённые элементы пропускаются, поэтому
    повторная страница/ретрай не перезапускает `tmux-resurrect restore`
    (наблюдалось раз в 15 минут на сервере).
  - *Dead-letter вместо вечного блока*: один сбойный элемент больше не
    откатывает seq и не держит канал в цикле «скачать 66 МБ → 60 ошибок».
    `state.retry_limit` (по умолчанию 3) попытки → dead-letter; смотреть
    `dsync state list`, повторять `dsync state retry [channel:key]`.
  - `[state.opencode] projects = []` теперь действительно отключает канал
    (раньше отключал только экспорт, а применение чужих сессий продолжалось).
- **Pull economy** (`pull-economy`): хаб пропускает SSH-пулл, если
  `commit_hash` проекта совпадает с уже развёрнутым на машине
  (`pull_only_on_change`, состояние в `deployed.json`), и не трогает машины,
  не выходившие на связь дольше `offline_skip_after_secs` (по умолчанию 45 мин).
  Ошибки уровня сети/аутентификации (`No route to host`, `Connection refused`,
  `Permission denied`) больше не ретраятся трижды. Было ~1700 пуллов в сутки,
  из них ~1000 — в офлайн-машину.
- **Hub self-update**: `scripts/post-pull.sh` собирает бинарь, обновляет
  клиент и, если на машине запущен хаб, планирует его перезапуск
  detached-таймером (`systemd-run --on-active=3`), чтобы не убить текущую
  SSH-сессию пулла.
- **Version visibility**: `PROTOCOL_VERSION` + `version_string()` с git-хешем
  (`build.rs`: `0.1.0+535cf06 (proto 2)`); каждый push несёт версию клиента,
  хаб копит её в `MachineState`, `dsync status`/`doctor` показывают версию хаба
  и **каждой машины** — дрейф сборок виден сразу (раньше четыре машины с
  разными бинарями дружно назывались `0.1.0`). `dsync status --json` — для
  мониторинга; в статусе также локальный state (seq/applied/failing/dead-letter).
- **`dsync state`**: `list` (счётчики, сбойные и dead-letter элементы) и
  `retry [channel:key]`.
- **`dsync doctor`**: версия хаба vs клиента, сводка state-стора (элементы,
  байты, последний origin), локальные сбои/dead-letter, осиротевшие tmp-файлы,
  поддержка `opencode session import` (V1 vs V2).
- **Hub hygiene**: при старте хаб удаляет осиротевшие `*.tmp-*` старше часа
  (наблюдалось 571 файл) и элементы каналов, которых больше нет в конфиге
  (например `zellij` после удаления поддержки); в `state_status` добавлен
  размер канала в байтах.
- **Push speed**: авто-обнаружение проектов сканирует репозитории параллельно
  (29 репо × ~5 git-вызовов последовательно = 8–11 с на push); лог пишет
  только реально новые проекты (было «auto-discovered 29 new» каждый цикл).

- **TUI improvements** (`tui-improvements`):
  - *Config-safe editor*: config mutations from the TUI now go through
    `toml_edit` patches (`ConfigEditor::apply_patch`) — comments, unknown
    sections and Go-template syntax in the chezmoi source file survive every
    add/remove/toggle. On chezmoi-managed configs each save is explicitly
    confirmed, written to the source template and followed by
    `chezmoi apply <target>` (refused with an actionable message when no
    template is found). Every save logs a precise diff
    (`+ projects.<name> added`, `~ … changed`, `- … removed`).
  - *Full form editing*: caret-aware input in all forms — Left/Right/Home/End,
    Backspace/Delete, Ctrl-U (UTF-8-safe, by `char` boundaries), Tab/↑↓ for
    field navigation, caret rendering and an "insert at column N" hint.
  - *Pull visibility*: `e` on Dashboard/Machines opens a failed-pulls overlay
    (machine × project × attempts × last error from `MachineStatus.pulls`,
    with scroll and a proper empty state).
  - *Mouse*: terminal init enables mouse capture (graceful degrade to keyboard
    scroll if unsupported); wheel scrolls Log/Help/Doctor while no form is
    open; a panic hook restores the terminal on every exit path.
  - *State tab*: read-only `state_status` hub request (new protocol message)
    summarizing the state store per channel (item count, last updated/origin,
    last sync error) plus the local `[state]` config; toggles for
    `tmux`/`tmux_restore` (with confirmation) and an editor for the
    `[state.opencode] projects` list — all saved through the safe patch path.
    Old hubs answer an error envelope → the tab shows "unavailable".
- **Auto-project discovery** (`auto-projects`): dsync watches a root directory
  (default `~/projects`, depth 1) and automatically announces any new git repo
  to the fleet — no manual `[projects.*]` config edit needed. Configure via
  `[auto_projects] { root, branch, machines }`. Announced projects are
  **bootstrapped** on machines that don't have them yet: the hub `git clone`s
  them from their origin URL (reported by the pushing machine), then keeps
  pulling as usual (`git pull --rebase --autostash`). Auto-discovered repos are
  announced but not auto-committed — dsync-owned repos stay in `[projects.*]`.
- `dsync capture <path>` + automatic capture of live dotfile edits inside
  `dsync push`: edit `~/.zshrc`, `~/.config/...` etc. from anywhere (nvim,
  bash, sed, scripts) and the change goes fleet-wide — no `chezmoi edit`
  needed. Under the hood dsync drives chezmoi (`re-add`/`apply`), see
  `[capture]` in the config and `src/client/capture.rs`.
- **Hub token auth** (`hub-auth`): per-machine tokens in `[hub] tokens`
  validated constant-time (`subtle`) on every `push`/`pull`/`status` request;
  clients send `[hub_connect] token` on each request. The hub **refuses to
  start** without tokens. See the README migration steps.
- **Hub request limits** (`hub-request-limits`): bounded request reads
  (`max_message_size`, default 8 MiB) and a `max_concurrency` semaphore
  (default 32, 5 s acquisition timeout → explicit "hub busy" error).
- **SSH-pull orchestration** (`hub-pull-orchestration`): hub pulls retry with
  `pull_retries` (backoff `5s·2ⁿ`, cap 60 s) and record a `PullOutcome`
  (ok/error/attempts/finished) per machine×project; `dsync status` and the TUI
  show pull result lines.
  Split timeouts: connect stays at 30 s (unreachable hosts fail fast), exec
  (`git pull && post_pull`, sometimes a `cargo build`) gets `pull_timeout_secs`
  (default 300). State saves are serialized with a unique temp file — no more
  concurrent-rename ENOENT when several pulls finish at once.
- **State retention** (`hub-state-retention`): machines not seen for
  `retention_days` (default 30) are pruned with atomic save; machines with
  `last_push == 0` are never evicted.
- **SSH host-key TOFU** (`ssh-host-trust`): the hub pins `host:port` SSH host
  fingerprints in `ssh_known_hosts.toml` (fingerprint via russh 0.44
  `PublicKey` bytes). First pull trusts and persists; an unchanged key is
  accepted; a changed key fails the pull with a *stored vs presented* message.
  `dsync trust ssh list` / `rm <host:port>` manage the store; `dsync doctor`
  reports per-remote trust state.
- Cross-platform foundation: built-in `dsync watch` scheduler (no systemd required).
- Interactive `dsync init` setup wizard (planned).
- chezmoi wrapper subcommands under `dsync dotfiles` (planned).
- GitHub Actions CI matrix (Linux/macOS/Windows).
- Config schema versioning (`config_version` field).

### Changed

- Repository cleaned: legacy Python v1 implementation removed (see git history).
- Hub binary deployment path documented as `dsync-hub`.
- Old `machines.json` files (with or without `pulls`) still load; the new binary
  writes the extended schema, and the previous binary ignores the extra fields
  (additive schema, safe rollback).

### Security

- TLS certificate verification on the client side (TOFU): first connection
  remembers the hub fingerprint, later ones verify it; `dsync trust list|rm`
  manages stored fingerprints. Hub persists its self-signed cert in the data dir
  so the fingerprint is stable across restarts.
- Hub token auth now locks the hub (above).

> **BREAKING:** hub starts require `[hub] tokens` and clients must send
> `[hub_connect] token`. Deploy order and rollback: see README → "Security: hub
> token auth". Config tokens belong in a chezmoi/`age` template, never in git.

## [0.1.0] — 2026-09-18

### Added

- QUIC hub + client (`dsync hub` daemon, `dsync push` / `dsync pull` / `dsync status`).
- Multi-machine orchestration: hub triggers SSH pulls + `post_pull` hooks on all remotes.
- SSH transport over russh with hard timeout; configurable key path.
- TUI (`dsync tui`) with Dashboard/Projects/Machines/Doctor/Log/Help tabs
  and in-app config editing.
- Telegram bot (`dsync bot`) with exec/shell/opencode modes.
- `dsync doctor` diagnostics.
- Atomic hub state save (tmp + rename), hub-side clock for `last_push` time.
```