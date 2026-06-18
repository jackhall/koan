"""Unified command line: `python3 tools/modgraph <verb>`.

Verbs:
  score    score a DOT subtree (reads the corrected observe/modules.dot)
  regen    rebuild observe/modules.dot (cargo-modules + reexport correction)
           and observe/doc_graph.dot, then score
  rewrite  what-if structural rewrites for scoring a proposed refactor:
             rewrite module  — rename whole modules
             rewrite item    — SCIP-driven item extraction
"""
from __future__ import annotations

import argparse
from pathlib import Path

import rewrite
from baseline import update_baseline
from graph import load_uses
from modules import discover_modules
from loc import subtree_loc
from regenerate import regenerate_source_data
from score import score_tree


def _add_score_args(p: argparse.ArgumentParser) -> None:
    p.add_argument("--edges", type=Path, default=Path("observe/modules.dot"),
                   help="DOT graph to score (default: observe/modules.dot)")
    p.add_argument("--root", default="koan", metavar="MODULE",
                   help="root module to score recursively (default: koan)")
    p.add_argument("--src-root", type=Path, default=Path("src"),
                   help="source root for LOC lookup (default: src)")
    p.add_argument("--alpha", type=float, default=2.0, help="feedback penalty (default 2.0)")
    p.add_argument("--lambda-facade", type=float, default=2.0,
                   help="provider-side facade penalty λ (default 2.0). At each "
                        "partition, each dst_group's external API surface "
                        "(distinct dst_modules referenced from other groups) "
                        "past the first contributes λ to coupling. A group with "
                        "one external entry — the facade ideal — pays nothing. "
                        "Set 0 to disable.")
    p.add_argument("--beta", type=float, default=20.0,
                   help="per-non-leaf charge; penalises passthrough wrappers and "
                        "tree depth (default 20.0)")
    p.add_argument("--beta-children-pivot", type=float, default=3.0,
                   help="if >0, scale β by max(1, P/children) so wrappers with fewer "
                        "than P direct children pay amplified β (thin pass-throughs); "
                        "0 disables, leaving β flat (default 3)")
    p.add_argument("--gamma", type=float, default=50.0,
                   help="per-file size charge weight; "
                        "size(m) = γ·eff_loc·log(1+eff_loc/T) (default 50.0)")
    p.add_argument("--size-pivot", type=float, default=325.0,
                   help="LOC pivot T in the size charge; files much smaller than T "
                        "are near-free, files much larger turn super-linear "
                        "(default 325)")
    p.add_argument("--delta", type=float, default=1.0,
                   help="prose-attribution weight δ in effective LOC (default 1.0). "
                        "Each design/roadmap/README markdown doc has its raw LOC "
                        "split uniformly across the src files it links to; that "
                        "share is multiplied by δ and folded into the size charge. "
                        "Set 0 to disable.")
    p.add_argument("--prose-redirect", action="append", default=[],
                   metavar="OLD=NEW",
                   help="rewrite doc-link targets in the prose-attribution pass — "
                        "`OLD` and `NEW` are paths relative to src/. Simulates doc "
                        "consolidation alongside a code-level seam. Repeatable.")
    p.add_argument("--kappa", type=float, default=10.0,
                   help="per-hop redirect cost κ in effective LOC (default 10). "
                        "Each `[...](*.md)` link a src file embeds adds κ to its "
                        "effective LOC.")
    p.add_argument("--epsilon", type=float, default=20.0,
                   help="owner-credit weight ε (default 20). Each file's attributed "
                        "prose loc earns a credit ε·L·log(1+L/P_o) subtracted from "
                        "its size charge, rewarding concentrating a documented "
                        "concept into a named owner module. Set 0 to disable.")
    p.add_argument("--owner-pivot", type=float, default=100.0,
                   help="prose-loc pivot P_o in the owner credit (default 100).")
    p.add_argument("--denominator", type=float, default=1000.0, metavar="D",
                   help="fixed divisor for the bottom-line score (default 1000). "
                        "The score is total structural+size cost / D — a constant "
                        "scale, not the tree's LOC.")
    p.add_argument("--exact-threshold", type=int, default=6,
                   help="use exact search for N <= this many groups (default 6)")
    p.add_argument("--baseline", type=Path, metavar="FILE",
                   help="prune unreachable-SHA entries, prepend today's measurement, "
                        "trim to 5, and write the file; prints a delta line against "
                        "the prior top entry (e.g. --baseline observe/complexity.txt).")


def _run_score(args: argparse.Namespace) -> int:
    edges = load_uses(args.edges)
    prose_redirect: dict[Path, Path] = {}
    for spec in args.prose_redirect:
        if "=" not in spec:
            raise SystemExit(f"--prose-redirect expects OLD=NEW, got {spec!r}")
        old, new = spec.split("=", 1)
        prose_redirect[Path(old)] = Path(new)

    modules = discover_modules(edges)
    root_loc = subtree_loc(args.root, modules, args.src_root)
    score = score_tree(
        edges, args.root, args.src_root,
        args.alpha, args.beta, args.beta_children_pivot, args.gamma, args.size_pivot,
        args.exact_threshold,
        args.delta, args.kappa,
        args.epsilon, args.owner_pivot,
        args.lambda_facade,
        prose_redirect or None,
        args.denominator,
    )
    if args.baseline is not None:
        update_baseline(args.baseline, score, root_loc)
    return 0


def _cmd_score(args: argparse.Namespace) -> int:
    return _run_score(args)


def _cmd_regen(args: argparse.Namespace) -> int:
    regenerate_source_data(
        args.edges, args.src_root, args.root.split("::")[0],
        reexport_correct=not args.no_reexport,
    )
    return _run_score(args)


def main() -> int:
    ap = argparse.ArgumentParser(
        prog="modgraph",
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="verb", required=True)

    ps = sub.add_parser("score", help="score a DOT subtree",
                        formatter_class=argparse.RawDescriptionHelpFormatter)
    _add_score_args(ps)
    ps.set_defaults(func=_cmd_score)

    pr = sub.add_parser("regen", help="rebuild the graph (with reexport "
                                      "correction) + doc graph, then score",
                        formatter_class=argparse.RawDescriptionHelpFormatter)
    _add_score_args(pr)
    pr.add_argument("--no-reexport", action="store_true",
                    help="diagnostic: skip re-export correction and score the raw "
                         "cargo-modules (def-resolved) graph for comparison")
    pr.set_defaults(func=_cmd_regen)

    pw = sub.add_parser("rewrite", help="what-if structural rewrite for scoring")
    wsub = pw.add_subparsers(dest="mode", required=True)

    pm = wsub.add_parser("module", help="rename whole modules")
    pm.add_argument("--edges", required=True, type=Path, help="DOT input")
    pm.add_argument("--output-edges", required=True, type=Path, help="rewritten DOT output")
    pm.add_argument("--src-root", type=Path, default=Path("src"),
                    help="source root to mirror (default: src)")
    pm.add_argument("--output-src", type=Path,
                    help="where to mirror src/ under renamed paths (default: skip)")
    pm.add_argument("--root", default="koan", help="crate root module name (default: koan)")
    pm.add_argument("--rename", action="append", type=rewrite.parse_rename, default=[],
                    metavar="OLD=NEW", help="module rename; repeatable")
    pm.add_argument("--rename-file", type=Path, help="file of OLD=NEW lines")
    pm.set_defaults(func=rewrite.cmd_module)

    pi = wsub.add_parser("item", help="extract individual items into new modules (SCIP-driven)")
    pi.add_argument("--scip", required=True, type=Path,
                    help="SCIP index (from `rust-analyzer scip <repo> --output ...`)")
    pi.add_argument("--edges", required=True, type=Path, help="DOT graph to rewrite")
    pi.add_argument("--src-root", required=True, type=Path, help="src/ directory to mirror")
    pi.add_argument("--output-edges", required=True, type=Path, help="rewritten DOT output")
    pi.add_argument("--output-src", required=True, type=Path, help="mirrored src/ tree output")
    pi.add_argument("--move", action="append", type=rewrite.parse_move, default=[],
                    metavar="OLD=NEW", help="move item OLD into module NEW. Repeatable.")
    pi.add_argument("--delete", action="append", type=rewrite.parse_delete, default=[],
                    dest="delete_specs", metavar="ITEM",
                    help="delete item ITEM from its source file. Repeatable.")
    pi.add_argument("--delete-file", action="append", default=[], dest="delete_files",
                    metavar="PATH",
                    help="delete the entire file at PATH (repo-relative, under src/).")
    pi.set_defaults(func=rewrite.cmd_item)

    args = ap.parse_args()
    return args.func(args)
