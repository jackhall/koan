#!/usr/bin/env python3
"""Measure a complexity index for a module partition.

Reads a cargo-modules DOT file and a TOML partition spec, then reports:
  index(m) = cross_edges(m) + alpha * feedback_weight(m) + beta
  total    = Σ index(m) · loc(m)        (summed over non-leaf m)
  per-loc  = total / loc(root)          (under --fractal)

where feedback_weight is the total weight of edges that go against the
best topological ordering of the groups. For N <= 6 groups the best order
is found by exhaustive search; above that, the Eades-Lin-Smyth GR
heuristic is used.

The `--fractal` per-loc number is normalised by the root subtree's LOC
(a fixed constant for a given crate). This is what makes nesting cost
something: every interior level contributes its own loc to the sum, so
adding a wrapper around a heavy subtree adds (β + cross + α·fb) · loc.

`beta` is a flat per-non-leaf charge. The default of 5.0 says "one extra
module layer costs roughly what 5 cross edges cost" — calibrated by
sweeping β across known structures (gratuitous wrappers, useful
wrappers, nesting refactors); above ~10 the metric starts preferring
known-bad moves (e.g. dropping a load-bearing wrapper) over deep
nesting, and at β=0 a passthrough wrapper is undetectable.

Usage:
  python3 tools/modgraph.py --edges <dot-file> --partition <toml-file>
                            [--alpha 2.0] [--beta 5.0]

Partition TOML:
  [groups]
  parse    = ["koan::parse"]
  model    = ["koan::dispatch::types", "koan::dispatch::values"]
  machine  = ["koan::dispatch::kfunction", "koan::dispatch::runtime",
              "koan::execute"]
  builtins = ["koan::builtins"]
"""

from __future__ import annotations

import argparse
import itertools
import re
import sys
import tomllib
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


def load_partition(path: Path) -> dict[str, list[str]]:
    data = tomllib.loads(path.read_text())
    groups = data.get("groups")
    if not isinstance(groups, dict) or not groups:
        sys.exit(f"{path}: missing or empty [groups] table")
    return {name: list(prefixes) for name, prefixes in groups.items()}


def partition_from_children(
    edges: list[tuple[str, str]], parent: str
) -> dict[str, list[str]]:
    """One group per direct child of `parent`, named by the child's last segment."""
    prefix = parent + "::"
    children: set[str] = set()
    for src, dst in edges:
        for mod in (src, dst):
            if mod.startswith(prefix):
                tail = mod[len(prefix):].split("::", 1)[0]
                children.add(tail)
    if not children:
        sys.exit(f"--children-of {parent}: no descendants found in edges")
    return {child: [f"{parent}::{child}"] for child in sorted(children)}


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
            for g in list(remaining):
                if out_weight(g) == 0:
                    s2.insert(0, g)
                    remaining.remove(g)
                    progress = True
            for g in list(remaining):
                if in_weight(g) == 0:
                    s1.append(g)
                    remaining.remove(g)
                    progress = True
        if not remaining:
            break
        pick = max(remaining, key=lambda g: out_weight(g) - in_weight(g))
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
    exact_threshold: int,
) -> int:
    modules = discover_modules(edges)

    weighted_sum = 0.0
    nonleaf_loc_sum = 0
    root_loc = subtree_loc(root, modules, src_root)

    def walk(module: str, depth: int) -> None:
        nonlocal weighted_sum, nonleaf_loc_sum
        children = direct_children(module, modules)
        loc = subtree_loc(module, modules, src_root)
        if not children:
            print(f"{'  ' * depth}{module:<60} loc {loc:>6}   leaf")
            return
        partition = {c: [f"{module}::{c}"] for c in children}
        raw_index, cross, fb = score_partition(edges, partition, alpha, exact_threshold)
        index = raw_index + beta
        weighted_sum += index * loc
        nonleaf_loc_sum += loc
        print(f"{'  ' * depth}{module:<60} loc {loc:>6}   "
              f"children {len(children)}   cross {cross}   fb {fb}   index {index:.1f}")
        for c in children:
            walk(f"{module}::{c}", depth + 1)

    walk(root, 0)
    print()
    print(f"total Σ index·loc:                          {weighted_sum:.0f}")
    if root_loc:
        per_root_loc = weighted_sum / root_loc
        print(f"per root-loc (Σ index·loc / loc({root})):  {per_root_loc:.2f}")
    if nonleaf_loc_sum:
        avg = weighted_sum / nonleaf_loc_sum
        print(f"per nonleaf-loc (legacy avg):               {avg:.2f}")
    return 0


def render_matrix(order: list[str], matrix: dict[tuple[str, str], int]) -> str:
    width = max(len(g) for g in order)
    lines = []
    header = " " * (width + 4) + "  ".join(f"{g:>{width}}" for g in order)
    lines.append(header)
    for src in order:
        row = [f"{matrix.get((src, dst), 0):>{width}}" for dst in order]
        lines.append(f"  {src:>{width}}  " + "  ".join(row))
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--edges", required=True, type=Path, help="cargo-modules DOT output")
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--partition", type=Path, help="partition TOML")
    src.add_argument("--children-of", dest="children_of", metavar="MODULE",
                     help="auto-partition into one group per direct sub-module of MODULE "
                          "(e.g. --children-of koan::dispatch)")
    src.add_argument("--fractal", metavar="MODULE",
                     help="recursively score every parent module under MODULE; "
                          "aggregate weighted by lines of code")
    ap.add_argument("--src-root", type=Path, default=Path("src"),
                    help="source root for LOC lookup (default: src)")
    ap.add_argument("--alpha", type=float, default=2.0, help="feedback penalty (default 2.0)")
    ap.add_argument("--beta", type=float, default=5.0,
                    help="per-non-leaf charge under --fractal; "
                         "penalises passthrough wrappers and tree depth (default 5.0)")
    ap.add_argument("--exact-threshold", type=int, default=6,
                    help="use exact search for N <= this many groups (default 6)")
    args = ap.parse_args()

    edges = load_edges(args.edges)
    if args.fractal:
        return fractal_report(edges, args.fractal, args.src_root,
                              args.alpha, args.beta, args.exact_threshold)
    if args.partition:
        partition = load_partition(args.partition)
    else:
        partition = partition_from_children(edges, args.children_of)
    groups = list(partition.keys())

    matrix, cross, unclassified = build_matrix(edges, partition)

    if len(groups) <= args.exact_threshold:
        order, fb = best_order_exact(groups, matrix)
        method = f"exact (N={len(groups)})"
    else:
        order, fb = best_order_greedy(groups, matrix)
        method = f"greedy GR (N={len(groups)})"

    index = cross + args.alpha * fb

    print(f"groups: {len(groups)}   cross edges: {cross}   "
          f"feedback: {fb}   index: {index:.1f}   alpha: {args.alpha}")
    print(f"method: {method}")
    print(f"best order: {' -> '.join(order)}")
    if unclassified:
        print(f"unclassified edges (skipped): {unclassified}")
    print()

    rank = {g: i for i, g in enumerate(order)}
    back = [(a, b, w) for (a, b), w in matrix.items() if rank[a] > rank[b]]
    if back:
        print(f"back edges ({sum(w for _, _, w in back)}):")
        for a, b, w in sorted(back, key=lambda t: -t[2]):
            print(f"  {a} -> {b}: {w}")
        print()

    print("matrix (rows = src group, cols = dst group):")
    print(render_matrix(order, matrix))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
