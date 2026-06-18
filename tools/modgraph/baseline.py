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
from pathlib import Path

from score import Score

BASELINE_HEADER = (
    "# columns: date  short-sha  score  coupling  nesting  size  root-loc\n"
    "# (the four scoring columns are total cost / fixed denominator D=1000;\n"
    "#  root-loc is the absolute subtree LOC, tracked for context only)\n"
    "# managed by `tools/modgraph score|regen --baseline`; newest first, capped to 5 entries\n"
)
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

    kept: list[str] = []
    for line in (path.read_text().splitlines() if path.exists() else []):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parsed = _parse_baseline_line(stripped)
        if parsed is None:
            continue
        _, entry_sha, _ = parsed
        # Strip the dirty marker before the ancestor check so dirty-tagged SHAs
        # are still tested against HEAD-ancestry like clean ones.
        bare_sha = entry_sha[:-1] if entry_sha.endswith("+") else entry_sha
        if not _git_is_ancestor(bare_sha):
            continue
        kept.append(stripped)

    prior = _parse_baseline_line(kept[0]) if kept else None
    kept.insert(0, f"{today} {sha_field} {per_loc:.2f} "
                   f"{score.coupling:.2f} {score.nesting:.2f} {score.size:.2f} "
                   f"{root_loc}")
    path.write_text(BASELINE_HEADER + "\n".join(kept[:BASELINE_LIMIT]) + "\n")

    if prior is None:
        print(f"\nbaseline: score {per_loc:.2f} — first run (recorded to {path}).")
    else:
        prior_date, prior_sha, prior_per_loc = prior
        delta = per_loc - prior_per_loc
        print(f"\nbaseline: score {per_loc:.2f} vs prior {prior_per_loc:.2f} "
              f"from {prior_date} {prior_sha} (Δ {delta:+.2f}, recorded to {path}).")
