"""LOC measures, the per-file size charge, and design/roadmap prose attribution.

Three LOC measures feed the score:
  * production LOC (`file_loc`/`subtree_loc`) — tests and `#[cfg(test)]` blocks
    filtered out; what the structural terms weight by;
  * raw LOC (`file_loc_raw`) — every non-blank line, the reader-cost base;
  * effective LOC — raw plus attributed doc prose and outbound-`*.md` hops,
    what `size_charge` weights.
See the package docstring (`__main__`) / the scorer module for the rationale.
"""
from __future__ import annotations

import math
import re
from collections import defaultdict
from pathlib import Path

from modules import module_to_file

REPO_ROOT = Path(__file__).resolve().parents[2]
MD_GLOBS = ("*.md", "design/**/*.md", "roadmap/**/*.md")
_MD_LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)\s]+)\)")


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


def file_loc_raw(path: Path) -> int:
    """Count every non-blank line in the file, including test code, doc
    comments, and inline comments. This is the per-reader context-cost
    measure used by `size_charge` — tests and comments are real lines a
    reader has to load, even if the structural terms (coupling, nesting)
    ignore them."""
    try:
        text = path.read_text()
    except OSError:
        return 0
    return sum(1 for line in text.splitlines() if line.strip())


def own_file_loc_raw(module: str, src_root: Path) -> int:
    """Raw LOC of just this module's own backing file (no descendants).
    Counterpart to [`own_file_loc`] that does not filter tests/comments."""
    f = module_to_file(module, src_root)
    return file_loc_raw(f) if f is not None else 0


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


def owner_credit(prose_loc: float, epsilon: float, owner_pivot: float) -> float:
    """Reward concentrating a documented concept into a named owner file.
    Symmetric in shape to the size charge — ε·L·log(1+L/P_o) — but
    subtracted from the size term, capped at the size term's value (so
    `size_charge` never goes below zero). Sub-linear below P_o (small
    concepts aren't worth their own module), super-linear above it (the
    heavily-documented concept deserves a name)."""
    if prose_loc <= 0 or epsilon <= 0.0 or owner_pivot <= 0.0:
        return 0.0
    return epsilon * prose_loc * math.log(1.0 + prose_loc / owner_pivot)


def size_charge(eff_loc: float, gamma: float, pivot: float) -> float:
    """Soft log-shaped penalty per file: γ·L·log(1 + L/T). L is the
    *effective* reader LOC — raw code/comment lines plus uniformly-
    attributed prose from any design/roadmap doc that mentions this file,
    plus a per-hop charge for every outbound `*.md` link the file embeds.
    The structural terms (coupling, nesting) still weight by production
    LOC; the size term reflects total reading effort to comprehend the
    file in isolation."""
    if eff_loc <= 0 or gamma <= 0.0 or pivot <= 0.0:
        return 0.0
    return gamma * eff_loc * math.log(1.0 + eff_loc / pivot)


def _doc_raw_loc(path: Path) -> int:
    """Non-blank lines in a markdown file — the prose-LOC measure attributed
    out to the src files the doc mentions."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return 0
    return sum(1 for line in text.splitlines() if line.strip())


def _doc_to_src_files(doc: Path, redirect: dict[Path, Path] | None = None) -> set[Path]:
    """Set of paths *relative to the repo's canonical `src/`* that `doc`
    markdown-links to. Returns `Path` objects of the form `machine/core/
    scope.rs` — caller joins these against whatever src_root they're
    scoring, so the attribution still applies when scoring a `/tmp/...`
    mirror produced by the rewriter. Test files excluded.

    `redirect` is an optional `{old_rel: new_rel}` map: doc-link targets
    matching a key are rewritten to the corresponding value before the
    set is built. Used by `--prose-redirect` to simulate doc
    consolidation alongside a code-level seam — e.g. if `lookup.md` is
    written as the canonical owner of the `core::lookup` protocol and
    the prior co-citers (scope.rs, bindings.rs, ktype_predicates.rs)
    no longer carry the protocol's prose, redirect them all to
    `core/lookup.rs`."""
    try:
        text = doc.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return set()
    out: set[Path] = set()
    canonical_src = (REPO_ROOT / "src").resolve()
    for m in _MD_LINK_RE.finditer(text):
        target = m.group(2).split("#", 1)[0].split("?", 1)[0]
        if not target or target.startswith(("http://", "https://", "mailto:")):
            continue
        resolved = (doc.parent / target).resolve()
        try:
            rel = resolved.relative_to(canonical_src)
        except ValueError:
            continue
        if resolved.suffix == ".rs" and "/tests" not in str(rel) and not _is_test_file(resolved):
            if redirect and rel in redirect:
                rel = redirect[rel]
            out.add(rel)
    return out


def _src_hop_count(path: Path) -> int:
    """Count outbound markdown-doc links (`[...](something.md)`) in this src
    file's comments — each one is a reader hop into prose."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return 0
    hops = 0
    for line in text.splitlines():
        stripped = line.lstrip()
        if not stripped.startswith("//"):
            continue
        for m in _MD_LINK_RE.finditer(stripped):
            target = m.group(2).split("#", 1)[0].split("?", 1)[0]
            if target.endswith(".md"):
                hops += 1
    return hops


def build_prose_attribution(
    src_root: Path,
    redirect: dict[Path, Path] | None = None,
) -> tuple[dict[Path, float], dict[Path, int]]:
    """Walk all design/roadmap/top-level markdown files, attribute each
    doc's raw LOC *uniformly* across every src file it links to, and count
    each src file's outbound `*.md` hops. Returns (attributed_prose,
    hop_count), both keyed by *path relative to the repo's canonical
    `src/`* (e.g. `machine/core/scope.rs`). Keying on canonical-relative
    paths lets the same attribution apply when scoring a `/tmp/...` src
    mirror produced by the rewriter. Files with zero entries are omitted
    (callers use `.get(p, 0)`)."""
    prose: dict[Path, float] = defaultdict(float)
    hops: dict[Path, int] = {}
    for pat in MD_GLOBS:
        for doc in REPO_ROOT.glob(pat):
            files = _doc_to_src_files(doc, redirect)
            if not files:
                continue
            attribution = _doc_raw_loc(doc) / len(files)
            for f in files:
                prose[f] += attribution
    src_root_abs = src_root.resolve()
    for rs in src_root_abs.rglob("*.rs"):
        if _is_test_file(rs):
            continue
        h = _src_hop_count(rs)
        if h:
            try:
                hops[rs.relative_to(src_root_abs)] = h
            except ValueError:
                continue
    return prose, hops
