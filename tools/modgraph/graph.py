"""DOT graph I/O — the one place that parses and emits cargo-modules DOT.

A `Graph` carries the module node set plus `uses` and `owns` edges separately.
Scoring weights only `uses` edges (the import surface); `owns` edges encode the
module tree. `parse_dot` reads both; `write_dot` emits a corrected graph the
scorer can read back (its `uses` lines carry `[label="uses"]` so `load_uses`
and the scorer's edge match find them; `owns` lines are ignored by scoring).
"""
from __future__ import annotations

import dataclasses
import re
from pathlib import Path

# Anchored like the original scorer's matcher: leading whitespace, then the
# edge, then the label. `.match` from line start, `.*` swallows DOT attributes
# before the label token.
_EDGE_RE = re.compile(r'\s*"([^"]+)"\s*->\s*"([^"]+)".*\[label="(uses|owns)"')
_NODE_RE = re.compile(r'^\s*"([^"]+)"\s*\[')


@dataclasses.dataclass
class Graph:
    nodes: set[str]
    uses: list[tuple[str, str]]
    owns: list[tuple[str, str]]


def parse_dot(path: Path) -> Graph:
    nodes: set[str] = set()
    uses: list[tuple[str, str]] = []
    owns: list[tuple[str, str]] = []
    for line in path.read_text().splitlines():
        m = _EDGE_RE.match(line)
        if m:
            s, d, kind = m.group(1), m.group(2), m.group(3)
            (owns if kind == "owns" else uses).append((s, d))
            nodes.add(s)
            nodes.add(d)
            continue
        m = _NODE_RE.match(line)
        if m:
            nodes.add(m.group(1))
    return Graph(nodes=nodes, uses=uses, owns=owns)


def load_uses(path: Path) -> list[tuple[str, str]]:
    """Just the `uses` edges — what the scorer consumes."""
    return parse_dot(path).uses


_USES_ATTRS = '[label="uses", style="dashed"]'
_OWNS_ATTRS = '[label="owns", style="solid"]'


def write_dot(
    path: Path,
    nodes: set[str],
    owns: list[tuple[str, str]],
    uses: list[tuple[str, str]] | set[tuple[str, str]],
) -> None:
    lines = ["digraph {"]
    for n in sorted(nodes):
        lines.append(f'    "{n}" [label="{n}"];')
    for s, d in owns:
        lines.append(f'    "{s}" -> "{d}" {_OWNS_ATTRS};')
    for s, d in sorted(uses):
        lines.append(f'    "{s}" -> "{d}" {_USES_ATTRS};')
    lines.append("}")
    path.write_text("\n".join(lines) + "\n")
