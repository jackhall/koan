"""The tracked complexity trend log (`observe/complexity.txt`).

Prunes entries whose commit is no longer reachable from HEAD (branch switch,
hard reset, rebase drop), prepends today's measurement, trims to a fixed depth,
and prints a delta against the prior top entry. Dirty-snapshot (`+`) entries
survive pruning so a pre-commit hook (which always sees a staged-but-uncommitted
tree) doesn't erase the log.
"""
from __future__ import annotations

import datetime
import subprocess
import sys
from pathlib import Path

from score import Score

# `tools/` is the parent of this package's own directory, which is `sys.path[0]`.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from trendlog import render  # noqa: E402  (needs the path above)

BASELINE_NOTES = [
    "the four scoring columns are total cost / fixed denominator D=1000;",
    "root-loc is the absolute subtree LOC, tracked for context only",
    "managed by `tools/modgraph score|regen --baseline`; newest first, capped to 5 entries",
]
BASELINE_COLUMNS = ["date", "short-sha", "score", "coupling", "nesting", "size", "root-loc"]
BASELINE_LIMIT = 5


def _git(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], capture_output=True, text=True)


def _git_short_sha() -> str | None:
    r = _git("rev-parse", "--short", "HEAD")
    return r.stdout.strip() if r.returncode == 0 else None


def _git_working_tree_dirty() -> bool:
    r = _git("status", "--porcelain")
    return r.returncode == 0 and bool(r.stdout.strip())


def _git_is_ancestor(sha: str) -> bool:
    return _git("merge-base", "--is-ancestor", sha, "HEAD").returncode == 0


def _parse_baseline_line(line: str) -> tuple[str, str, float] | None:
    parts = line.split()
    if len(parts) < 3:
        return None
    try:
        return parts[0], parts[1], float(parts[2])
    except ValueError:
        return None


def _entry_fields(line: str) -> list[str] | None:
    """One recorded row's fields, or none when the line is not a row. Kept as written
    rather than reformatted: only the leading three are parsed for the delta, and a
    row's own precision is what it was recorded at."""
    parts = line.split()
    return parts if len(parts) == len(BASELINE_COLUMNS) else None


def _read_top_entry(path: Path) -> tuple[str, str, float] | None:
    """The newest recorded measurement, or none when the log is absent or bare."""
    for line in (path.read_text().splitlines() if path.exists() else []):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parsed = _parse_baseline_line(stripped)
        if parsed is not None:
            return parsed
    return None


def read_delta(path: Path, score: Score) -> None:
    """Print the delta a read-only run reports — the same line `update_baseline`
    prints, minus the recording. The trend log holds one entry per commit, so a
    local sanity-check reads it rather than writing to it, and still gets to name
    the number it moved from."""
    prior = _read_top_entry(path)
    if prior is None:
        print(f"\nbaseline: score {score.total:.2f} — no prior baseline.")
    else:
        prior_date, prior_sha, prior_per_loc = prior
        print(f"\nbaseline: score {score.total:.2f} vs prior {prior_per_loc:.2f} "
              f"from {prior_date} {prior_sha} (Δ {score.total - prior_per_loc:+.2f}).")


def update_baseline(path: Path, score: Score, root_loc: int) -> None:
    """Prune stale entries, prepend today's measurement, write the file, and
    print a one-line delta against the prior top entry.

    Pruning rule:
      - Drop any entry whose SHA (stripping a trailing `+` dirty marker) is no
        longer an ancestor of HEAD. Covers `git checkout` to a different
        branch, `git reset --hard` past the commit, and rebase drops.

    Dirty-snapshot (`+`-suffixed) entries are kept: when modgraph runs from a
    pre-commit hook, the staged-but-not-yet-committed tree is by definition
    dirty, so pruning `+` entries on every run would erase the trend log.
    """
    sha = _git_short_sha() or "no-git"
    sha_field = f"{sha}+" if _git_working_tree_dirty() else sha
    today = datetime.date.today().isoformat()
    per_loc = score.total

    kept: list[list[str]] = []
    for line in (path.read_text().splitlines() if path.exists() else []):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        fields = _entry_fields(stripped)
        if fields is None:
            continue
        entry_sha = fields[1]
        # Strip the dirty marker before the ancestor check so dirty-tagged SHAs
        # are still tested against HEAD-ancestry like clean ones.
        bare_sha = entry_sha[:-1] if entry_sha.endswith("+") else entry_sha
        if not _git_is_ancestor(bare_sha):
            continue
        kept.append(fields)

    prior = _parse_baseline_line(" ".join(kept[0])) if kept else None
    kept.insert(0, [today, sha_field, f"{per_loc:.2f}", f"{score.coupling:.2f}",
                    f"{score.nesting:.2f}", f"{score.size:.2f}", str(root_loc)])
    path.write_text(render(BASELINE_NOTES, BASELINE_COLUMNS, kept[:BASELINE_LIMIT]))

    if prior is None:
        print(f"\nbaseline: score {per_loc:.2f} — first run (recorded to {path}).")
    else:
        prior_date, prior_sha, prior_per_loc = prior
        delta = per_loc - prior_per_loc
        print(f"\nbaseline: score {per_loc:.2f} vs prior {prior_per_loc:.2f} "
              f"from {prior_date} {prior_sha} (Δ {delta:+.2f}, recorded to {path}).")
