import io
from pathlib import Path

from dsync import conflict
from dsync.chezmoi import GitResult


def _git_stub(calls: list[tuple[list[str], int]]):
    def fake_git(repo, args, timeout=30):
        calls.append((args, timeout))
        return GitResult(success=True)

    return fake_git


def test_resolve_interactive_keep_local_uses_ours(monkeypatch, tmp_path: Path):
    calls: list[tuple[list[str], int]] = []
    monkeypatch.setattr(conflict, "_git", _git_stub(calls))
    monkeypatch.setattr(conflict, "is_rebasing", lambda p: False)
    monkeypatch.setattr(conflict, "get_conflicted_files", lambda p: ["a.txt"])
    monkeypatch.setattr(conflict, "check_diverged", lambda p, b: (1, 1))
    monkeypatch.setattr(conflict, "show_diff_summary", lambda *a, **k: None)
    monkeypatch.setattr("sys.stdin", io.StringIO("1\n"))

    result = conflict.resolve_interactive(tmp_path, "main")

    assert result is True
    pull_calls = [c for c in calls if c[0][0] == "merge"]
    assert pull_calls == [(["merge", "-X", "ours", "origin/main"], 60)]


def test_resolve_interactive_keep_remote_uses_reset(monkeypatch, tmp_path: Path):
    calls: list[tuple[list[str], int]] = []
    monkeypatch.setattr(conflict, "_git", _git_stub(calls))
    monkeypatch.setattr(conflict, "is_rebasing", lambda p: False)
    monkeypatch.setattr(conflict, "get_conflicted_files", lambda p: ["a.txt"])
    monkeypatch.setattr(conflict, "check_diverged", lambda p, b: (1, 1))
    monkeypatch.setattr(conflict, "show_diff_summary", lambda *a, **k: None)
    monkeypatch.setattr("sys.stdin", io.StringIO("2\n"))

    result = conflict.resolve_interactive(tmp_path, "main")

    assert result is False
    assert any(c[0] == ["reset", "--hard", "origin/main"] for c in calls)


def test_safe_pull_detects_conflict(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(conflict, "is_rebasing", lambda p: True)
    monkeypatch.setattr(conflict, "has_conflict", lambda p: False)
    monkeypatch.setattr(conflict, "rebase_abort", lambda p: GitResult(success=True))
    monkeypatch.setattr(
        conflict,
        "_git",
        lambda p, a, timeout=30: GitResult(
            success=False, stderr="CONFLICT (content): Merge conflict in a.txt"
        ),
    )

    r, had_conflict = conflict.safe_pull(tmp_path, "main")
    assert had_conflict


def test_safe_pull_network_error_not_conflict(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(conflict, "is_rebasing", lambda p: False)
    monkeypatch.setattr(conflict, "has_conflict", lambda p: False)
    monkeypatch.setattr(
        conflict,
        "_git",
        lambda p, a, timeout=30: GitResult(
            success=False, stderr="fatal: unable to connect to github.com"
        ),
    )

    r, had_conflict = conflict.safe_pull(tmp_path, "main")
    assert not had_conflict


def test_has_conflict_detects_uu(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(
        conflict,
        "_git",
        lambda p, a, timeout=30: GitResult(
            success=True, stdout="UU a.txt\n M b.txt\n"
        ),
    )
    assert conflict.has_conflict(tmp_path) is True


def test_has_conflict_clean(monkeypatch, tmp_path: Path):
    monkeypatch.setattr(
        conflict,
        "_git",
        lambda p, a, timeout=30: GitResult(success=True, stdout=" M b.txt\n"),
    )
    assert conflict.has_conflict(tmp_path) is False
