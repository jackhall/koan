#!/usr/bin/env python3
"""Parse an lcov.info file from `cargo llvm-cov --lcov` and print the
line-coverage total. With `--baseline FILE`, also maintain a per-commit
trend log (newest first, capped to 5 entries) and print a delta against
the prior top entry. Without `--baseline`, print a delta against the
prior top entry without modifying the file (read-only).

Trend-log format mirrors `observe/complexity.txt`:

    # columns: date  short-sha  line-pct  function-pct
    2026-05-16 5d882a1+ 86.06 89.62

`+` on the SHA marks a dirty working tree. Entries whose SHA (with any
trailing `+` stripped) is no longer an ancestor of HEAD are pruned.

  python3 tools/coverage.py --lcov observe/coverage.lcov \\
                            --baseline observe/coverage.txt
"""

from __future__ import annotations

import argparse
import datetime
import subprocess
import sys
from pathlib import Path


HEADER = (
    "# columns: date  short-sha  line-pct  function-pct\n"
    "# managed by tools/coverage.py --baseline; newest first, capped to 5 entries\n"
)


def parse_lcov(text: str) -> tuple[float, float]:
    """Return (line%, function%) totals from an lcov file."""
    lf = lh = fnf = fnh = 0
    for line in text.splitlines():
        if line.startswith("LF:"):
            lf += int(line[3:])
        elif line.startswith("LH:"):
            lh += int(line[3:])
        elif line.startswith("FNF:"):
            fnf += int(line[4:])
        elif line.startswith("FNH:"):
            fnh += int(line[4:])
    line_pct = 100 * lh / lf if lf else 0.0
    fn_pct = 100 * fnh / fnf if fnf else 0.0
    return line_pct, fn_pct


def _git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True)


def _short_sha() -> str | None:
    r = _git("rev-parse", "--short", "HEAD")
    return r.stdout.strip() if r.returncode == 0 else None


def _working_tree_dirty() -> bool:
    return _git("diff", "--quiet", "HEAD").returncode != 0


def _is_ancestor(sha: str) -> bool:
    return _git("merge-base", "--is-ancestor", sha, "HEAD").returncode == 0


def _parse_entry(line: str) -> tuple[str, str, float, float] | None:
    parts = line.split()
    if len(parts) < 4:
        return None
    try:
        return parts[0], parts[1], float(parts[2]), float(parts[3])
    except ValueError:
        return None


def _read_top_entry(path: Path) -> tuple[str, str, float, float] | None:
    if not path.exists():
        return None
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parsed = _parse_entry(stripped)
        if parsed is not None:
            return parsed
    return None


def update_baseline(path: Path, line_pct: float, fn_pct: float) -> None:
    sha = _short_sha() or "no-git"
    sha_field = f"{sha}+" if _working_tree_dirty() else sha
    today = datetime.date.today().isoformat()

    kept: list[str] = []
    for line in (path.read_text().splitlines() if path.exists() else []):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parsed = _parse_entry(stripped)
        if parsed is None:
            continue
        _, entry_sha, _, _ = parsed
        bare_sha = entry_sha[:-1] if entry_sha.endswith("+") else entry_sha
        if not _is_ancestor(bare_sha):
            continue
        kept.append(stripped)

    prior = _parse_entry(kept[0]) if kept else None
    kept.insert(0, f"{today} {sha_field} {line_pct:.2f} {fn_pct:.2f}")
    del kept[5:]
    path.write_text(HEADER + "\n".join(kept) + "\n")

    if prior is None:
        print(f"\ncoverage: line {line_pct:.2f}% — first run (recorded to {path}).")
    else:
        delta = line_pct - prior[2]
        print(f"\ncoverage: line {line_pct:.2f}% vs prior {prior[2]:.2f}% "
              f"from {prior[0]} {prior[1]} (Δ {delta:+.2f}, recorded to {path}).")


def print_delta(path: Path, line_pct: float) -> None:
    prior = _read_top_entry(path)
    if prior is None:
        print(f"\ncoverage: line {line_pct:.2f}% — no prior baseline.")
    else:
        delta = line_pct - prior[2]
        print(f"\ncoverage: line {line_pct:.2f}% vs prior {prior[2]:.2f}% "
              f"from {prior[0]} {prior[1]} (Δ {delta:+.2f}).")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--lcov", default="observe/coverage.lcov",
                    help="lcov file from `cargo llvm-cov --lcov` (default: observe/coverage.lcov)")
    ap.add_argument("--baseline", type=Path, default=None,
                    help="trend log to update; without this flag, the trend log is read-only "
                         "(e.g. --baseline observe/coverage.txt)")
    ap.add_argument("--read-baseline", type=Path, default=Path("observe/coverage.txt"),
                    help="trend log to read for the delta when --baseline is unset "
                         "(default: observe/coverage.txt)")
    args = ap.parse_args()

    lcov_path = Path(args.lcov)
    if not lcov_path.exists():
        print(f"lcov file not found: {lcov_path}", file=sys.stderr)
        return 1
    line_pct, fn_pct = parse_lcov(lcov_path.read_text())

    print(f"coverage totals (from {lcov_path}): line {line_pct:.2f}%, function {fn_pct:.2f}%")

    if args.baseline is not None:
        update_baseline(args.baseline, line_pct, fn_pct)
    else:
        print_delta(args.read_baseline, line_pct)
    return 0


if __name__ == "__main__":
    sys.exit(main())
