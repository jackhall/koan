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


def is_test_file(path: Path) -> bool:
    """True for a source file that holds only test code — a `tests.rs` module, a
    file under a `tests/` directory, or the shared `test_support.rs` fixtures.

    Test code is invisible to the score: `cargo modules` is run without
    `--cfg-test`, so the module node set never holds a test module, and the LOC
    and coupling measures filter to match. A test-only import is a fixture's
    convenience, not a constraint on how production modules may be reshuffled.
    """
    name = path.name
    if name == "test_support.rs" or name.endswith("_tests.rs") or name == "tests.rs":
        return True
    return any(part == "tests" for part in path.parts)


def strip_cfg_test_blocks(lines: list[str]) -> list[str]:
    """Drop each `#[cfg(test)] mod … { … }` block from `lines`, brace-matched.

    The inline companion to [`is_test_file`]: a production file's own test module
    is test code sitting in a production path, so its lines and its `use`
    statements are filtered out the same way a separate `tests.rs` is.

    Expects comment-stripped lines — a brace inside a comment or string literal
    would otherwise throw the depth count off. That is the same naivety the LOC
    proxy accepts.
    """
    out: list[str] = []
    i = 0
    while i < len(lines):
        if lines[i].strip().startswith("#[cfg(test)]"):
            # The `mod … {` may sit on the same line, or after blank lines.
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            if j < len(lines) and lines[j].lstrip().startswith("mod "):
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
        out.append(lines[i])
        i += 1
    return out
