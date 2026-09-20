# dsync

**Multi-machine dotfiles & project sync across your fleet — git-driven, hub-coordinated, no public SSH needed.**

[![CI](https://github.com/mflkee/dsync/actions/workflows/ci.yml/badge.svg)](https://github.com/mflkee/dsync/actions/workflows/ci.yml)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/badge/crates.io-soon-orange.svg)](https://crates.io)

`dsync` keeps the same git repos (dotfiles, configs, projects) in sync across all
your machines. It runs inside your private network (NetBird, Tailscale, WireGuard,
a VPN, or a VPS) and never exposes SSH to the public internet.

```
┌──────────────┐   QUIC    ┌──────────────┐        ┌──────────────┐
│  desktop     │ ────────▶ │   hub        │ ──────▶ │  notebook    │
│  (client)    │           │ (coordinator)│  SSH    │  (client)    │
└──────────────┘           └──────┬───────┘        └──────────────┘
         ▲                        │                    ▲
         └── git push/pull ───────┴── git pull + post_pull hooks
```

## Why dsync?

- **Fleet-wide, not single-host.** chezmoi/yadm/dotbot sync *one* machine.
  `dsync` *pushes a change once* and the hub triggers `git pull` + `post_pull`
  on every other machine in your fleet.
- **No public SSH, no cloud agent.** All traffic stays inside your private mesh.
- **Just git underneath.** Each project is a normal git repo (GitHub, Gitea,
  cgit, or a bare repo on the hub). Content transport is battle-tested git;
  `dsync` adds the orchestration.
- **Zero learning curve for dotfiles.** `dsync init` sets everything up for you
  — it can even install and drive [chezmoi](https://www.chezmoi.io/) under the
  hood, so you never have to learn a second tool.
- **Guard rails on top of plain git pull:** shell-quoted remote commands, hard
  SSH timeouts so dead machines can't hang the hub, atomic state storage.

## Quickstart

```sh
# Install (crates.io): soon
cargo install dsync            # or: cargo install --git https://github.com/mflkee/dsync

# Walk through setup: machine name, hub address, machines, projects, scheduler
dsync init

# Push your state to the hub — the hub then pulls + applies on all other machines
dsync push

# See what happened anywhere
dsync status
dsync doctor
dsync tui
```

## Commands

| Command | Description |
|---------|-------------|
| `dsync init` | Interactive setup wizard (machine, hub, remotes, projects, scheduler) |
| `dsync push [machine]` | Commit local changes, push to git origin, notify hub. Also captures live dotfile edits (see `dsync capture`) |
| `dsync capture <path>...` | Instant capture of chezmoi-managed live files (or `~/dotfiles` source edits) into dotfiles repo, then push fleet state — no `chezmoi edit` needed |
| `dsync pull [machine]` | Fetch fleet state from the hub |
| `dsync status` | Show online/offline machines and their sync state |
| `dsync doctor` | Diagnose config, SSH keys, network, hub reachability |
| `dsync tui` | Full TUI: dashboard, projects, machines, doctor, log, config editor |
| `dsync hub` | Run the hub daemon (QUIC server + SSH-pull coordinator) |
| `dsync watch` | Background sync loop (poll+push+pull on an interval) |
| `dsync trust list` / `rm <addr>` | Manage trusted hub fingerprints (TOFU) |
| `dsync bot` | Telegram bot: exec / shell / opencode commands on fleet machines |
| `dsync dotfiles ...` | chezmoi wrapper: `add`, `apply`, `diff`, `status`, `edit` |

## Config

`dsync` looks for `dsync.toml` (repo-local), `~/.config/dsync/dsync/config.toml`,
or `/etc/dsync/config.toml` — in that order.

```toml
config_version = 1

[machine]
name = "desktop"

[hub_connect]
address = "100.89.126.211:42069"   # hub inside your private network

[projects.dotfiles]
path = "~/dotfiles"
branch = "main"
machines = ["desktop", "notebook", "server"]
post_pull = "chezmoi apply --force"

[projects.dsync]
path = "~/projects/dsync"
branch = "main"
machines = ["desktop", "notebook", "server"]
post_pull = "cargo build --release && cp target/release/dsync ~/.local/bin/dsync"

[remote.desktop]
host = "192.168.1.10"
port = 22
user = "me"

# When running the hub on this machine:
[hub]
bind = "0.0.0.0:42069"
data_dir = "~/.local/share/dsync-hub"
```

Each project's git `origin` is where content actually lives (your GitHub/Gitea
repo, or a bare repo on the hub). The hub only coordinates: it records each
machine's state and triggers SSH pulls with `post_pull` hooks.

## Capturing live dotfile edits (no `chezmoi edit`)

Plain `git add/commit` only sees changes **inside repos**. Editing a live file
like `~/.zshrc` or `~/.config/nvim/init.lua` never touches the `dotfiles` repo,
so it used to go nowhere. `dsync` fixes that by driving chezmoi under the hood:

- **Periodic (catches anything — bash, sed, scripts, nvim):** every `dsync
  push` / `dsync watch` compares sha256 of live files against a snapshot
  (`~/.local/share/dsync/capture-state.json`) and `chezmoi re-add`s whatever
  changed before committing. Edits to `~/dotfiles/dot_config/...` sources are
  `chezmoi apply --force`-d locally so the live file follows immediately.
- **Instant:** an optional nvim `BufWritePost` hook calls `dsync capture <path>`
  on save, so the fleet gets the change right away.

Behavior controls in `[capture]` (all optional):

```toml
[capture]
watch   = ["~/.zshrc", "~/.config", "~/.local/bin"]   # default: all chezmoi-managed files
exclude = ["~/.config/ghostty/themes"]                # never auto-captured
```

Skipped by default: files whose source ends `.tmpl` (template output — edit the
template instead), generated `~/.local/share/applications`/themes, media
(`~/Pictures`), nested git repos, and anything not managed by chezmoi so
`dsync capture` never pollutes the dotfiles repo with random project files.

## Running the hub

```sh
dsync hub          # foreground
# or as a service: systemd user unit (Linux), LaunchAgent (macOS),
# Task Scheduler (Windows), or plain `dsync watch`-driven loop
```

The hub needs SSH access to every machine (an `ed25519` key is generated by
`dsync init` if missing; install the public key on each target).

## Security: trust on first use (TOFU)

The hub runs a self-signed QUIC/TLS certificate (persisted in its data dir, so
the fingerprint is stable across restarts). Clients use **TOFU**: the first
connection remembers the hub's certificate fingerprint in
`~/.local/share/dsync/known_hosts.toml`, every later connection verifies it.

- A changed fingerprint (new hub, reinstall, or MITM) **fails the connection**
  with an actionable error instead of silently accepting.
- Manage trust: `dsync trust list` shows known fingerprints,
  `dsync trust rm <addr>` forgets one (next connect re-trusts).
- `dsync doctor` reports whether the configured hub is already trusted.
- Want real CA-verified TLS instead? Point `[hub] cert` / `[hub] key` at a
  certificate issued by a CA you trust — the client still requires the
  fingerprint to match unless you remove the entry.

## Requirements

- Rust 1.80+ to build; git + an SSH server on target machines at runtime.
- Works on Linux, macOS, Windows.

## License

MIT. See [LICENSE](LICENSE).