"""The fractal complexity score: coupling + nesting + size over a subtree.

`score_tree` walks the module tree from a root, scoring each interior node's
child partition (cross edges, feedback against the best topological order, and
provider-side fan-out) and charging per-wrapper nesting and per-file size. The
three components compose by addition and are reported over a fixed denominator.
See the package docstring for the calibration rationale.
"""
from __future__ import annotations

import dataclasses
import itertools
from collections import defaultdict
from pathlib import Path

from loc import (
    build_prose_attribution,
    own_file_loc,
    own_file_loc_raw,
    owner_credit,
    size_charge,
    subtree_loc,
)
from modules import direct_children, discover_modules, module_to_file


@dataclasses.dataclass(frozen=True)
class Score:
    """The three per-loc components: coupling (cross + α·fb at each
    wrapper, loc-weighted), nesting (β·scale at each wrapper, loc-weighted),
    and size (γ·L·log per file). Sums over a subtree compose by addition;
    `per(denominator)` produces the reported fixed-denominator breakdown."""
    coupling: float = 0.0
    nesting: float = 0.0
    size: float = 0.0

    @property
    def total(self) -> float:
        return self.coupling + self.nesting + self.size

    def __add__(self, other: Score) -> Score:
        return Score(self.coupling + other.coupling,
                     self.nesting + other.nesting,
                     self.size + other.size)

    def per(self, loc: int) -> Score:
        if not loc:
            return Score()
        return Score(self.coupling / loc, self.nesting / loc, self.size / loc)


def classify(module: str, partition: dict[str, list[str]]) -> str | None:
    best_group, best_len = None, -1
    for group, prefixes in partition.items():
        for p in prefixes:
            if (module == p or module.startswith(p + "::")) and len(p) > best_len:
                best_group, best_len = group, len(p)
    return best_group


def build_matrix(
    edges: list[tuple[str, str]], partition: dict[str, list[str]]
) -> tuple[dict[tuple[str, str], int], int, int, dict[str, set[str]]]:
    """Edges are deduplicated by `(source_group, dst_module)` before being
    aggregated to `(source_group, dst_group)`. This counts the number of
    *distinct target modules* a source group reaches in each target group,
    not the raw cargo-modules edge sum.

    Without this dedup, splitting a file into N submodules that share
    imports multiplies coupling by N (each child redundantly points at the
    same target), penalising the split even though the semantic dependency
    is unchanged. After dedup, "type_ops depends on N distinct things in
    machine" is invariant under how type_ops is internally subdivided —
    the metric measures the subtree-level dependency surface, not the
    number of `use` sites.

    `external_entries[dst_group]` accumulates the set of distinct
    dst_modules each dst_group exposes to *other* groups in this partition
    — i.e. its external API surface. The fan-out term in `score_partition`
    uses this to reward groups with a thin external interface (the
    provider-side facade signal)."""
    matrix: dict[tuple[str, str], int] = defaultdict(int)
    cross = 0
    unclassified = 0
    seen: set[tuple[str, str]] = set()
    external_entries: dict[str, set[str]] = defaultdict(set)
    for src, dst in edges:
        sg = classify(src, partition)
        dg = classify(dst, partition)
        if sg is None or dg is None:
            unclassified += 1
            continue
        if sg != dg:
            external_entries[dg].add(dst)
            key = (sg, dst)
            if key in seen:
                continue
            seen.add(key)
            matrix[(sg, dg)] += 1
            cross += 1
    return matrix, cross, unclassified, external_entries


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


def score_partition(
    edges: list[tuple[str, str]],
    partition: dict[str, list[str]],
    alpha: float,
    lambda_facade: float,
    exact_threshold: int,
) -> tuple[float, int, int, int]:
    """Returns (index, cross_edges, feedback_weight, fan_out)."""
    matrix, cross, _, external_entries = build_matrix(edges, partition)
    groups = list(partition.keys())
    if not groups:
        return 0.0, 0, 0, 0
    if len(groups) <= exact_threshold:
        _, fb = best_order_exact(groups, matrix)
    else:
        _, fb = best_order_greedy(groups, matrix)
    fan_out = sum(max(0, len(entries) - 1) for entries in external_entries.values())
    return cross + alpha * fb + lambda_facade * fan_out, cross, fb, fan_out


def score_tree(
    edges: list[tuple[str, str]],
    root: str,
    src_root: Path,
    alpha: float,
    beta: float,
    beta_children_pivot: float,
    gamma: float,
    pivot: float,
    exact_threshold: int,
    delta: float,
    kappa: float,
    epsilon: float,
    owner_pivot: float,
    lambda_facade: float,
    prose_redirect: dict[Path, Path] | None = None,
    denominator: float = 1000.0,
    report: bool = True,
) -> Score:
    """Walk the subtree, print the per-module report, and return the
    fixed-denominator Score breakdown. With `report=False` the per-module
    breakdown is suppressed and only the final score line is printed."""
    modules = discover_modules(edges)
    root_loc = subtree_loc(root, modules, src_root)
    prose_attribution, hop_count = build_prose_attribution(src_root, prose_redirect)

    def walk(module: str, depth: int) -> Score:
        indent = "  " * depth
        children = direct_children(module, modules)
        loc = subtree_loc(module, modules, src_root)
        own_loc = own_file_loc(module, src_root)
        raw_loc = own_file_loc_raw(module, src_root)
        own_file = module_to_file(module, src_root)
        if own_file is not None:
            try:
                rel_key = own_file.resolve().relative_to(src_root.resolve())
            except ValueError:
                rel_key = None
        else:
            rel_key = None
        prose_loc = prose_attribution.get(rel_key, 0.0) if rel_key is not None else 0.0
        hops = hop_count.get(rel_key, 0) if rel_key is not None else 0
        eff_loc = raw_loc + delta * prose_loc + kappa * hops
        gross_size = size_charge(eff_loc, gamma, pivot)
        credit = owner_credit(prose_loc, epsilon, owner_pivot)
        size = max(0.0, gross_size - credit)
        size_tail = (
            f"   own {own_loc:>4} (raw {raw_loc:>4}, eff {eff_loc:>6.1f})  size {size:>6.1f}"
            + (f" (−{credit:.1f} owner)" if credit > 0 else "")
            if (own_loc or raw_loc) else ""
        )
        head = f"{indent}{module:<60} loc {loc:>6}"

        if not children:
            if report:
                print(f"{head}   leaf{size_tail}")
            return Score(size=size)

        partition = {c: [f"{module}::{c}"] for c in children}
        coupling, cross, fb, fan = score_partition(
            edges, partition, alpha, lambda_facade, exact_threshold)
        beta_scale = max(1.0, beta_children_pivot / len(children)) if beta_children_pivot > 0 else 1.0
        nest = beta * beta_scale
        if report:
            print(f"{head}   children {len(children)}   cross {cross}   fb {fb}   fan {fan}"
                  f"   nest {nest:.1f}   index {coupling + nest:.1f}{size_tail}")

        here = Score(coupling=coupling * loc, nesting=nest * loc, size=size)
        return sum((walk(f"{module}::{c}", depth + 1) for c in children), here)

    totals = walk(root, 0)
    per = totals.per(denominator)
    if report:
        print()
    print(f"score (denominator {denominator:g}, loc({root}) = {root_loc}, "
          f"γ={gamma}, T={pivot:g}):  {per.total:.2f}   "
          f"(coupling {per.coupling:.2f}, nesting {per.nesting:.2f}, size {per.size:.2f})")
    return per
