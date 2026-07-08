import tomllib
from pathlib import Path

import tomli_w

CONFIG_DIR = Path.home() / ".config" / "dsync"
CONFIG_PATH = CONFIG_DIR / "config.toml"
CHEZMOI_SOURCE = Path.home() / "dotfiles"

DEFAULT_CONFIG = """# dsync configuration
# Machines to sync chezmoi dotfiles to
[machines]
# name = { host = "hostname.netbird.cloud", user = "username" }

[git]
source = "~/dotfiles"
branch = "main"
# Optional: the URL used to clone the repo on new machines.
# remote_url = "https://github.com/username/dotfiles.git"

# Optional: aliases for NetBird machine discovery.
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

# Optional: projects to sync across machines.
# [projects]
# myapp = { path = "~/projects/myapp", remote = "git@github.com:mflkee/myapp.git", machines = ["notebook", "desktop"] }
"""

DEFAULT_DISCOVER_PREFIXES = ("archlinux-", "mkair-")


class Config:
    def __init__(self, path: Path = CONFIG_PATH):
        self.path = path
        self.data = self._load()

    def _load(self) -> dict:
        if not self.path.exists():
            return {
                "machines": {},
                "git": {"source": str(CHEZMOI_SOURCE), "branch": "main"},
            }
        raw = self.path.read_text()
        return tomllib.loads(raw)

    @property
    def machines(self) -> dict:
        return self.data.get("machines", {})

    @property
    def git_source(self) -> Path:
        src = self.data.get("git", {}).get("source", str(CHEZMOI_SOURCE))
        return Path(src).expanduser()

    @property
    def git_branch(self) -> str:
        return self.data.get("git", {}).get("branch", "main")

    @property
    def git_remote_url(self) -> str | None:
        return self.data.get("git", {}).get("remote_url")

    @property
    def discover_aliases(self) -> dict[str, str]:
        return self.data.get("discover", {}).get("aliases", {})

    @property
    def discover_prefixes(self) -> tuple[str, ...]:
        prefixes = self.data.get("discover", {}).get("prefixes")
        if prefixes is None:
            return DEFAULT_DISCOVER_PREFIXES
        return tuple(prefixes)

    @property
    def log_file(self) -> Path:
        path = self.data.get("logging", {}).get("file")
        if path:
            return Path(path).expanduser()
        return Path.home() / ".local" / "share" / "dsync" / "dsync.log"

    @property
    def log_level(self) -> str:
        return self.data.get("logging", {}).get("level", "INFO")

    @property
    def projects(self) -> dict[str, dict]:
        return self.data.get("projects", {})

    def add_machine(self, name: str, host: str, user: str = "mflkee"):
        machines = self.data.setdefault("machines", {})
        machines[name] = {"host": host, "user": user}
        self._save()

    def _save(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("wb") as f:
            tomli_w.dump(self.data, f)

    @classmethod
    def ensure_default(cls):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        if not CONFIG_PATH.exists():
            CONFIG_PATH.write_text(DEFAULT_CONFIG)
            return Config(CONFIG_PATH)
        return cls()
