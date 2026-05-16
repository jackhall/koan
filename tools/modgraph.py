#!/usr/bin/env python3
"""Measure a fractal complexity index for a module subtree.

Reads a cargo-modules DOT file, recursively walks the module tree from
`--root`, and reports per node:
  index(m)   = cross_edges(m) + alpha * feedback_weight(m) + beta
  size(m)    = gamma * own_loc(m) * log(1 + own_loc(m) / pivot)
The aggregate is reported as a single normalised number — the per-root-loc
score, split into a structural and a size component:
  per-loc    = (Σ index(m) · loc(m) + Σ size(m)) / loc(root)
The split lets you see whether complexity is structural (coupling /
nesting) or per-file (large files). The absolute totals are intentionally
not surfaced — only the per-loc number is calibrated to compare against
prior runs.

`cross_edges` and `feedback_weight` are computed at every interior node
against the one-group-per-child partition of its children. For N ≤ 6
children the best topological order is found by exhaustive search;
above that, the Eades-Lin-Smyth GR heuristic is used.

The per-loc number is normalised by the root subtree's LOC (a fixed
constant for a given root). This is what makes nesting cost something:
every interior level contributes its own loc to the sum, so adding a
wrapper around a heavy subtree adds (β + cross + α·fb) · loc.

`beta` is the per-non-leaf charge (default 20). `beta-children-pivot` P
(default 3) scales it by `max(1, P/children)`, so a 2-child wrapper pays
1.5× β while a 3+ child wrapper pays full β. The intent: punish thin
pass-through wrappers (e.g. `runtime` hosting only `builtins` + `machine`)
without treating cohesive groupings (e.g. `model` over ast/types/values,
`values` over 4 leaves) as overhead. Calibrated by a joint β×P sweep on
the koan tree to the cleanest decision boundary: dissolving a 2-child
thin wrapper unambiguously wins, while dissolving any 3+ child cohesive
grouping unambiguously loses. Pushing β much higher makes the layer cost
grow linearly with subtree-loc, which falsely accepts dissolving
medium-cohesion 3-child wrappers; pushing P higher does the same for
4-child wrappers. Setting P=0 disables scaling for flat β. At β=0 a
passthrough wrapper is undetectable.

`gamma`/`pivot` shape the per-file size charge. Without it, a single
3000-line leaf scores zero while any split incurs structural cost, so
the metric strictly rewards inaction. The charge `γ · L · log(1 + L/T)`
is sub-linear in L for L ≪ T (small files are nearly free) and turns
super-linear as L ≫ T. Defaults (γ=10, T=200) put a 200-line file at
~1386 and a 1000-line file at ~17918, so a 1000-LOC leaf split into a
wrapper + five 200-LOC children gives a clear net per-loc drop (~−0.66
on koan) — the size savings exceed the wrapper's β·loc cost. Applied to
every module's own file (leaves and parents alike), so fat `mod.rs`
files above small children are also penalised.

Usage:
  python3 tools/modgraph.py --edges <dot-file> --root koan
  python3 tools/modgraph.py --edges <dot-file> --root koan::runtime::machine

Pass `--baseline <file>` to record the run in a tracked baseline file and
print a delta against the prior top entry. The flag also prunes stale
entries automatically: any entry whose SHA is no longer reachable from
HEAD (branch checkout, hard reset, rebase drop) is removed, and every
prior dirty-snapshot (`+`-suffixed) entry is removed before today's
measurement is prepended. Trimmed to 5 entries.

  python3 tools/modgraph.py --edges /tmp/koan.dot --root koan \\
                            --baseline tools/complexity.txt
"""

from __future__ import annotations

import argparse
import datetime
import itertools
import math
import re
import subprocess
from collections import defaultdict
from pathlib import Path

EDGE_RE = re.compile(r'\s*"([^"]+)"\s*->\s*"([^"]+)".*\[label="uses"')


def load_edges(path: Path) -> list[tuple[str, str]]:
    edges = []
    for line in path.read_text().splitlines():
        m = EDGE_RE.match(line)
        if m:
            edges.append((m.group(1), m.group(2)))
    return edges


def classify(module: str, partition: dict[str, list[str]]) -> str | None:
    best_group, best_len = None, -1
    for group, prefixes in partition.items():
        for p in prefixes:
            if (module == p or module.startswith(p + "::")) and len(p) > best_len:
                best_group, best_len = group, len(p)
    return best_group


def build_matrix(
    edges: list[tuple[str, str]], partition: dict[str, list[str]]
) -> tuple[dict[tuple[str, str], int], int, int]:
    matrix: dict[tuple[str, str], int] = defaultdict(int)
    cross = 0
    unclassified = 0
    for src, dst in edges:
        sg = classify(src, partition)
        dg = classify(dst, partition)
        if sg is None or dg is None:
            unclassified += 1
            continue
        if sg != dg:
            matrix[(sg, dg)] += 1
            cross += 1
    return matrix, cross, unclassified


def feedback(order: list[str], matrix: dict[tuple[str, str], int]) -> int:
    rank = {g: i for i, g in enumerate(order)}
    return sum(w for (a, b), w in matrix.items() if rank[a] > rank[b])


def best_order_exact(
    groups: list[str], matrix: dict[tuple[str, str], int]
) -> tuple[list[str], int]:
    best, best_fb = None, None
    for perm in itertools.permutations(groups):
        fb = feedback(list(perm), matrix)
        if best_fb is None or fb < best_fb:
            best, best_fb = list(perm), fb
    return best, best_fb


def best_order_greedy(
    groups: list[str], matrix: dict[tuple[str, str], int]
) -> tuple[list[str], int]:
    """Eades-Lin-Smyth GR heuristic for weighted minimum feedback arc set."""
    remaining = set(groups)
    s1: list[str] = []
    s2: list[str] = []

    def out_weight(g: str) -> int:
        return sum(w for (a, b), w in matrix.items() if a == g and b in remaining)

    def in_weight(g: str) -> int:
        return sum(w for (a, b), w in matrix.items() if b == g and a in remaining)

    while remaining:
        progress = True
        while progress:
            progress = False
            for g in sorted(remaining):
                if out_weight(g) == 0:
                    s2.insert(0, g)
                    remaining.remove(g)
                    progress = True
            for g in sorted(remaining):
                if in_weight(g) == 0:
                    s1.append(g)
                    remaining.remove(g)
                    progress = True
        if not remaining:
            break
        pick = max(sorted(remaining),
                   key=lambda g: out_weight(g) - in_weight(g))
        s1.append(pick)
        remaining.remove(pick)

    order = s1 + s2
    return order, feedback(order, matrix)


def discover_modules(edges: list[tuple[str, str]]) -> set[str]:
    return {m for edge in edges for m in edge}


def direct_children(parent: str, modules: set[str]) -> list[str]:
    prefix = parent + "::"
    seen = set()
    for m in modules:
        if m.startswith(prefix):
            seen.add(m[len(prefix):].split("::", 1)[0])
    return sorted(seen)


def module_to_file(module: str, src_root: Path) -> Path | None:
    """`koan::dispatch::types::ktype` -> `src/dispatch/types/ktype.rs` (or `.../mod.rs`)."""
    parts = module.split("::")[1:]
    if not parts:
        return None
    flat = src_root.joinpath(*parts).with_suffix(".rs")
    if flat.exists():
        return flat
    nested = src_root.joinpath(*parts, "mod.rs")
    if nested.exists():
        return nested
    return None


def _is_test_file(path: Path) -> bool:
    name = path.name
    if name == "test_support.rs" or name.endswith("_tests.rs") or name == "tests.rs":
        return True
    return any(part == "tests" for part in path.parts)


def _strip_comments(text: str) -> list[str]:
    """Remove line and block comments (including `///` and `//!` doc comments).
    Naive about string literals — acceptable for a LOC proxy."""
    out_lines: list[str] = []
    in_block = False
    for line in text.splitlines():
        buf = []
        i = 0
        while i < len(line):
            if in_block:
                end = line.find("*/", i)
                if end < 0:
                    i = len(line)
                else:
                    in_block = False
                    i = end + 2
            else:
                if line.startswith("/*", i):
                    in_block = True
                    i += 2
                elif line.startswith("//", i):
                    break
                else:
                    buf.append(line[i])
                    i += 1
        out_lines.append("".join(buf))
    return out_lines


def file_loc(path: Path) -> int:
    """Count non-blank, non-comment lines, skipping test files entirely and
    `#[cfg(test)] mod` blocks inline. Edges from those modules still count —
    we just don't weight LOC by them."""
    try:
        if _is_test_file(path):
            return 0
        text = path.read_text()
    except OSError:
        return 0

    lines = _strip_comments(text)
    count = 0
    i = 0
    while i < len(lines):
        stripped = lines[i].strip()
        if stripped.startswith("#[cfg(test)]"):
            # Look ahead for `mod ... {` (could be on the same or next non-blank line).
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            if j < len(lines) and lines[j].lstrip().startswith("mod "):
                # Find the opening brace, then skip to matching close.
                k = j
                while k < len(lines) and "{" not in lines[k]:
                    k += 1
                if k < len(lines):
                    depth = lines[k].count("{") - lines[k].count("}")
                    k += 1
                    while k < len(lines) and depth > 0:
                        depth += lines[k].count("{") - lines[k].count("}")
                        k += 1
                    i = k
                    continue
        if stripped:
            count += 1
        i += 1
    return count


def own_file_loc(module: str, src_root: Path) -> int:
    """LOC of just this module's own backing file (no descendants)."""
    f = module_to_file(module, src_root)
    return file_loc(f) if f is not None else 0


def size_charge(own_loc: int, gamma: float, pivot: float) -> float:
    """Soft log-shaped penalty per file: γ·L·log(1 + L/T)."""
    if own_loc <= 0 or gamma <= 0.0 or pivot <= 0.0:
        return 0.0
    return gamma * own_loc * math.log(1.0 + own_loc / pivot)


def subtree_loc(module: str, modules: set[str], src_root: Path) -> int:
    prefix = module + "::"
    total = 0
    f = module_to_file(module, src_root)
    if f is not None:
        total += file_loc(f)
    for m in modules:
        if m.startswith(prefix):
            f = module_to_file(m, src_root)
            if f is not None:
                total += file_loc(f)
    return total


def score_partition(
    edges: list[tuple[str, str]],
    partition: dict[str, list[str]],
    alpha: float,
    exact_threshold: int,
) -> tuple[float, int, int]:
    """Returns (index, cross_edges, feedback_weight)."""
    matrix, cross, _ = build_matrix(edges, partition)
    groups = list(partition.keys())
    if not groups:
        return 0.0, 0, 0
    if len(groups) <= exact_threshold:
        _, fb = best_order_exact(groups, matrix)
    else:
        _, fb = best_order_greedy(groups, matrix)
    return cross + alpha * fb, cross, fb


def fractal_report(
    edges: list[tuple[str, str]],
    root: str,
    src_root: Path,
    alpha: float,
    beta: float,
    beta_children_pivot: float,
    gamma: float,
    pivot: float,
    exact_threshold: int,
) -> tuple[float, float, float]:
    """Walk the subtree, print the per-module report, and return
    (per_root_loc, structure_per_loc, size_per_loc)."""
    modules = discover_modules(edges)

    structure_sum = 0.0
    size_sum = 0.0
    nonleaf_loc_sum = 0
    root_loc = subtree_loc(root, modules, src_root)

    def walk(module: str, depth: int) -> None:
        nonlocal structure_sum, size_sum, nonleaf_loc_sum
        children = direct_children(module, modules)
        loc = subtree_loc(module, modules, src_root)
        own_loc = own_file_loc(module, src_root)
        size = size_charge(own_loc, gamma, pivot)
        size_sum += size
        size_str = f"   own {own_loc:>4}  size {size:>6.1f}" if own_loc else ""
        if not children:
            print(f"{'  ' * depth}{module:<60} loc {loc:>6}   leaf{size_str}")
            return
        partition = {c: [f"{module}::{c}"] for c in children}
        raw_index, cross, fb = score_partition(edges, partition, alpha, exact_threshold)
        beta_scale = max(1.0, beta_children_pivot / len(children)) if beta_children_pivot > 0 else 1.0
        index = raw_index + beta * beta_scale
        structure_sum += index * loc
        nonleaf_loc_sum += loc
        print(f"{'  ' * depth}{module:<60} loc {loc:>6}   "
              f"children {len(children)}   cross {cross}   fb {fb}   index {index:.1f}"
              f"{size_str}")
        for c in children:
            walk(f"{module}::{c}", depth + 1)

    walk(root, 0)
    total = structure_sum + size_sum
    per_root_loc = total / root_loc if root_loc else 0.0
    structure_per_loc = structure_sum / root_loc if root_loc else 0.0
    size_per_loc = size_sum / root_loc if root_loc else 0.0
    print()
    if root_loc:
        print(f"per root-loc (loc({root}) = {root_loc}, "
              f"γ={gamma}, T={pivot:g}):  "
              f"{per_root_loc:.2f}   "
              f"(structure {structure_per_loc:.2f}, "
              f"size {size_per_loc:.2f})")
    else:
        print(f"per root-loc: 0.00  (root loc = 0)")
    return per_root_loc, structure_per_loc, size_per_loc


BASELINE_HEADER = (
    "# columns: date  short-sha  per-loc  structure  size\n"
    "# (all three numeric columns are per root-loc)\n"
    "# managed by tools/modgraph.py --baseline; newest first, capped to 5 entries\n"
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


def update_baseline(
    path: Path,
    per_loc: float,
    structure: float,
    size: float,
) -> None:
    """Prune stale entries, prepend today's measurement, write the file, and
    print a one-line delta against the prior top entry.

    Pruning rules:
      - Drop any entry whose SHA carries a `+` suffix (dirty snapshots are
        ephemeral — superseded by the next measurement or invalidated by a
        working-tree reset).
      - Drop any entry whose SHA is no longer an ancestor of HEAD (covers
        `git checkout` to a different branch, `git reset --hard` past the
        commit, and rebase drops).
    """
    sha = _git_short_sha() or "no-git"
    sha_field = f"{sha}+" if _git_working_tree_dirty() else sha
    today = datetime.date.today().isoformat()

    raw_lines: list[str] = []
    if path.exists():
        for line in path.read_text().splitlines():
            stripped = line.strip()
            if stripped and not stripped.startswith("#"):
                raw_lines.append(stripped)

    kept: list[str] = []
    for line in raw_lines:
        parsed = _parse_baseline_line(line)
        if parsed is None:
            continue
        _, entry_sha, _ = parsed
        if entry_sha.endswith("+"):
            continue
        if not _git_is_ancestor(entry_sha):
            continue
        kept.append(line)

    prior = _parse_baseline_line(kept[0]) if kept else None
    new_entry = f"{today} {sha_field} {per_loc:.2f} {structure:.2f} {size:.2f}"
    kept.insert(0, new_entry)
    kept = kept[:BASELINE_LIMIT]

    path.write_text(BASELINE_HEADER + "\n".join(kept) + "\n")

    if prior is None:
        print(f"\nbaseline: per-loc {per_loc:.2f} — first run "
              f"(recorded to {path}).")
    else:
        prior_date, prior_sha, prior_per_loc = prior
        delta = per_loc - prior_per_loc
        print(f"\nbaseline: per-loc {per_loc:.2f} vs prior {prior_per_loc:.2f} "
              f"from {prior_date} {prior_sha} (Δ {delta:+.2f}, recorded to {path}).")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--edges", required=True, type=Path, help="cargo-modules DOT output")
    ap.add_argument("--root", required=True, metavar="MODULE",
                    help="root module to score recursively (e.g. koan, koan::runtime::machine)")
    ap.add_argument("--src-root", type=Path, default=Path("src"),
                    help="source root for LOC lookup (default: src)")
    ap.add_argument("--alpha", type=float, default=2.0, help="feedback penalty (default 2.0)")
    ap.add_argument("--beta", type=float, default=20.0,
                    help="per-non-leaf charge; "
                         "penalises passthrough wrappers and tree depth (default 20.0)")
    ap.add_argument("--beta-children-pivot", type=float, default=3.0,
                    help="if >0, scale β by max(1, P/children) so wrappers with fewer "
                         "than P direct children pay amplified β (thin pass-throughs); "
                         "0 disables, leaving β flat (default 3)")
    ap.add_argument("--gamma", type=float, default=10.0,
                    help="per-file size charge weight; "
                         "size(m) = γ·own_loc·log(1+own_loc/T) (default 10.0)")
    ap.add_argument("--size-pivot", type=float, default=200.0,
                    help="LOC pivot T in the size charge; files much smaller than T "
                         "are near-free, files much larger turn super-linear (default 200)")
    ap.add_argument("--exact-threshold", type=int, default=6,
                    help="use exact search for N <= this many groups (default 6)")
    ap.add_argument("--baseline", type=Path, metavar="FILE",
                    help="prune stale entries (unreachable SHAs and prior dirty "
                         "snapshots), prepend today's measurement, trim to 5, and "
                         "write the file; prints a delta line against the prior top "
                         "entry (e.g. --baseline tools/complexity.txt)")
    args = ap.parse_args()

    edges = load_edges(args.edges)
    per_loc, structure, size = fractal_report(
        edges, args.root, args.src_root,
        args.alpha, args.beta, args.beta_children_pivot, args.gamma, args.size_pivot,
        args.exact_threshold,
    )
    if args.baseline is not None:
        update_baseline(args.baseline, per_loc, structure, size)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
