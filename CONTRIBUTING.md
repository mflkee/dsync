# Contributing to dsync

Thanks for taking the time to contribute! This project is young, so before
opening a PR it's worth opening an issue or a discussion first.

## Development setup

```sh
git clone git@github.com:mflkee/dsync.git
cd dsync
cargo build
cargo test
```

`dsync` requires no system services to build — all dependencies are pure Rust
(quinn/rustls for QUIC, russh for SSH, ratatui for the TUI).

## Code style

- `cargo fmt` and `cargo clippy -- -D warnings` must pass locally.
- New public subcommands need at least a smoke test in `tests/`.
- Keep the TUI thread free of blocking work: all network I/O lives in the
  backend thread and reaches the UI via the crossbeam channel.

## Commit messages

Follow the existing convention — a short imperative summary line,
optionally with a scope prefix:

```
tui: poll hub immediately at startup
hub: atomic state save (tmp+rename)
```

## What not to do

- Do not re-introduce the legacy Python implementation (v1). It lives in git
  history only and is replaced by the Rust binary.
- Do not add code that depends on systemd, XDG paths that differ across OSes,
  or Linux-only shell commands — dsync targets Linux/macOS/Windows.