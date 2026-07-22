"""Hub: массовый статус и pull всех git-репозиториев в общей корневой папке."""

import concurrent.futures
import shlex
from dataclasses import dataclass
from pathlib import Path

from .chezmoi import GitResult, _git
from .chezmoi import get_status as git_status


@dataclass
class HubRepo:
    name: str
    path: Path
    branch: str = ""
    is_clean: bool = True
    ahead: int = 0
    behind: int = 0
    has_remote: bool = False
    error: str = ""

    @property
    def status_text(self) -> str:
        if self.error:
            return f"error: {self.error}"
        parts = []
        if not self.is_clean:
            parts.append("dirty")
        if self.has_remote:
            if self.ahead:
                parts.append(f"ahead {self.ahead}")
            if self.behind:
                parts.append(f"behind {self.behind}")
        else:
            parts.append("no remote")
        return ", ".join(parts) if parts else "clean"


def discover_repos(root: Path) -> list[Path]:
    root = root.expanduser()
    if not root.is_dir():
        return []
    repos = []
    for child in sorted(root.iterdir()):
        if (
            child.is_dir()
            and not child.name.startswith(".")
            and (child / ".git").is_dir()
        ):
            repos.append(child)
    return repos


def check_repo(repo: Path, do_fetch: bool = True) -> HubRepo:
    hr = HubRepo(name=repo.name, path=repo)
    if do_fetch:
        _git(repo, ["fetch", "origin", "--quiet"], timeout=30)
    gs = git_status(repo)
    hr.branch = gs.current_branch
    hr.is_clean = gs.is_clean
    hr.ahead = gs.ahead
    hr.behind = gs.behind
    hr.has_remote = gs.has_remote
    hr.error = gs.error
    return hr


def collect_status(root: Path, jobs: int = 4) -> list[HubRepo]:
    repos = discover_repos(root)
    if not repos:
        return []
    workers = max(1, min(jobs, len(repos)))
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as ex:
        return list(ex.map(lambda r: check_repo(r, do_fetch=True), repos))


def pull_repo(repo: Path) -> GitResult:
    gs = git_status(repo)
    if gs.error:
        return GitResult(success=False, stderr=gs.error)
    if not gs.is_clean:
        return GitResult(success=False, stderr="dirty")
    if not gs.has_remote:
        return GitResult(success=False, stderr="no remote")
    fetch = _git(repo, ["fetch", "origin", "--quiet"], timeout=30)
    if not fetch.success:
        return fetch
    pull = _git(repo, ["pull", "--ff-only"], timeout=120)
    if not pull.success:
        return pull
    return GitResult(success=True, stdout=pull.stdout)


def pull_all(root: Path, jobs: int = 4) -> list[tuple[Path, GitResult]]:
    repos = discover_repos(root)
    if not repos:
        return []
    workers = max(1, min(jobs, len(repos)))
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as ex:
        results = list(ex.map(pull_repo, repos))
    return list(zip(repos, results))


def remote_hub_script(root: str) -> str:
    """Shell script: ff-pull всех чистых репо под root на удалённой машине.

    Выводит строки HUB|name|status|detail для парсинга на локальной стороне.
    """
    return f"""export PATH="$HOME/.local/bin:$PATH"
root=$(awk -F'"' '/^\\[hub\\]/{{f=1;next}} /^\\[/{{f=0}} f && /root/{{print $2}}' "$HOME/.config/dsync/config.toml" 2>/dev/null)
root=${{root:-{shlex.quote(root)}}}
root="${{root/#\\~/$HOME}}"
[ -d "$root" ] || {{ echo "HUB_ERROR|no root dir $root"; exit 0; }}
find "$root" -maxdepth 2 -name .git -type d 2>/dev/null | while read -r d; do
  repo="${{d%/.git}}"
  name="${{repo##*/}}"
  cd "$repo" 2>/dev/null || continue
  if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
    echo "HUB|$name|dirty|"
    continue
  fi
  if ! git rev-parse --abbrev-ref '@{{upstream}}' >/dev/null 2>&1; then
    echo "HUB|$name|noremote|"
    continue
  fi
  git fetch origin --quiet 2>/dev/null
  out=$(git pull --ff-only 2>&1)
  if [ $? -eq 0 ]; then
    case "$out" in
      *"Already up to date"*) echo "HUB|$name|uptodate|" ;;
      *) echo "HUB|$name|updated|" ;;
    esac
  else
    detail=$(echo "$out" | tr '\\n' ' ' | cut -c1-80)
    echo "HUB|$name|failed|$detail"
  fi
done"""
