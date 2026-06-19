"""Module-path ↔ source-file mapping and module-tree navigation.

Pure path/string ops over the module namespace — no LOC, no DOT, no scoring.
Shared by the scorer, the re-export corrector, and the what-if rewriter.
"""
from __future__ import annotations

from pathlib import Path


def discover_modules(edges: list[tuple[str, str]]) -> set[str]:
    """The module set is the endpoints of the `uses` edges — matching the
    scorer's historical view (a module with no import in either direction is
    invisible to the tree walk, exactly as before)."""
    return {m for edge in edges for m in edge}


def direct_children(parent: str, modules: set[str]) -> list[str]:
    prefix = parent + "::"
    seen = set()
    for m in modules:
        if m.startswith(prefix):
            seen.add(m[len(prefix):].split("::", 1)[0])
    return sorted(seen)


def module_to_file(module: str, src_root: Path) -> Path | None:
    """`koan::machine::core::scope` -> `src/machine/core/scope.rs` (or
    `.../mod.rs`). The crate root (`koan`, no path parts) maps to no file —
    `lib.rs` is intentionally uncounted, matching the scorer's longstanding
    behaviour."""
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


def relpath_to_module(relpath: str, package: str = "koan") -> str | None:
    """`src/machine/core/scope.rs` -> `koan::machine::core::scope`
    (`mod.rs`/`lib.rs`/`main.rs` collapse to the directory module). Accepts
    paths with or without the leading `src/`."""
    p = relpath
    if p.startswith("src/"):
        p = p[4:]
    if not p.endswith(".rs"):
        return None
    p = p[:-3]
    parts = [x for x in p.split("/") if x]
    if parts and parts[-1] in ("mod", "lib", "main"):
        parts = parts[:-1]
    return "::".join([package] + parts)
