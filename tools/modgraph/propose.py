"""`modgraph propose` — surface co-location group candidates.

`rewrite item` scores a hand-picked group; this proposes them. It builds the
item-level directed graph from a SCIP index (def spans + body refs, reusing the
same brace scanner the rewriter uses), then generates two candidate kinds:

  * CYCLE   — non-trivial strongly-connected components straddling >=2 modules
              (the α·feedback carriers; a cycle you cannot layer away).
  * DENSITY — dense clusters from a greedy-modularity pass over the undirected
              projection, straddling >=2 modules (the cross carriers).

Each candidate is scored by the existing co-locate what-if (its members into one
synthetic module) and reported with the predicted Δscore against the current
baseline, ranked most-negative first. `propose` only ranks; the human picks the
cut — it never auto-applies a move. Cost is bounded by min/max group size, a
top-N scoring cap, and a minimum-improvement display filter; every drop is
logged so a truncated list never reads as exhaustive.
"""
from __future__ import annotations

import contextlib
import dataclasses
import io
from collections import defaultdict
from pathlib import Path
from tempfile import TemporaryDirectory

from graph import load_uses
from modules import relpath_to_module
from rewrite import (MoveSpec, diff_edges, find_item_end, render_dot_diff,
                     write_item_mirror)
from scip import scip_symbol_to_path
from score import Score, score_tree

# Louvain local-moving converges in a handful of passes; cap as a backstop.
_MAX_LOUVAIN_PASSES = 20


@dataclasses.dataclass
class Item:
    """A movable item node: its canonical path, the SCIP symbol that names it,
    the module and source file it lives in, and the body span [def_line,
    body_end) whose refs are its outbound edges. The span is computed once with
    `rewrite.find_item_end`, so it matches the rewriter's transplant exactly."""
    path: str
    symbol: str
    module: str
    source_file: str  # "src/..." as SCIP spells it
    def_line: int
    body_end: int


# ====================================================================
# item-level graph construction
# ====================================================================

def build_item_graph(
    docs: dict[str, dict], src_root: Path, root: str, known: set[str]
) -> tuple[dict[str, Item], dict[str, set[str]], int]:
    """Build the item digraph scoped to `root` from a parsed SCIP index.

    Nodes are koan items defined in src files whose module is `root` or under
    `root::`; an edge A -> B means a reference to item B occurs inside A's body
    span. Returns (items, adjacency, ambiguous_count).

    A path defined in more than one file is *ambiguous* — the item rewriter
    refuses to move it — so it is dropped from the graph entirely (neither node
    nor edge endpoint). Module-level refs (a top-level `use`, enclosed by no
    item), self-references, external-crate refs, and edges to non-recorded items
    are all dropped.

    Module *declarations* are excluded: a path that is itself a graph node in
    `known` names a `mod`, not a movable item, and its body span covers the whole
    file, so every inner reference would otherwise collapse onto the module and
    hairball the graph.

    Test code is excluded: the scored DOT carries no `tests` nodes (the scorer
    ignores `#[cfg(test)]`), so a test item is invisible to the what-if and would
    only inflate the graph with cycles no refactor can act on. koan keeps all
    test code under a `tests` module segment (`foo/tests.rs` and inline
    `#[cfg(test)] mod tests`), which the canonical SCIP path carries.
    """
    in_root = lambda mod: mod == root or mod.startswith(root + "::")

    lines_cache: dict[str, list[str]] = {}

    def lines_of(relpath: str) -> list[str]:
        if relpath not in lines_cache:
            rel = relpath[len("src/"):] if relpath.startswith("src/") else relpath
            path = src_root / rel
            lines_cache[relpath] = (
                path.read_text(encoding="utf-8", errors="ignore").splitlines()
                if path.exists() else []
            )
        return lines_cache[relpath]

    # First pass: record every def's span and detect path ambiguity. Spans are
    # kept per document so the second pass can attribute refs to enclosing items.
    spans_by_doc: dict[str, list[tuple[str, str, int, int]]] = {}
    path_files: dict[str, set[str]] = defaultdict(set)
    for relpath, doc in docs.items():
        if not relpath.startswith("src/") or not relpath.endswith(".rs"):
            continue
        if relpath_to_module(relpath) is None:
            continue
        lines = lines_of(relpath)
        recs: list[tuple[str, str, int, int]] = []
        for sym, line in doc["defs"]:
            path = scip_symbol_to_path(sym)
            if path is None or path in known or _is_test(path):
                continue
            end = find_item_end(lines, line) if line < len(lines) else line + 1
            recs.append((path, sym, line, end))
            path_files[path].add(relpath)
        spans_by_doc[relpath] = recs

    ambiguous = {p for p, files in path_files.items() if len(files) > 1}

    items: dict[str, Item] = {}
    for relpath, recs in spans_by_doc.items():
        mod = relpath_to_module(relpath)
        if mod is None or not in_root(mod):
            continue
        for path, sym, line, end in recs:
            if path in ambiguous:
                continue
            items[path] = Item(path=path, symbol=sym, module=mod,
                               source_file=relpath, def_line=line, body_end=end)

    # Second pass: each body ref to a recorded item becomes an outbound edge,
    # attributed to the innermost recorded item whose span contains the ref line.
    adj: dict[str, set[str]] = defaultdict(set)
    for relpath, recs in spans_by_doc.items():
        own_recs = sorted((r for r in recs if r[0] in items), key=lambda r: r[2])
        if not own_recs:
            continue
        for sym, line in docs[relpath]["refs"]:
            target = scip_symbol_to_path(sym)
            if target is None or target not in items:
                continue
            owner = _innermost_owner(own_recs, line)
            if owner is None or owner == target:
                continue
            adj[owner].add(target)

    return items, adj, len(ambiguous)


def _is_test(path: str) -> bool:
    """True for items under a `tests` module segment — koan's universal home for
    `#[cfg(test)]` code, which the scored DOT never represents."""
    return "tests" in path.split("::")


def _innermost_owner(
    recs_sorted: list[tuple[str, str, int, int]], line: int
) -> str | None:
    """The path of the innermost recorded item whose span contains `line`.
    `recs_sorted` is ascending by def_line, so the last qualifying record is the
    most deeply nested; once a def starts past `line` no later record can."""
    owner = None
    for path, _sym, def_line, end in recs_sorted:
        if def_line > line:
            break
        if line < end:
            owner = path
    return owner


# ====================================================================
# strongly-connected components — Tarjan (iterative)
# ====================================================================

def tarjan_scc(nodes: set[str], adj: dict[str, set[str]]) -> list[list[str]]:
    """All strongly-connected components, via an explicit-stack Tarjan so a deep
    dependency chain cannot exhaust Python's recursion limit. Deterministic:
    roots and adjacency are walked in sorted order."""
    index: dict[str, int] = {}
    lowlink: dict[str, int] = {}
    on_stack: dict[str, bool] = {}
    stack: list[str] = []
    counter = 0
    result: list[list[str]] = []

    for root in sorted(nodes):
        if root in index:
            continue
        work: list[tuple[str, list[str], int]] = [
            (root, sorted(adj.get(root, ())), 0)
        ]
        index[root] = lowlink[root] = counter
        counter += 1
        stack.append(root)
        on_stack[root] = True
        while work:
            node, succ, i = work[-1]
            recursed = False
            while i < len(succ):
                w = succ[i]
                i += 1
                if w not in index:
                    work[-1] = (node, succ, i)
                    index[w] = lowlink[w] = counter
                    counter += 1
                    stack.append(w)
                    on_stack[w] = True
                    work.append((w, sorted(adj.get(w, ())), 0))
                    recursed = True
                    break
                if on_stack.get(w):
                    lowlink[node] = min(lowlink[node], index[w])
            if recursed:
                continue
            if lowlink[node] == index[node]:
                comp: list[str] = []
                while True:
                    x = stack.pop()
                    on_stack[x] = False
                    comp.append(x)
                    if x == node:
                        break
                result.append(comp)
            work.pop()
            if work:
                parent = work[-1][0]
                lowlink[parent] = min(lowlink[parent], lowlink[node])
    return result


# ====================================================================
# density clusters — greedy modularity (Louvain local-moving)
# ====================================================================

def undirected_projection(adj: dict[str, set[str]]) -> dict[str, dict[str, int]]:
    """Symmetric weighted projection of the directed item graph: each directed
    edge contributes 1 to the weight of its endpoint pair in both directions, so
    a reciprocated A<->B coupling weighs 2 (denser than a one-way reference)."""
    und: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for a, outs in adj.items():
        for b in outs:
            if a == b:
                continue
            und[a][b] += 1
            und[b][a] += 1
    return und


def louvain_communities(und: dict[str, dict[str, int]]) -> list[set[str]]:
    """One level of Louvain local-moving modularity optimization over the
    undirected projection. Deterministic (nodes and neighbour communities are
    visited in sorted order; a move is taken only on a strict gain). Nodes with
    no item-edges are absent from `und` and so join no community — they cannot be
    a dense cluster."""
    nodes = sorted(und)
    if not nodes:
        return []
    degree = {v: sum(und[v].values()) for v in nodes}
    m = sum(degree.values()) / 2.0
    if m == 0:
        return [{v} for v in nodes]
    two_m = 2.0 * m

    node_comm = {v: v for v in nodes}
    comm_tot = {v: degree[v] for v in nodes}  # Σ degree of community members

    for _ in range(_MAX_LOUVAIN_PASSES):
        moved = False
        for v in nodes:
            cv = node_comm[v]
            ki = degree[v]
            neigh_w: dict[str, float] = defaultdict(float)
            for u, w in und[v].items():
                neigh_w[node_comm[u]] += w
            comm_tot[cv] -= ki  # detach v before scoring re-insertion
            best_comm = cv
            best_gain = neigh_w.get(cv, 0.0) - ki * comm_tot[cv] / two_m
            for c in sorted(neigh_w):
                gain = neigh_w[c] - ki * comm_tot[c] / two_m
                if gain > best_gain + 1e-12:
                    best_gain, best_comm = gain, c
            comm_tot[best_comm] += ki
            if best_comm != cv:
                node_comm[v] = best_comm
                moved = True
        if not moved:
            break

    groups: dict[str, set[str]] = defaultdict(set)
    for v, c in node_comm.items():
        groups[c].add(v)
    return list(groups.values())


# ====================================================================
# co-locate what-if scoring
# ====================================================================

def common_ancestor(modules: set[str]) -> str:
    """Deepest module path that is a prefix of every member module — where the
    co-located group is placed. A straddle spans >=2 modules, so this is a
    proper ancestor; it falls back to the crate root if they share none."""
    parts_lists = [m.split("::") for m in sorted(modules)]
    common: list[str] = []
    for segs in zip(*parts_lists):
        if len(set(segs)) == 1:
            common.append(segs[0])
        else:
            break
    return "::".join(common) if common else "koan"


def colocate_module(members: list[str], items: dict[str, Item]) -> str:
    anc = common_ancestor({items[p].module for p in members})
    return f"{anc}::colocated"


def score_colocation(
    members: list[str],
    items: dict[str, Item],
    docs: dict[str, dict],
    attribution: dict[str, list[tuple[str, str]]],
    known: set[str],
    original_dot: str,
    src_root: Path,
    score_fn,
) -> float:
    """Total score after co-locating `members` into one synthetic module, via the
    exact `rewrite item` pipeline (so the Δ matches `rewrite item` + `score`).
    Body spans are reused from the item graph, so no re-scan is needed."""
    new_mod = colocate_module(members, items)
    moves: list[MoveSpec] = []
    for path in members:
        it = items[path]
        mv = MoveSpec(old_path=path, new_module=new_mod)
        mv.symbol = it.symbol
        mv.source_module = it.module
        mv.source_file = it.source_file
        mv.def_line = it.def_line
        mv.body_end = it.body_end
        moves.append(mv)

    add, remove = diff_edges(docs, moves, attribution, known)
    with TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        dot_path = tmp_path / "candidate.dot"
        dot_path.write_text(
            render_dot_diff(original_dot, add, remove, {new_mod}), encoding="utf-8")
        mirror = tmp_path / "src"
        write_item_mirror(src_root, mirror, moves)
        return score_fn(load_uses(dot_path), mirror).total


def _make_score_fn(root: str, params: dict):
    """A stdout-suppressing closure over the scoring scalars — `score_tree`
    always prints its per-module report, which would bury the triage output."""
    def fn(edges, src_root: Path) -> Score:
        with contextlib.redirect_stdout(io.StringIO()):
            return score_tree(
                edges, root, src_root,
                params["alpha"], params["beta"], params["beta_children_pivot"],
                params["gamma"], params["pivot"], params["exact_threshold"],
                params["delta"], params["kappa"], params["epsilon"],
                params["owner_pivot"], params["lambda_facade"],
                None, params["denominator"],
            )
    return fn


# ====================================================================
# orchestration + output
# ====================================================================

@dataclasses.dataclass
class Candidate:
    kind: str               # "CYCLE" | "DENSITY"
    members: list[str]
    modules: set[str]
    delta: float = 0.0


def _cross_module_edges(members: set[str], items: dict[str, Item],
                        adj: dict[str, set[str]]) -> int:
    """Intra-group directed edges that cross a module boundary — the coupling
    co-location relieves. Used as the cheap pre-score rank for the top-N cap."""
    n = 0
    for a in members:
        ma = items[a].module
        for b in adj.get(a, ()):
            if b in members and items[b].module != ma:
                n += 1
    return n


def generate_candidates(
    items: dict[str, Item], adj: dict[str, set[str]], min_group_size: int
) -> list[Candidate]:
    """SCC (CYCLE) then modularity (DENSITY) candidates straddling >=2 modules,
    deduped by member set with CYCLE winning an exact tie."""
    out: list[Candidate] = []
    seen: set[frozenset[str]] = set()

    def consider(kind: str, members: list[str]) -> None:
        if len(members) < min_group_size:
            return
        modules = {items[p].module for p in members}
        if len(modules) < 2:
            return
        key = frozenset(members)
        if key in seen:
            return
        seen.add(key)
        out.append(Candidate(kind, sorted(members), modules))

    for comp in tarjan_scc(set(items), adj):
        consider("CYCLE", comp)
    for community in louvain_communities(undirected_projection(adj)):
        consider("DENSITY", [p for p in community if p in items])
    return out


def run(
    docs: dict[str, dict],
    *,
    root: str,
    src_root: Path,
    edges_path: Path,
    known: set[str],
    attribution: dict[str, list[tuple[str, str]]],
    score_params: dict,
    min_group_size: int,
    max_group_size: int,
    top_n: int,
    min_delta: float,
) -> int:
    items, adj, ambiguous = build_item_graph(docs, src_root, root, known)
    print(f"item graph: {len(items)} items, "
          f"{sum(len(s) for s in adj.values())} edges under {root}"
          + (f" ({ambiguous} ambiguous paths dropped)" if ambiguous else ""))

    candidates = generate_candidates(items, adj, min_group_size)
    n_generated = len(candidates)

    sized = [c for c in candidates if len(c.members) <= max_group_size]
    dropped_oversize = n_generated - len(sized)

    # Score the smallest groups first within each kind: co-location cost grows
    # with group size, so the improving cuts are the small ones — and they are
    # the cheapest to score (cross-edge count breaks size ties so a tight
    # straddle outranks a loose pair). The top-N budget is split so both kinds
    # are surfaced (criteria 2 & 3): density is guaranteed at least half the
    # slots (it holds the small improvers), cycles take the rest (the α carriers,
    # usually few).
    rank = lambda c: (len(c.members),
                      -_cross_module_edges(set(c.members), items, adj))
    cyc = sorted((c for c in sized if c.kind == "CYCLE"), key=rank)
    den = sorted((c for c in sized if c.kind == "DENSITY"), key=rank)
    den_slots = min(len(den), max(1, top_n // 2))
    cyc_take = cyc[: top_n - den_slots]
    den_take = den[: top_n - len(cyc_take)]
    to_score = cyc_take + den_take
    dropped_topn = len(sized) - len(to_score)

    score_fn = _make_score_fn(root, score_params)
    original_dot = edges_path.read_text(encoding="utf-8")
    baseline = score_fn(load_uses(edges_path), src_root).total
    for c in to_score:
        c.delta = score_colocation(c.members, items, docs, attribution, known,
                                   original_dot, src_root, score_fn) - baseline

    # min-|Δ| drops candidates whose predicted change is negligible; both strong
    # improvements (Δ<0, act on these) and strong regressions (Δ>0, the design
    # doc's "foundation" — co-location costs more, leave it) survive and are
    # ranked most-negative first.
    shown = [c for c in to_score if abs(c.delta) >= min_delta]
    dropped_delta = len(to_score) - len(shown)
    shown.sort(key=lambda c: (c.delta, -len(c.members)))

    _print_triage(shown, baseline, root)
    print()
    improving = sum(1 for c in shown if c.delta < 0)
    print(f"candidates: {n_generated} generated, {len(to_score)} scored, "
          f"{len(shown)} shown ({improving} improving).")
    drops = []
    if ambiguous:
        drops.append(f"{ambiguous} ambiguous item(s)")
    if dropped_oversize:
        drops.append(f"{dropped_oversize} over max-size {max_group_size}")
    if dropped_topn:
        drops.append(f"{dropped_topn} below top-N {top_n}")
    if dropped_delta:
        drops.append(f"{dropped_delta} below min-|Δ| {min_delta:g}")
    if drops:
        print("dropped: " + "; ".join(drops) + ".")
    return 0


def _strip_root(module: str, root: str) -> str:
    pkg = root.split("::")[0] + "::"
    return module[len(pkg):] if module.startswith(pkg) else module


def _print_triage(shown: list[Candidate], baseline: float, root: str) -> None:
    print()
    print(f"modgraph propose — co-location candidates "
          f"(root {root}, baseline {baseline:.2f})")
    print("  Δ<0 = co-locate to improve; Δ>0 = foundation (co-location regresses, leave it)")
    if not shown:
        print("  no candidates.")
        return
    for rank, c in enumerate(shown, 1):
        mods = ", ".join(_strip_root(m, root) for m in sorted(c.modules))
        names = ", ".join(_strip_root(p, root) for p in c.members)
        tag = "" if c.delta < 0 else "   (foundation)"
        print()
        print(f"  #{rank}  {c.kind:<7} Δ {c.delta:+9.2f}   "
              f"{len(c.members)} items across {len(c.modules)} modules{tag}")
        print(f"        modules: {mods}")
        print(f"        items:   {names}")
