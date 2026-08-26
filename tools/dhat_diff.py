"""Difference two dhat heap profiles into per-unit allocation-site attribution.

The attribution half of the allocation audit: `tools/alloc_audit.py` prices a term
(allocations per step / dispatch / call), this names the sites the term is made of.
Run one shape at two sizes under the `dhat` cargo feature and difference the block
counts per allocation site — a site whose count scales with the size difference is
on the per-unit path; constant-cost sites (startup, seeding) cancel.

    cargo run --features dhat -- audit/shapes/tail_loop_steps10.koan
    mv dhat-heap.json small.json
    cargo run --features dhat -- audit/shapes/tail_loop_steps100.koan
    mv dhat-heap.json big.json
    python3 tools/dhat_diff.py small.json big.json 90
    python3 tools/dhat_diff.py small.json big.json 90 --detail 'arm_tail|run_action'

`units` is the size difference (90 extra steps above). The default report aggregates
by owning frame — the deepest koan/workgraph frame under the allocation, with its
caller for context. `--detail <regex>` instead prints the full (filtered) stack of
every scaling site whose frames match the pattern.
"""

import argparse
import json
import re

# Frames that are allocator/runtime plumbing, never an owner.
NOISE = re.compile(
    r"^(dhat::|__rustc|backtrace::|alloc::|core::|std::|hashbrown::raw|"
    r"<alloc::|<core::|<std::|<dhat::|<hashbrown::raw|<T as |<str as )"
)
OWNING = re.compile(
    r"koan::|workgraph::|bumpalo::Bump<_>::new_chunk|hashbrown::(map|rustc)|smallvec::SmallVec"
)


def load(path: str) -> dict[tuple[str, ...], int]:
    """Site -> total block count, keyed by the normalized frame stack."""
    data = json.load(open(path))
    table = data["ftbl"]

    def norm(index: int) -> str:
        return re.sub(r"^0x[0-9a-f]+: ", "", table[index])

    sites: dict[tuple[str, ...], int] = {}
    for pp in data["pps"]:
        key = tuple(norm(fi) for fi in pp["fs"])
        sites[key] = sites.get(key, 0) + pp["tbk"]
    return sites


def strip_location(frame: str) -> str:
    return re.sub(r" \(.*?\)$", "", frame)


def owner(stack: tuple[str, ...]) -> str:
    """The deepest owning frame plus its nearest koan caller, as one label."""
    owning = [f for f in stack if OWNING.search(f) and not NOISE.search(f)]
    if not owning:
        return strip_location(stack[0]) if stack else "???"
    label = strip_location(owning[0])
    for frame in owning[1:]:
        if "koan::" in frame:
            return f"{label}   <-   {strip_location(frame)}"
    return label


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("small", help="dhat-heap.json of the smaller run")
    parser.add_argument("big", help="dhat-heap.json of the bigger run")
    parser.add_argument("units", type=float, help="size difference between the runs")
    parser.add_argument(
        "--detail",
        metavar="REGEX",
        help="print full stacks of scaling sites whose frames match",
    )
    parser.add_argument(
        "--floor",
        type=float,
        default=0.3,
        help="hide sites below this many allocations per unit (default 0.3)",
    )
    args = parser.parse_args()

    small, big = load(args.small), load(args.big)
    deltas = {}
    for key in set(small) | set(big):
        delta = big.get(key, 0) - small.get(key, 0)
        if delta > 0:
            deltas[key] = delta
    total = sum(deltas.values())
    print(f"# total scaling blocks: {total} over {args.units:g} units"
          f" = {total / args.units:.2f}/unit")

    if args.detail:
        pattern = re.compile(args.detail)
        for key, delta in sorted(deltas.items(), key=lambda kv: -kv[1]):
            per = delta / args.units
            if per < args.floor or not any(pattern.search(f) for f in key):
                continue
            print(f"\n== {per:.2f}/unit  (delta {delta})")
            for frame in key:
                if NOISE.search(frame):
                    continue
                print("   ", frame[:160])
        return

    aggregated: dict[str, int] = {}
    for key, delta in deltas.items():
        label = owner(key)
        aggregated[label] = aggregated.get(label, 0) + delta
    for label, delta in sorted(aggregated.items(), key=lambda kv: -kv[1]):
        if delta / args.units >= args.floor:
            print(f"{delta / args.units:6.2f}  {label}")


if __name__ == "__main__":
    main()
