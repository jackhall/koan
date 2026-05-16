#!/usr/bin/env python3
"""Measure a fractal complexity index for a module subtree.

Reads a cargo-modules DOT file, recursively walks the module tree from
`--root`, and reports per node:
  index(m)   = cross_edges(m) + alpha * feedback_weight(m) + beta
  size(m)    = gamma * own_loc(m) * log(1 + own_loc(m) / pivot)
plus the aggregated total:
  total      = Σ index(m) · loc(m)   +   Σ size(m)
                  (structure)              (per-file size)
  per-loc    = total / loc(root)

`cross_edges` and `feedback_weight` are computed at every interior node
against the one-group-per-child partition of its children. For N ≤ 6
children the best topological order is found by exhaustive search;
above that, the Eades-Lin-Smyth GR heuristic is used.

The per-loc number is normalised by the root subtree's LOC (a fixed
constant for a given root). This is what makes nesting cost something:
every interior level contributes its own loc to the sum, so adding a
wrapper around a heavy subtree adds (β + cross + α·fb) · loc.

`beta` is a flat per-non-leaf charge. The default of 5.0 says "one extra
module layer costs roughly what 5 cross edges cost" — calibrated by
sweeping β across known structures (gratuitous wrappers, useful
wrappers, nesting refactors); above ~10 the metric starts preferring
known-bad moves (e.g. dropping a load-bearing wrapper) over deep
nesting, and at β=0 a passthrough wrapper is undetectable.

`gamma`/`pivot` shape the per-file size charge. Without it, a single
3000-line leaf scores zero while any split incurs structural cost, so
the metric strictly rewards inaction. The charge `γ · L · log(1 + L/T)`
is sub-linear in L for L ≪ T (small files are nearly free) and turns
super-linear as L ≫ T. Defaults (γ=10, T=400) put a 400-line file at
~2773 and a 1000-line file at ~12530 — roughly the cost of an extra
wrapper layer around an 800-LOC subtree. Applied to every module's own
file (leaves and parents alike), so fat `mod.rs` files above small
children are also penalised.

Usage:
  python3 tools/modgraph.py --edges <dot-file> --root koan
  python3 tools/modgraph.py --edges <dot-file> --root koan::runtime::machine
"""

from __future__ import annotations

import argparse
import itertools
import math
import re
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
    gamma: float,
    pivot: float,
    exact_threshold: int,
) -> int:
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
        index = raw_index + beta
        structure_sum += index * loc
        nonleaf_loc_sum += loc
        print(f"{'  ' * depth}{module:<60} loc {loc:>6}   "
              f"children {len(children)}   cross {cross}   fb {fb}   index {index:.1f}"
              f"{size_str}")
        for c in children:
            walk(f"{module}::{c}", depth + 1)

    walk(root, 0)
    total = structure_sum + size_sum
    print()
    print(f"structure Σ index·loc:                      {structure_sum:>12.0f}")
    print(f"size      Σ γ·L·log(1+L/T):                 {size_sum:>12.0f}   "
          f"(γ={gamma}, T={pivot:g})")
    print(f"total:                                      {total:>12.0f}")
    if root_loc:
        print(f"per root-loc (total / loc({root})):        "
              f"{total / root_loc:>12.2f}   "
              f"(structure {structure_sum / root_loc:.2f}, "
              f"size {size_sum / root_loc:.2f})")
    if nonleaf_loc_sum:
        avg = structure_sum / nonleaf_loc_sum
        print(f"structure per nonleaf-loc (legacy avg):     {avg:>12.2f}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--edges", required=True, type=Path, help="cargo-modules DOT output")
    ap.add_argument("--root", required=True, metavar="MODULE",
                    help="root module to score recursively (e.g. koan, koan::runtime::machine)")
    ap.add_argument("--src-root", type=Path, default=Path("src"),
                    help="source root for LOC lookup (default: src)")
    ap.add_argument("--alpha", type=float, default=2.0, help="feedback penalty (default 2.0)")
    ap.add_argument("--beta", type=float, default=5.0,
                    help="per-non-leaf charge; "
                         "penalises passthrough wrappers and tree depth (default 5.0)")
    ap.add_argument("--gamma", type=float, default=10.0,
                    help="per-file size charge weight; "
                         "size(m) = γ·own_loc·log(1+own_loc/T) (default 10.0)")
    ap.add_argument("--size-pivot", type=float, default=400.0,
                    help="LOC pivot T in the size charge; files much smaller than T "
                         "are near-free, files much larger turn super-linear (default 400)")
    ap.add_argument("--exact-threshold", type=int, default=6,
                    help="use exact search for N <= this many groups (default 6)")
    args = ap.parse_args()

    edges = load_edges(args.edges)
    return fractal_report(edges, args.root, args.src_root,
                          args.alpha, args.beta, args.gamma, args.size_pivot,
                          args.exact_threshold)


if __name__ == "__main__":
    raise SystemExit(main())
