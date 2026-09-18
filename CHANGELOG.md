# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cross-platform foundation: built-in `dsync watch` scheduler (no systemd required).
- Interactive `dsync init` setup wizard (planned).
- chezmoi wrapper subcommands under `dsync dotfiles` (planned).
- GitHub Actions CI matrix (Linux/macOS/Windows).
- Config schema versioning (`config_version` field).

### Changed

- Repository cleaned: legacy Python v1 implementation removed (see git history).
- Hub binary deployment path documented as `dsync-hub`.

### Security

- TLS certificate verification on the client side (TOFU): first connection
  remembers the hub fingerprint, later ones verify it; `dsync trust list|rm`
  manages stored fingerprints. Hub persists its self-signed cert in the data dir
  so the fingerprint is stable across restarts.

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