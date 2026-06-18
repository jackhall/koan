"""Re-attribute `uses` edges to the import surface the author actually wrote.

`cargo modules` resolves every `use` edge to the item's *definition* module,
discarding `pub use` re-export facades. For reorganization-coupling scoring that
is the wrong target: if `machine.rs` re-exports `Scope` at the top level, a
consumer writing `use crate::machine::Scope` is coupled to machine's public
vocabulary, not to `core::scope`'s internal location — moving `scope` anywhere
under machine would not break it. cargo-modules still draws the edge to
`core::scope`, inflating `fan`/`cross`.

`correct()` discards cargo-modules' `uses` edges and rebuilds them from the
module path each `use` statement literally names (its prefix, minus the trailing
item segment). The result reflects what actually constrains a submodule
reshuffle: genuine deep imports (the author named a child path) survive as deep
edges; facade imports collapse onto the facade module the author entered through.
This is core graph construction, applied by `regen` — not an optional pass.

Limitation: attribution is parsed from `use` statements. Inline fully-qualified
path references (no `use`) are not captured; the `use`-based signal dominates.
"""
from __future__ import annotations

import re
from pathlib import Path

from modules import relpath_to_module


def strip_comments(text: str) -> str:
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return text


def iter_use_statements(text: str):
    """Yield the body of each `use ...;` / `pub use ...;` statement (no trailing ;)."""
    text = strip_comments(text)
    i = 0
    n = len(text)
    while i < n:
        m = re.compile(r"\buse\b").search(text, i)
        if not m:
            break
        j = m.end()
        # walk to the matching `;`, respecting brace nesting
        depth = 0
        k = j
        while k < n:
            c = text[k]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
            elif c == ";" and depth == 0:
                break
            k += 1
        yield text[j:k].strip()
        i = k + 1


def split_top_level(s: str) -> list[str]:
    parts, depth, cur = [], 0, []
    for c in s:
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
        if c == "," and depth == 0:
            parts.append("".join(cur)); cur = []
        else:
            cur.append(c)
    if "".join(cur).strip():
        parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


def parse_use_tree(body: str, prefix: list[str], out: list[list[str]]):
    """Expand a use-tree body into leaf segment-paths (item names included)."""
    body = body.strip()
    if not body:
        return
    depth = 0
    brace_at = -1
    for idx, c in enumerate(body):
        if c == "{":
            if depth == 0:
                brace_at = idx
                break
            depth += 1
    if brace_at == -1:
        # leaf: a::b::c [as d]  /  a::b::*
        path = body.split(" as ")[0].strip()
        segs = [s.strip() for s in path.split("::") if s.strip()]
        if segs:
            out.append(prefix + segs)
        return
    head = body[:brace_at].rstrip(":").strip()
    head_segs = [s.strip() for s in head.split("::") if s.strip()]
    depth = 0
    close = -1
    for idx in range(brace_at, len(body)):
        if body[idx] == "{":
            depth += 1
        elif body[idx] == "}":
            depth -= 1
            if depth == 0:
                close = idx
                break
    inner = body[brace_at + 1:close]
    for part in split_top_level(inner):
        parse_use_tree(part, prefix + head_segs, out)


def resolve_segments(segs: list[str], self_mod: str, package: str) -> list[str] | None:
    """Resolve crate/self/super-relative segments to an absolute path list."""
    self_parts = self_mod.split("::")
    if not segs:
        return None
    if segs[0] == "crate":
        return [package] + segs[1:]
    if segs[0] == "self":
        return self_parts + segs[1:]
    if segs[0] == "super":
        up = self_parts[:]
        rest = segs
        while rest and rest[0] == "super":
            if len(up) <= 1:
                return None
            up = up[:-1]
            rest = rest[1:]
        return up + rest
    # bare path: external crate (e.g. std, serde) — ignore
    return None


def written_module(abs_segs: list[str], known: set[str]) -> str | None:
    """Map an absolute use-path (item included) to the module it enters.

    Walk from the longest prefix down: the written module is the deepest prefix
    that is a known module node. (`crate::machine::model::KType` -> `..::model`;
    `crate::machine::core::kfunction::body::ReturnContract` -> `..::kfunction::body`.)
    """
    for end in range(len(abs_segs), 0, -1):
        cand = "::".join(abs_segs[:end])
        if cand in known:
            return cand
    return None


def correct(
    known: set[str], src_root: Path, package: str = "koan"
) -> set[tuple[str, str]]:
    """Rebuild the `uses` edge set from the `use` statements under `src_root`.

    `known` is the module node set (from the cargo-modules graph). Each source
    module's `use` statements are resolved and attributed to the deepest known
    module they name; edges to the module's own self are dropped.
    """
    uses: set[tuple[str, str]] = set()
    for rs in sorted(src_root.rglob("*.rs")):
        rel = "src/" + str(rs.relative_to(src_root)).replace("\\", "/")
        mod = relpath_to_module(rel, package)
        if mod is None:
            continue
        if mod not in known and mod != package:
            # a file whose module isn't a graph node (e.g. a tests submodule):
            # attribute its edges to the nearest known ancestor
            parts = mod.split("::")
            while parts and "::".join(parts) not in known:
                parts = parts[:-1]
            if not parts:
                continue
            mod = "::".join(parts)
        text = rs.read_text(errors="ignore")
        for body in iter_use_statements(text):
            leaves: list[list[str]] = []
            parse_use_tree(body, [], leaves)
            for segs in leaves:
                abs_segs = resolve_segments(segs, mod, package)
                if not abs_segs or abs_segs[0] != package:
                    continue
                dst = written_module(abs_segs, known)
                if dst and dst != mod:
                    uses.add((mod, dst))
    return uses
