import tomllib
import os
from pathlib import Path

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
"""


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

    def add_machine(self, name: str, host: str, user: str = "mflkee"):
        machines = self.data.setdefault("machines", {})
        machines[name] = {"host": host, "user": user}
        self._save()

    def _save(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        lines = ["# dsync configuration\n"]
        lines.append("[machines]\n")
        for name, info in self.data.get("machines", {}).items():
            host = info.get("host", "")
            user = info.get("user", "mflkee")
            lines.append(f'{name} = {{ host = "{host}", user = "{user}" }}\n')
        git = self.data.get("git", {})
        lines.append("\n[git]\n")
        lines.append(f'source = "{git.get("source", str(CHEZMOI_SOURCE))}"\n')
        lines.append(f'branch = "{git.get("branch", "main")}"\n')
        self.path.write_text("".join(lines))

    @classmethod
    def ensure_default(cls):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        if not CONFIG_PATH.exists():
            CONFIG_PATH.write_text(DEFAULT_CONFIG)
            return Config(CONFIG_PATH)
        return cls()
