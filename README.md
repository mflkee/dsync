# dsync

Decentralized dotfiles synchronization over NetBird.

`dsync` keeps your [chezmoi](https://www.chezmoi.io/) dotfiles in sync across
personal machines without exposing SSH to the public internet. It uses
[NetBird](https://netbird.io/) peer names/FQDNs to discover machines and pushes
changes over SSH inside the private mesh network.

## Features

- `dsync status` — show NetBird status, peers, and configured sync targets.
- `dsync sync` — commit local dotfile changes, push to GitHub, and pull/apply
  them on all configured remote machines.
- `dsync sync notebook desktop` — sync only specific machines.
- `dsync sync --only notebook --only desktop` — alternative way to select machines.
- `dsync sync --dry-run` — preview what `sync` would do without making changes.
- `dsync sync --jobs 8` — sync up to 8 remote machines in parallel (default: 4).
- `dsync push` — push current state to remote machines.
- `dsync push notebook` — push only to a specific machine.
- `dsync push --dry-run` — preview what `push` would do.
- `dsync pull` — pull latest dotfiles from GitHub and run `chezmoi apply`.
- `dsync pull --dry-run` — preview what `pull` would do.
- `dsync setup` — copy SSH keys to all configured machines.
- `dsync add <name> <host>` / `dsync remove <name>` — manage machine list.
- `dsync timer --enable` — run `dsync sync` every 30 minutes via systemd timer.
- `dsync zen export|import|info` — export/import Zen Browser profile data.

## Install

Requires `uv`, `git`, `openssh`, `chezmoi`, and `netbird`.

```bash
# 1. Install uv if you don't have it yet
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh

# 2. Clone and install dsync
export PATH="$HOME/.local/bin:$PATH"
git clone https://github.com/mflkee/dsync.git "$HOME/.local/share/dsync"
uv tool install --editable "$HOME/.local/share/dsync"

# 3. Make sure ~/.local/bin is in PATH
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshenv
```

## Configure

Edit `~/.config/dsync/config.toml`:

```toml
[machines]
notebook = { host = "archlinux-notebook-XXXXXX.netbird.cloud", user = "mflkee" }
desktop  = { host = "archlinux-desktop.netbird.cloud", user = "mflkee" }
server   = { host = "mkair-server.netbird.cloud", user = "mflkee" }

[git]
source = "~/dotfiles"
branch = "main"
# Optional: the URL used to clone the repo on new machines.
# Defaults to the local repo's origin remote.
# remote_url = "https://github.com/username/dotfiles.git"

# Optional: aliases for `dsync discover` to map NetBird hostnames to short names.
# [discover.aliases]
# archlinux-notebook = "notebook"
# archlinux-desktop = "desktop"
# mkair-server = "server"

# Optional: prefixes stripped from NetBird hostnames before alias lookup.
# [discover]
# prefixes = ["archlinux-", "mkair-"]

# Optional: logging settings.
# [logging]
# file = "~/.local/share/dsync/dsync.log"
# level = "INFO"
```

Each `host` must be a NetBird FQDN resolvable inside the mesh. Use the value
reported by `netbird status --json` under `fqdn`.

## First run

1. Ensure all target machines are online in NetBird.
2. Run `dsync setup` to copy your SSH key.
3. Run `dsync sync`.

## Optional: automatic sync

```bash
dsync timer --enable
systemctl --user status dsync.timer
```

## License

MIT
