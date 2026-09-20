# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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