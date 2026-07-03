#!/usr/bin/env python3
"""Maintain links between docs and source for the koan repo.

Subcommands:
  check                run every gating audit in one pass: broken links, roadmap
                       Requires/Unblocks symmetry, orphaned design/ + roadmap/
                       docs, and the derived Next-items list, plus an
                       informational report of src/**/*.rs files changed vs a git
                       ref. Exits non-zero if any gate fails; the source-tree
                       section never affects the exit code.
  sync-next            regenerate the global "Next items" list in roadmap/README.md
                       plus each project README's slice, from the roadmap
                       dependency graph (items with no open prerequisite)
  refs <path>          list every file that links to <path>
  fix-refs OLD=NEW ... rewrite every link that resolves to OLD so it points at
                       NEW instead — used to fix inbound references after a
                       file move or rename
  rm-roadmap <path>    delete a roadmap/*.md file and prune inbound/outbound bullets
  dag                  emit a Graphviz DOT digraph of the roadmap/*.md
                       Requires/Unblocks edges — pipe to `dot -Tpng > dag.png` or
                       paste into an online viewer
  signals              emit mechanical doc-abstraction signals as JSON
                       (co-cited src triples, backref density, comment-density
                       spikes, shared phrases) — paired with the `doc-abstraction`
                       skill, which adds the semantic judgment the tool can't
"""

from __future__ import annotations

import argparse
import bisect
import os
import re
import subprocess
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

MD_GLOBS = ("*.md", "design/**/*.md", "roadmap/**/*.md")
SRC_GLOBS = ("src/**/*.rs", "workgraph/src/**/*.rs")

# [text](target) — target stops at the first ')' that isn't escaped, no nesting,
# no whitespace, no '#' fragment included in the resolved path.
LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)\s]+)\)")

# `# Heading` — top-of-file H1, used to derive the canonical title of a doc.
H1_RE = re.compile(r"^#\s+(.*?)\s*$")


def read_h1_title(path: Path) -> str | None:
    """Return the text of the first `# Heading` line in `path`, or None.

    Used by `fix-refs` to learn the canonical title of a renamed doc so it can
    update visible link text alongside the path.
    """
    try:
        with path.open(encoding="utf-8") as f:
            for raw in f:
                m = H1_RE.match(raw.rstrip("\r\n"))
                if m:
                    return m.group(1)
    except (OSError, UnicodeDecodeError):
        return None
    return None


@dataclass(frozen=True)
class Link:
    source: Path        # file the link appears in
    line: int           # 1-based
    text: str
    target: str         # raw target as written
    resolved: Path      # target resolved against source's directory


def iter_doc_files() -> list[Path]:
    files: list[Path] = []
    for pat in MD_GLOBS:
        files.extend(REPO.glob(pat))
    return sorted(set(files))


def iter_src_files() -> list[Path]:
    files: list[Path] = []
    for pat in SRC_GLOBS:
        files.extend(REPO.glob(pat))
    return sorted(set(files))


def _accept_link(path: Path, target: str, is_rust: bool) -> Path | None:
    """Resolve a raw `[text](target)` target to a filesystem path, or return None
    if it's not a path-style link the checker should follow (URLs, mail, rustdoc
    intra-doc references)."""
    fs_part = target.split("#", 1)[0].split("?", 1)[0]
    if not fs_part or fs_part.startswith(("http://", "https://", "mailto:")):
        return None
    if is_rust and "::" in fs_part:
        return None
    return (path.parent / fs_part).resolve()


def extract_links(path: Path) -> list[Link]:
    """Pull every [text](target) link out of a file.

    For markdown the regex runs over the whole file so links whose `[text]` wraps
    across lines are caught — the prose convention is to wrap long sentences, and
    the original line-by-line scan silently skipped those.

    For .rs files we keep the per-line scan and only consider lines that look like
    doc comments (`//!` or `///`) or ordinary `//` comments — code-string literals
    containing `[x](y)` are rare and not worth special-casing, and multi-line
    rustdoc links across `///` continuations are uncommon enough to defer.
    """
    out: list[Link] = []
    is_rust = path.suffix == ".rs"
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return out

    if is_rust:
        for lineno, line in enumerate(text.splitlines(), start=1):
            if "//" not in line:
                continue
            for m in LINK_RE.finditer(line):
                resolved = _accept_link(path, m.group(2), is_rust=True)
                if resolved is None:
                    continue
                out.append(Link(path, lineno, m.group(1), m.group(2), resolved))
        return out

    # Markdown: scan the whole file so wrapped links are caught. `[^\]]+` already
    # spans newlines (negated character classes ignore DOTALL), and `[^)\s]+` for
    # the target excludes whitespace, so the (...) portion stays single-line.
    line_starts = [0]
    for idx, ch in enumerate(text):
        if ch == "\n":
            line_starts.append(idx + 1)
    for m in LINK_RE.finditer(text):
        resolved = _accept_link(path, m.group(2), is_rust=False)
        if resolved is None:
            continue
        # bisect_right(line_starts, offset) returns the 1-indexed line containing
        # offset: it counts the line starts at-or-before offset, which equals the
        # line number.
        lineno = bisect.bisect_right(line_starts, m.start())
        out.append(Link(path, lineno, m.group(1), m.group(2), resolved))
    return out


def all_links() -> list[Link]:
    links: list[Link] = []
    for f in iter_doc_files() + iter_src_files():
        links.extend(extract_links(f))
    return links


def rel(p: Path) -> str:
    try:
        return str(p.relative_to(REPO))
    except ValueError:
        return str(p)


# ---------- check (orchestrator + per-section helpers) ----------

def _check_links() -> int:
    broken = []
    for link in all_links():
        if not link.resolved.exists():
            broken.append(link)
    for link in broken:
        print(f"{rel(link.source)}:{link.line}: broken link "
              f"[{link.text}]({link.target}) -> {rel(link.resolved)}")
    if broken:
        print(f"\n{len(broken)} broken link(s).", file=sys.stderr)
        return 1
    print("All links resolve.")
    return 0


# ---------- header helpers ----------

# `## Heading` line — CommonMark-strict on the space after `##`. Trailing colons
# and whitespace are tolerated when matching a specific header name.
H2_HEADER_RE = re.compile(r"^\s*##\s+\S")


def is_h2_header_line(line: str) -> bool:
    """True if `line` is any level-2 markdown header. Used to detect section ends."""
    return bool(H2_HEADER_RE.match(line))


def matches_h2_header(line: str, name: str) -> bool:
    """True if `line` is `## <name>` (case-insensitive, allowing optional trailing
    colon and whitespace). Rejects partial-word matches like `## Dependencies blah`.
    """
    m = re.match(r"^\s*##\s+(.*?)\s*$", line)
    if not m:
        return False
    return m.group(1).rstrip(":").strip().lower() == name.lower()


# Inline bold labels: `**Requires:**`, `**Requires**:`, `__Unblocks:__`, etc.
# Tolerates colon inside or outside the bold delimiters, the `__` underscore form,
# case variations, and trailing whitespace. Rejects content after the label.
DEP_HEADER_RE = re.compile(
    r"^\s*(\*\*|__)(Requires|Unblocks)(?::?\1|\1:?)\s*$",
    re.IGNORECASE,
)
NONE_RE = re.compile(r"\bnone\b", re.IGNORECASE)


# ---------- deps ----------

def parse_dep_section(path: Path) -> tuple[set[Path], set[Path]]:
    """Return (requires, unblocks) — sets of resolved paths into `roadmap/`.

    Reads only the **Dependencies** section. A section ends at the next h2 header
    (`## ...`) or EOF. Targets outside `roadmap/` (e.g. `design/foo.md`) are
    ignored — only intra-roadmap edges have a symmetric partner. Targets are
    resolved against the link's source file's directory so `../sibling.md` and
    `../topic/item.md` work for items in `roadmap/<subdir>/`.
    """
    roadmap_dir = (REPO / "roadmap").resolve()
    requires: set[Path] = set()
    unblocks: set[Path] = set()
    text = path.read_text(encoding="utf-8")

    in_deps = False
    current: set[Path] | None = None
    for line in text.splitlines():
        if matches_h2_header(line, "dependencies"):
            in_deps = True
            continue
        if not in_deps:
            continue
        if is_h2_header_line(line):
            break
        m = DEP_HEADER_RE.match(line)
        if m:
            kind = m.group(2).lower()
            current = requires if kind == "requires" else unblocks
            continue
        if current is None:
            continue
        for m in LINK_RE.finditer(line):
            raw = m.group(2).split("#", 1)[0].split("?", 1)[0]
            if not raw or raw.startswith(("http://", "https://", "mailto:")):
                continue
            if not raw.endswith(".md"):
                continue
            resolved = (path.parent / raw).resolve()
            try:
                resolved.relative_to(roadmap_dir)
            except ValueError:
                continue
            current.add(resolved)
    return requires, unblocks


def _check_deps() -> int:
    roadmap_dir = REPO / "roadmap"
    items = sorted(roadmap_dir.glob("**/*.md"))
    deps: dict[Path, tuple[set[Path], set[Path]]] = {}
    for f in items:
        deps[f.resolve()] = parse_dep_section(f)

    def disp(p: Path) -> str:
        return rel(p)

    issues: list[str] = []
    for name, (req, unb) in deps.items():
        for target in sorted(req):
            if target not in deps:
                issues.append(
                    f"{disp(name)}: requires '{disp(target)}' but file does not exist"
                )
                continue
            if name not in deps[target][1]:
                issues.append(
                    f"{disp(name)} requires {disp(target)}, but {disp(target)} "
                    f"does not list {disp(name)} under Unblocks"
                )
        for target in sorted(unb):
            if target not in deps:
                issues.append(
                    f"{disp(name)}: unblocks '{disp(target)}' but file does not exist"
                )
                continue
            if name not in deps[target][0]:
                issues.append(
                    f"{disp(name)} unblocks {disp(target)}, but {disp(target)} "
                    f"does not list {disp(name)} under Requires"
                )

    for line in issues:
        print(line)
    if issues:
        print(f"\n{len(issues)} dependency asymmetr{'y' if len(issues)==1 else 'ies'}.",
              file=sys.stderr)
        return 1
    print("Roadmap Requires/Unblocks edges are symmetric.")
    return 0


# ---------- orphans ----------

def _check_orphans() -> int:
    targets: list[Path] = []
    for sub in ("design", "roadmap"):
        targets.extend((REPO / sub).glob("**/*.md"))
    targets.sort()

    referenced: set[Path] = set()
    for link in all_links():
        if link.resolved.exists():
            referenced.add(link.resolved)

    orphans = [t for t in targets if t.resolve() not in referenced]
    for o in orphans:
        print(rel(o))
    if orphans:
        print(f"\n{len(orphans)} orphaned doc(s).", file=sys.stderr)
        return 1
    print("No orphaned design/ or roadmap/ docs.")
    return 0


# ---------- refs ----------

def cmd_refs(args: argparse.Namespace) -> int:
    target = Path(args.path)
    if not target.is_absolute():
        target = (Path.cwd() / target).resolve()
    else:
        target = target.resolve()
    if not target.exists():
        print(f"warning: {args.path} does not exist on disk; searching anyway",
              file=sys.stderr)

    hits: list[Link] = []
    for link in all_links():
        if link.resolved == target:
            hits.append(link)

    for link in hits:
        print(f"{rel(link.source)}:{link.line}: [{link.text}]({link.target})")
    if not hits:
        print(f"No references to {rel(target)}.")
    return 0


# ---------- shared: bullet boundaries ----------

BULLET_RE = re.compile(r"^(\s*)[-*]\s+")


def find_section(lines: list[str], header: str) -> tuple[int, int] | None:
    """Locate a `## <header>` section. Returns (start, end) where start is the
    header line and end is exclusive (next h2 or EOF). Tolerates case, trailing
    colon, and trailing whitespace on the header line.
    """
    n = len(lines)
    for i, line in enumerate(lines):
        if matches_h2_header(line, header):
            j = i + 1
            while j < n and not is_h2_header_line(lines[j]):
                j += 1
            return (i, j)
    return None


def bullet_end(lines: list[str], start: int, end: int) -> int:
    """Given a bullet beginning at lines[start], return the exclusive end index.

    A continuation line is any non-blank line whose indent is at least
    `indent + 2` (the column after `- `). The bullet ends at the first blank
    line, less-indented line, or `end`.
    """
    m = BULLET_RE.match(lines[start])
    if not m:
        return start + 1
    indent = len(m.group(1))
    threshold = indent + 2
    j = start + 1
    while j < end:
        jline = lines[j]
        if not jline.strip():
            break
        jindent = len(jline) - len(jline.lstrip())
        if jindent < threshold:
            break
        j += 1
    return j


def remove_matching_bullets(
    lines: list[str], start: int, end: int,
    target_resolved: Path, source_dir: Path,
) -> tuple[list[str], int]:
    """Walk lines[start:end]; drop bullets whose links resolve to target_resolved.

    Returns (kept_lines, num_removed). Continuation lines belonging to a removed
    bullet are also dropped.
    """
    kept: list[str] = []
    i = start
    removed = 0
    while i < end:
        if not BULLET_RE.match(lines[i]):
            kept.append(lines[i])
            i += 1
            continue
        j = bullet_end(lines, i, end)
        bullet_text = "".join(lines[i:j])
        match = False
        for m in LINK_RE.finditer(bullet_text):
            raw = m.group(2).split("#", 1)[0].split("?", 1)[0]
            if not raw or raw.startswith(("http://", "https://", "mailto:")):
                continue
            if (source_dir / raw).resolve() == target_resolved:
                match = True
                break
        if match:
            removed += 1
        else:
            kept.extend(lines[i:j])
        i = j
    return kept, removed


# ---------- next items (derived from the dependency graph) ----------

NEXT_ITEMS_HEADER = "Next items"

# Fixed intro paragraph for the generated section. Hardcoded rather than wrapped
# at runtime so the `check` gate's exact-match comparison stays stable.
NEXT_ITEMS_INTRO = [
    "Computed from the dependency graph: every roadmap item whose `Requires:` list no\n",
    "longer names an unshipped item — anything here can be picked up without first\n",
    "landing something else. Regenerated by `python3 tools/doclinks.py sync-next`; do\n",
    "not edit by hand. Each project subdirectory's README carries its own slice.\n",
]

# Per-project variant, written into each `roadmap/<project>/README.md`. Same
# derived-and-gated contract as the global list, scoped to one project's items.
PROJECT_NEXT_INTRO = [
    "This project's items with no unshipped prerequisite — ready to start.\n",
    "Regenerated by `python3 tools/doclinks.py sync-next`; do not edit by hand.\n",
]


def compute_next_items() -> list[Path]:
    """Resolved paths of roadmap items with no unresolved roadmap prerequisite.

    An item is 'next' when its **Requires:** set — already restricted to
    intra-roadmap edges by `parse_dep_section` — names no file that still exists:
    a require pointing at an already-deleted (shipped) item no longer blocks,
    while one pointing at a live roadmap item does. Sorted by repo-relative path
    so the generated list is deterministic. `roadmap/README.md` is excluded."""
    roadmap_dir = (REPO / "roadmap").resolve()
    out: list[Path] = []
    for f in sorted(roadmap_dir.glob("**/*.md")):
        if f.name == "README.md":
            continue
        requires, _ = parse_dep_section(f)
        if not any(r.exists() for r in requires):
            out.append(f.resolve())
    return out


def project_readmes() -> list[Path]:
    """Each roadmap subdirectory's `README.md` — one per project — sorted by path.
    These carry the moved per-project descriptions plus a derived per-project
    `## Next items` slice; `sync-next` regenerates the slice and `check` gates it."""
    roadmap_dir = (REPO / "roadmap").resolve()
    return sorted(roadmap_dir.glob("*/README.md"))


def next_items_in(project_dir: Path, next_paths: list[Path]) -> list[Path]:
    """The subset of `next_paths` that live directly in `project_dir`."""
    pd = project_dir.resolve()
    return [p for p in next_paths if p.parent == pd]


def render_next_block(
    readme: Path, next_paths: list[Path], intro: list[str] = NEXT_ITEMS_INTRO,
) -> list[str]:
    """Render the whole `## Next items` section as newline-terminated lines:
    header, blank, intro paragraph, blank, one `- [Title](path)` bullet per item,
    and a trailing blank that separates it from the following `## ` header. Hrefs
    are relative to README's directory (roadmap/), matching the rest of the index.
    """
    block: list[str] = [f"## {NEXT_ITEMS_HEADER}\n", "\n"]
    block.extend(intro)
    block.append("\n")
    for p in next_paths:
        title = read_h1_title(p) or p.stem
        href = os.path.relpath(p, readme.parent)
        block.append(f"- [{title}]({href})\n")
    block.append("\n")
    return block


def _links_under_roadmap(block: list[str], base_dir: Path) -> set[Path]:
    """Resolved roadmap/*.md targets of every link in `block` (a slice of README
    lines), used to diff the listed Next-items membership against the computed
    set."""
    roadmap_dir = (REPO / "roadmap").resolve()
    out: set[Path] = set()
    for line in block:
        for m in LINK_RE.finditer(line):
            raw = m.group(2).split("#", 1)[0].split("?", 1)[0]
            if not raw or raw.startswith(("http://", "https://", "mailto:")):
                continue
            resolved = (base_dir / raw).resolve()
            try:
                resolved.relative_to(roadmap_dir)
            except ValueError:
                continue
            out.add(resolved)
    return out


def _apply_next_section(
    readme: Path, items: list[Path], intro: list[str], dry_run: bool,
) -> bool:
    """Regenerate the `## Next items` section of one README from `items`.

    Splices the section in place (preserving any description above it); if the
    README has no section yet, appends it at the end. Prints a per-file
    add/remove diff and returns whether it changed."""
    lines = readme.read_text(encoding="utf-8").splitlines(keepends=True)
    new_block = render_next_block(readme, items, intro)
    section = find_section(lines, NEXT_ITEMS_HEADER)
    if section is None:
        old_set: set[Path] = set()
        new_lines = lines + new_block
        changed = True
    else:
        start, end = section
        old_set = _links_under_roadmap(lines[start:end], readme.parent)
        new_lines = lines[:start] + new_block + lines[end:]
        changed = lines[start:end] != new_block

    if not changed:
        return False
    new_set = {p.resolve() for p in items}
    for p in sorted(new_set - old_set):
        print(f"  + {rel(p)}")
    for p in sorted(old_set - new_set):
        print(f"  - {rel(p)}")
    if new_set == old_set:
        print("  (membership unchanged; refreshed titles or formatting)")
    if not dry_run:
        readme.write_text("".join(new_lines), encoding="utf-8")
    verb = "would update" if dry_run else "updated"
    print(f"{verb} {rel(readme)} ({len(items)} next item(s))")
    return True


def cmd_sync_next(args: argparse.Namespace) -> int:
    """Regenerate the global `## Next items` list plus each project README's slice."""
    readme = REPO / "roadmap" / "README.md"
    if not readme.exists():
        print("error: roadmap/README.md not found", file=sys.stderr)
        return 1
    next_paths = compute_next_items()

    any_changed = _apply_next_section(readme, next_paths, NEXT_ITEMS_INTRO, args.dry_run)
    for proj in project_readmes():
        items = next_items_in(proj.parent, next_paths)
        any_changed |= _apply_next_section(proj, items, PROJECT_NEXT_INTRO, args.dry_run)

    if not any_changed:
        print(f"Next items already in sync ({len(next_paths)} item(s)); no changes.")
    return 0


def _check_one_next(readme: Path, items: list[Path], intro: list[str]) -> int:
    """Gate one README's `## Next items` section against what `sync-next` emits.
    Prints per-file discrepancies; returns 0 when in sync, 1 otherwise."""
    if not readme.exists():
        print(f"  {rel(readme)}: not found.", file=sys.stderr)
        return 1
    lines = readme.read_text(encoding="utf-8").splitlines(keepends=True)
    section = find_section(lines, NEXT_ITEMS_HEADER)
    expected = render_next_block(readme, items, intro)
    if section is None:
        print(f"  {rel(readme)}: no '## Next items' section "
              "(run: python3 tools/doclinks.py sync-next)")
        return 1
    current = lines[section[0]:section[1]]
    if current == expected:
        return 0
    cur_set = _links_under_roadmap(current, readme.parent)
    exp_set = {p.resolve() for p in items}
    for p in sorted(exp_set - cur_set):
        print(f"  {rel(readme)}: missing from Next items: {rel(p)}")
    for p in sorted(cur_set - exp_set):
        print(f"  {rel(readme)}: stale in Next items:     {rel(p)}")
    if cur_set == exp_set:
        print(f"  {rel(readme)}: membership correct but section text stale "
              "(title or formatting drift).")
    return 1


def _check_next_items() -> int:
    """Gate: the global `## Next items` list and every project README's slice must
    each equal what `sync-next` would emit."""
    next_paths = compute_next_items()
    rc = _check_one_next(REPO / "roadmap" / "README.md", next_paths, NEXT_ITEMS_INTRO)
    for proj in project_readmes():
        items = next_items_in(proj.parent, next_paths)
        rc = max(rc, _check_one_next(proj, items, PROJECT_NEXT_INTRO))
    if rc == 0:
        print("Next items lists match the dependency graph.")
    else:
        print("  fix: python3 tools/doclinks.py sync-next")
        print("\nNext-items lists are out of sync with the dependency graph.",
              file=sys.stderr)
    return rc


# ---------- fix-refs ----------

def _split_target(target: str) -> tuple[str, str]:
    """Split `path#frag` or `path?q=v` into (path, suffix-including-separator)."""
    for sep in ("#", "?"):
        idx = target.find(sep)
        if idx >= 0:
            return target[:idx], target[idx:]
    return target, ""


def _parse_mapping(arg: str) -> tuple[str, str]:
    if "=" not in arg:
        raise ValueError(f"mapping must be OLD=NEW, got: {arg!r}")
    old, new = arg.split("=", 1)
    old, new = old.strip(), new.strip()
    if not old or not new:
        raise ValueError(f"mapping has empty side: {arg!r}")
    return old, new


def cmd_fix_refs(args: argparse.Namespace) -> int:
    raw_pairs: list[str] = list(args.mapping or [])
    if args.from_file:
        for raw_line in Path(args.from_file).read_text(encoding="utf-8").splitlines():
            stripped = raw_line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            raw_pairs.append(stripped)
    if not raw_pairs:
        print("error: no mappings supplied (pass OLD=NEW pairs or --from-file)",
              file=sys.stderr)
        return 1

    by_resolved: dict[Path, tuple[Path, str, str, str | None]] = {}
    for raw in raw_pairs:
        try:
            old_disp, new_disp = _parse_mapping(raw)
        except ValueError as e:
            print(f"error: {e}", file=sys.stderr)
            return 1
        old_resolved = (REPO / old_disp).resolve()
        new_abs = (REPO / new_disp).resolve()
        if not new_abs.exists():
            print(f"error: new path does not exist: {new_disp}", file=sys.stderr)
            return 1
        if old_resolved in by_resolved:
            print(f"error: duplicate mapping for {old_disp}", file=sys.stderr)
            return 1
        new_title = None if args.keep_text else read_h1_title(new_abs)
        by_resolved[old_resolved] = (new_abs, old_disp, new_disp, new_title)

    edits: dict[Path, list[tuple[int, str, str, str, str, str, str]]] = defaultdict(list)
    for link in all_links():
        if link.resolved not in by_resolved:
            continue
        new_abs, old_disp, new_disp, new_title = by_resolved[link.resolved]
        new_path_part = os.path.relpath(new_abs, link.source.parent)
        _, suffix = _split_target(link.target)
        new_raw = new_path_part + suffix
        new_text = new_title if (new_title and new_title != link.text) else link.text
        old_substr = f"[{link.text}]({link.target})"
        new_substr = f"[{new_text}]({new_raw})"
        if old_substr == new_substr:
            continue
        edits[link.source].append(
            (link.line, old_substr, new_substr, old_disp, new_disp,
             link.text, new_text)
        )

    if not edits:
        print("No links matched the given mappings.")
        return 0

    total = 0
    for f in sorted(edits):
        items = edits[f]
        text = f.read_text(encoding="utf-8")
        lines = text.splitlines(keepends=True)
        for lineno, old_sub, new_sub, *_ in items:
            lines[lineno - 1] = lines[lineno - 1].replace(old_sub, new_sub, 1)
        if not args.dry_run:
            f.write_text("".join(lines), encoding="utf-8")
        verb = "would rewrite" if args.dry_run else "rewrote"
        for lineno, _, _, old_disp, new_disp, old_text, new_text in items:
            line = f"{rel(f)}:{lineno}: {verb} {old_disp} -> {new_disp}"
            if old_text != new_text:
                line += f" (text: {old_text!r} -> {new_text!r})"
            print(line)
        total += len(items)

    suffix = " (dry-run)" if args.dry_run else ""
    print(f"\n{total} link(s) across {len(edits)} file(s){suffix}.")
    return 0


# ---------- rm-roadmap ----------

def cmd_rm_roadmap(args: argparse.Namespace) -> int:
    target = Path(args.path)
    if not target.is_absolute():
        target = (Path.cwd() / target).resolve()
    else:
        target = target.resolve()

    roadmap_dir = (REPO / "roadmap").resolve()
    try:
        target.relative_to(roadmap_dir)
    except ValueError:
        print(f"error: {args.path} is not under roadmap/", file=sys.stderr)
        return 1
    if not target.exists():
        print(f"error: {args.path} does not exist", file=sys.stderr)
        return 1
    if target.suffix != ".md":
        print(f"error: {args.path} is not a markdown file", file=sys.stderr)
        return 1

    plan: list[tuple[Path, list[str], int]] = []  # (path, new_lines, removed)

    # Other roadmap items: prune Dependencies bullets pointing at target. The
    # index lists (global `## Next items` and each project README's slice) are
    # derived from the dependency graph, so they are regenerated wholesale after
    # the delete (below) rather than pruned here — deleting an item can also
    # *add* newly-unblocked dependents to them.
    for f in sorted(roadmap_dir.glob("**/*.md")):
        if f.resolve() == target:
            continue
        lines = f.read_text(encoding="utf-8").splitlines(keepends=True)
        section = find_section(lines, "dependencies")
        if not section:
            continue
        kept, removed = remove_matching_bullets(
            lines, section[0] + 1, section[1], target, f.parent,
        )
        if removed:
            new_lines = lines[:section[0] + 1] + kept + lines[section[1]:]
            plan.append((f, new_lines, removed))

    verb = "would update" if args.dry_run else "updated"
    for f, _, removed in plan:
        print(f"{verb} {rel(f)} ({removed} bullet(s) removed)")
    if not plan:
        print("No inbound bullets to prune.")

    delete_verb = "would delete" if args.dry_run else "deleted"
    if not args.dry_run:
        for f, new_lines, _ in plan:
            f.write_text("".join(new_lines), encoding="utf-8")
        target.unlink()
    print(f"{delete_verb} {rel(target)}")

    if args.dry_run:
        print("\nwould regenerate roadmap/README.md '## Next items' from the "
              "dependency graph")
        return 0

    # The delete may have unblocked dependents; rebuild the derived list.
    print("\nRegenerating '## Next items':")
    cmd_sync_next(argparse.Namespace(dry_run=False))

    print("\nNote: design-doc 'Open work' entries and source comments are "
          "not auto-handled. Running "
          "`check` to surface any remaining stale references:\n")
    return cmd_check(argparse.Namespace())


# ---------- dag ----------

def cmd_dag(args: argparse.Namespace) -> int:
    """Emit a Graphviz DOT digraph of intra-roadmap Requires/Unblocks edges.

    Edges point prerequisite -> dependent (`A -> B` means "do A first"). The
    Requires and Unblocks halves of each pair are unioned: `check` already
    gates symmetry, but during in-flight edits the two sides may briefly
    disagree, and surfacing both edges is more useful than silently dropping
    one.
    """
    roadmap_dir = REPO / "roadmap"
    items = sorted(roadmap_dir.glob("**/*.md"))

    # Key by resolved path so subdirectory items with colliding basenames stay
    # distinct. The on-graph URL is the repo-relative path (also unique).
    titles: dict[Path, str] = {}
    rels: dict[Path, str] = {}
    edges: set[tuple[Path, Path]] = set()
    for f in items:
        key = f.resolve()
        titles[key] = read_h1_title(f) or f.stem
        rels[key] = rel(f)
    for f in items:
        key = f.resolve()
        req, unb = parse_dep_section(f)
        for r in req:
            if r in titles:
                edges.add((r, key))
        for u in unb:
            if u in titles:
                edges.add((key, u))

    def node_id(p: Path) -> str:
        return "n_" + re.sub(r"[^A-Za-z0-9]", "_", rels[p].removesuffix(".md"))

    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace('"', '\\"')

    print("digraph roadmap {")
    print("  rankdir=LR;")
    print("  node [shape=box, style=\"rounded,filled\", fillcolor=\"#f5f5f5\", "
          "fontname=\"Helvetica\"];")
    print("  edge [color=\"#555555\"];")
    for key in sorted(titles, key=lambda p: rels[p]):
        print(f'  {node_id(key)} [label="{esc(titles[key])}", '
              f'tooltip="{esc(rels[key])}", URL="{esc(rels[key])}"];')
    for src, dst in sorted(edges, key=lambda e: (rels[e[0]], rels[e[1]])):
        print(f"  {node_id(src)} -> {node_id(dst)};")
    print("}")
    return 0


# ---------- src-changes ----------

# `git diff --name-status -M` rows look like one of:
#   M\tpath
#   A\tpath
#   D\tpath
#   R<score>\told\tnew
#   C<score>\told\tnew
# We only inspect the leading letter and the path columns.
def _changed_src_files(base: str) -> list[tuple[str, Path, Path | None]]:
    """Run `git diff --name-status -M <base> -- src workgraph/src` and return
    rows for `.rs` files only. Each row is (status, current_path, old_path),
    where old_path is set for renames/copies and for deletions (so callers can
    look up inbound links to the path that no longer exists)."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(REPO), "diff", "--name-status", "-M",
             base, "--", "src", "workgraph/src"],
            capture_output=True, text=True, check=True,
        )
    except FileNotFoundError:
        print("error: git not found on PATH", file=sys.stderr)
        sys.exit(1)
    except subprocess.CalledProcessError as e:
        msg = e.stderr.strip() or f"git diff exited {e.returncode}"
        print(f"error: {msg}", file=sys.stderr)
        sys.exit(1)

    rows: list[tuple[str, Path, Path | None]] = []
    for raw in proc.stdout.splitlines():
        parts = raw.split("\t")
        if len(parts) < 2:
            continue
        code = parts[0][:1]
        if code in ("R", "C") and len(parts) >= 3:
            old, new = parts[1], parts[2]
            if not new.endswith(".rs"):
                continue
            rows.append((code, REPO / new, REPO / old))
        elif code == "D":
            path = parts[1]
            if not path.endswith(".rs"):
                continue
            rows.append((code, REPO / path, REPO / path))
        else:
            path = parts[1]
            if not path.endswith(".rs"):
                continue
            rows.append((code, REPO / path, None))
    return rows


def _report_src_changes(base: str) -> int:
    rows = _changed_src_files(base)

    # Index every doc link by resolved target so we can look up inbound links
    # for both the current and (for renames/deletions) the prior path.
    inbound: dict[Path, list[Link]] = defaultdict(list)
    for link in all_links():
        inbound[link.resolved].append(link)

    if not rows:
        print(f"No src/**/*.rs files changed vs {base}.")
        return 0

    status_word = {"A": "added", "M": "modified", "D": "deleted",
                   "R": "renamed", "C": "copied", "T": "type-changed"}

    for code, path, old_path in rows:
        word = status_word.get(code, code)
        if code in ("R", "C") and old_path is not None:
            print(f"{code}  {rel(old_path)} -> {rel(path)}  ({word})")
        else:
            print(f"{code}  {rel(path)}  ({word})")

        new_links = sorted(inbound.get(path.resolve(), []),
                           key=lambda l: (rel(l.source), l.line))
        old_links = []
        if old_path is not None and old_path != path:
            old_links = sorted(inbound.get(old_path.resolve(), []),
                               key=lambda l: (rel(l.source), l.line))

        if code == "D":
            broken = old_links or new_links
            if broken:
                for link in broken:
                    print(f"     {rel(link.source)}:{link.line} (broken)")
            else:
                print("     no inbound doc links")
        elif code in ("R", "C"):
            if old_links:
                print(f"     old path inbound links (broken until rewritten):")
                for link in old_links:
                    print(f"       {rel(link.source)}:{link.line}")
            if new_links:
                print(f"     new path inbound links:")
                for link in new_links:
                    print(f"       {rel(link.source)}:{link.line}")
            if not old_links and not new_links:
                print("     no inbound doc links")
        else:
            if new_links:
                for link in new_links:
                    print(f"     {rel(link.source)}:{link.line}")
            else:
                print("     no inbound doc links")

    print(f"\n{len(rows)} source file(s) changed vs {base}. "
          f"Caller decides which warrant a doc/README update.")
    return 0


_STOP_WORDS = frozenset(
    "the a an of and or but in on at to for with by from is are was were be been "
    "being have has had do does did will would can could should may might that "
    "this these those it its as if then than so not no all any each one two".split()
)


def _src_refs_in_doc(doc: Path) -> set[str]:
    """Return the set of repo-relative `src/*.rs` paths linked from `doc`.

    Test files are excluded — they are call sites for the protocol, not
    participants in it, and they swamp the signal otherwise."""
    out: set[str] = set()
    for link in extract_links(doc):
        try:
            r = link.resolved.relative_to(REPO)
        except ValueError:
            continue
        s = str(r)
        if s.startswith("src/") and r.suffix == ".rs" and "/tests" not in s:
            out.add(s)
    return out


def _comment_lines(text: str, is_rust: bool) -> str:
    """Project a file down to its prose: comment lines for .rs, full body for .md."""
    if not is_rust:
        return text
    return " ".join(
        l.strip().lstrip("/").strip()
        for l in text.splitlines()
        if l.strip().startswith("//")
    )


_REF_CHAIN_RE = re.compile(
    r"\b(primarily described in|see also|covered in|described in|see)\b",
    re.IGNORECASE,
)


def _doc_link_graph(docs: list[Path]) -> dict[str, set[str]]:
    """Build the doc → doc edge set keyed by repo-relative path."""
    doc_set = {rel(d) for d in docs}
    graph: dict[str, set[str]] = defaultdict(set)
    for d in docs:
        for link in extract_links(d):
            try:
                tgt = str(link.resolved.relative_to(REPO))
            except ValueError:
                continue
            if tgt in doc_set and tgt != rel(d):
                graph[rel(d)].add(tgt)
    return graph


def cmd_signals(args: argparse.Namespace) -> int:
    """Emit mechanical doc-abstraction signals as JSON.

    Sections, each meant for the LLM to filter for semantic relevance:
      - co_cited_triples: src/*.rs triples linked from >= --min-docs design docs.
      - backref_density: src/*.rs files linked from many design docs.
      - comment_density_spikes: .rs files whose comment ratio is in the top --top.
      - shared_phrases: N-word prose phrases appearing in >= --min-files files
        (docs + source comments are scanned together).
      - reference_chains: doc → doc edges whose link text matches "described in"
        / "see also" / etc — candidate "this concept lives elsewhere" indirection.
      - unowned_concepts: high-backref src files whose top-mentioning doc holds
        < --owner-threshold of total mentions.
      - doc_hubs: doc-link-graph nodes ranked by (in_degree, out_degree).
    """
    import itertools
    import json
    from collections import Counter

    docs = iter_doc_files()
    srcs = iter_src_files()

    doc_refs = {rel(d): refs for d in docs if (refs := _src_refs_in_doc(d))}

    triples: Counter[tuple[str, ...]] = Counter()
    backrefs: Counter[str] = Counter()
    for refs in doc_refs.values():
        for r in refs:
            backrefs[r] += 1
        if len(refs) <= args.max_refs_per_doc:
            for t in itertools.combinations(sorted(refs), 3):
                triples[t] += 1

    densities: list[tuple[str, int, int, float]] = []
    for s in srcs:
        try:
            lines = s.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        total = len(lines)
        if total < 50:
            continue
        c = sum(1 for l in lines if l.strip().startswith("//"))
        densities.append((rel(s), c, total, c / total))
    densities.sort(key=lambda x: -x[3])

    ngrams: dict[str, set[str]] = defaultdict(set)
    for f in docs + srcs:
        try:
            text = f.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        prose = _comment_lines(text, f.suffix == ".rs")
        # strip link targets `[text](path)` -> `text` so URL/path tokens don't
        # dominate; backtick spans also drop their delimiters
        prose = LINK_RE.sub(r"\1", prose).replace("`", " ")
        words = re.findall(r"[a-z]{3,}", prose.lower())
        for i in range(len(words) - args.ngram + 1):
            window = words[i : i + args.ngram]
            # require at least one substantive word so "is the X of the Y"
            # doesn't dominate
            if sum(1 for w in window if w not in _STOP_WORDS and len(w) >= 6) < 2:
                continue
            ngrams[" ".join(window)].add(rel(f))
    shared = sorted(
        ((ng, sorted(paths)) for ng, paths in ngrams.items() if len(paths) >= args.min_files),
        key=lambda x: (-len(x[1]), x[0]),
    )

    graph = _doc_link_graph(docs)
    in_deg: Counter[str] = Counter()
    for src, tgts in graph.items():
        for t in tgts:
            in_deg[t] += 1

    graph_path = REPO / "observe" / "doc_graph.dot"
    graph_path.parent.mkdir(parents=True, exist_ok=True)
    with graph_path.open("w", encoding="utf-8") as gf:
        gf.write("digraph docs {\n  rankdir=LR;\n  node [shape=box];\n")
        for src, tgts in sorted(graph.items()):
            for t in sorted(tgts):
                gf.write(f'  "{src}" -> "{t}";\n')
        gf.write("}\n")

    chains: list[dict] = []
    for d in docs:
        try:
            text = d.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for m in LINK_RE.finditer(text):
            # cue may sit inside the link text OR in the preceding ~60 chars
            window_start = max(0, m.start() - 60)
            window = text[window_start : m.end()]
            cue = _REF_CHAIN_RE.search(window)
            if not cue:
                continue
            tgt = _accept_link(d, m.group(2), is_rust=False)
            if tgt is None:
                continue
            try:
                tgt_rel = str(tgt.relative_to(REPO))
            except ValueError:
                continue
            if tgt_rel.endswith(".md"):
                chains.append({"from": rel(d), "cue": cue.group(0),
                               "to": tgt_rel})

    # unowned_concepts: for each high-backref src file, find the doc that mentions
    # it most. If the leader holds < owner_threshold of total mentions, unowned.
    mention_counts: dict[str, Counter[str]] = defaultdict(Counter)
    for d in docs:
        try:
            text = d.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for src_path in backrefs:
            mention_counts[src_path][rel(d)] = text.count(src_path)
    unowned: list[dict] = []
    for src_path, doc_count in backrefs.most_common():
        if doc_count < args.min_docs:
            continue
        mentions = mention_counts[src_path]
        total = sum(mentions.values())
        if total == 0:
            continue
        top_doc, top_n = mentions.most_common(1)[0]
        share = top_n / total
        if share < args.owner_threshold:
            unowned.append({"file": src_path, "top_doc": top_doc,
                            "top_share": round(share, 3),
                            "total_mentions": total,
                            "doc_count": doc_count})

    co_cited = [(c, list(t)) for t, c in triples.most_common(args.top)
                if c >= args.min_docs]
    top_backrefs = backrefs.most_common(args.top)
    top_density = densities[: args.top]
    top_shared = shared[: args.top]
    top_chains = chains[: args.top]
    top_unowned = unowned[: args.top]
    hub_keys = set(graph) | set(in_deg)
    top_hubs = sorted(
        ((d, in_deg[d], len(graph.get(d, set()))) for d in hub_keys),
        key=lambda x: -(x[1] + x[2]),
    )[: args.top]

    if args.json:
        out = {
            "co_cited_triples": [{"files": fs, "doc_count": c}
                                 for c, fs in co_cited],
            "backref_density": [{"file": f, "doc_count": c}
                                for f, c in top_backrefs],
            "comment_density_spikes": [
                {"file": f, "comments": cc, "total": tt, "ratio": round(rr, 3)}
                for f, cc, tt, rr in top_density
            ],
            "shared_phrases": [{"phrase": ng, "files": paths}
                               for ng, paths in top_shared],
            "reference_chains": top_chains,
            "unowned_concepts": top_unowned,
            "doc_hubs": [{"doc": d, "in": i, "out": o}
                         for d, i, o in top_hubs],
        }
        print(json.dumps(out, indent=2))
        return 0

    def section(title: str) -> None:
        print(f"\n## {title}")

    section(f"co-cited triples (doc_count >= {args.min_docs})")
    for c, fs in co_cited:
        print(f"  {c}  " + "  +  ".join(fs))
    if not co_cited:
        print("  (none)")

    section(f"backref density (top {args.top})")
    for f, c in top_backrefs:
        print(f"  {c:3d}  {f}")

    section(f"comment density spikes (top {args.top})")
    for f, cc, tt, rr in top_density:
        print(f"  {rr:.3f}  {f}  ({cc}/{tt})")

    section(f"shared phrases (>= {args.min_files} files)")
    for ng, paths in top_shared:
        print(f"  {len(paths)}  {ng}")
        for p in paths:
            print(f"      - {p}")

    section("reference chains")
    for c in top_chains:
        print(f"  {c['from']}  --\"{c['cue']}\"-->  {c['to']}")
    if not top_chains:
        print("  (none)")

    section(f"unowned concepts (owner share < {args.owner_threshold})")
    for u in top_unowned:
        print(f"  {u['file']}    {u['doc_count']} docs, "
              f"top={u['top_doc']} @ {u['top_share']:.0%}")

    section("doc hubs (in_degree + out_degree)")
    for d, i, o in top_hubs:
        print(f"  in={i:2d}  out={o:2d}  {d}")
    print()
    return 0


def _src_to_module(rel_path: str) -> str | None:
    """Map a repo-relative src/*.rs path to its cargo-modules module path.

    `src/lib.rs` is the crate root (`koan`); `src/foo/mod.rs` is `koan::foo`;
    everything else is the file path with `/` -> `::` and the `.rs` suffix
    stripped."""
    if not rel_path.startswith("src/") or not rel_path.endswith(".rs"):
        return None
    stem = rel_path[len("src/"):-len(".rs")]
    if stem == "lib":
        return "koan"
    if stem.endswith("/mod"):
        stem = stem[:-len("/mod")]
    return "koan::" + stem.replace("/", "::")


def _parse_dot_edges(dot_path: Path) -> set[frozenset[str]]:
    """Pull every directed edge out of a cargo-modules DOT file and return it as
    an undirected set of frozenset({a, b}) — the gap view doesn't care which
    direction the `uses` arrow points."""
    text = dot_path.read_text(encoding="utf-8")
    edges: set[frozenset[str]] = set()
    for m in re.finditer(r'"([^"]+)"\s*->\s*"([^"]+)"', text):
        a, b = m.group(1), m.group(2)
        if a != b:
            edges.add(frozenset((a, b)))
    return edges


def cmd_gap(args: argparse.Namespace) -> int:
    """Surface src-file pairs the design docs co-cite but the cargo-modules
    graph doesn't structurally couple.

    Companion to `modgraph` and `signals`: modgraph scores a proposed module
    tree on structural cost; this view hunts for *candidates* — concepts
    visible in the prose that the code-graph doesn't yet reflect. High gap =
    "docs see this, the module graph doesn't" = candidate seam. Low gap = docs
    and structure agree, no hidden seam.

    Score: `(docs_co_citing + phrases_shared) / (1 + structural_edges)`,
    where `structural_edges` counts direct cargo-modules edges between the
    two files' modules (either direction) plus their parent-child relation."""
    import itertools
    from collections import Counter

    docs = iter_doc_files()
    srcs = iter_src_files()

    doc_refs = {rel(d): refs for d in docs if (refs := _src_refs_in_doc(d))}

    pair_docs: Counter[frozenset[str]] = Counter()
    for refs in doc_refs.values():
        for a, b in itertools.combinations(sorted(refs), 2):
            pair_docs[frozenset((a, b))] += 1

    pair_phrases: Counter[frozenset[str]] = Counter()
    ngram_files: dict[str, set[str]] = defaultdict(set)
    for f in docs + srcs:
        try:
            text = f.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        prose = _comment_lines(text, f.suffix == ".rs")
        prose = LINK_RE.sub(r"\1", prose).replace("`", " ")
        words = re.findall(r"[a-z]{3,}", prose.lower())
        for i in range(len(words) - args.ngram + 1):
            window = words[i : i + args.ngram]
            if sum(1 for w in window if w not in _STOP_WORDS and len(w) >= 6) < 2:
                continue
            ngram_files[" ".join(window)].add(rel(f))
    for ng, paths in ngram_files.items():
        srcs_in_phrase = [p for p in paths if p.startswith("src/")]
        if len(srcs_in_phrase) < 2:
            continue
        for a, b in itertools.combinations(sorted(srcs_in_phrase), 2):
            pair_phrases[frozenset((a, b))] += 1

    edges = _parse_dot_edges(Path(args.edges))

    def structural_weight(a: str, b: str) -> int:
        ma, mb = _src_to_module(a), _src_to_module(b)
        if ma is None or mb is None:
            return 0
        w = 1 if frozenset((ma, mb)) in edges else 0
        # parent/child in module tree is implicit structural coupling
        if ma.startswith(mb + "::") or mb.startswith(ma + "::"):
            w += 1
        return w

    rows: list[dict] = []
    for pair, doc_count in pair_docs.items():
        if doc_count < args.min_docs:
            continue
        a, b = sorted(pair)
        phrase_count = pair_phrases.get(pair, 0)
        edge_w = structural_weight(a, b)
        gap = (doc_count + phrase_count) / (1 + edge_w)
        rows.append({
            "a": a, "b": b,
            "docs": doc_count, "phrases": phrase_count,
            "edges": edge_w, "gap": round(gap, 3),
        })
    rows.sort(key=lambda r: (-r["gap"], -r["docs"], r["a"], r["b"]))
    rows = rows[: args.top]

    if args.json:
        import json
        print(json.dumps({"gap": rows}, indent=2))
        return 0

    print(f"## doc/code gap (top {args.top}, min-docs={args.min_docs})")
    print(f"## score = (docs + phrases) / (1 + structural_edges)")
    for r in rows:
        print(f"  gap={r['gap']:6.2f}  docs={r['docs']:2d}  "
              f"phrases={r['phrases']:2d}  edges={r['edges']:1d}   "
              f"{r['a']}  +  {r['b']}")
    if not rows:
        print("  (none)")
    return 0


def cmd_check(args: argparse.Namespace) -> int:
    """Run every audit in one pass. The four gates (links, deps, orphans, and the
    derived Next-items list) exit non-zero if any flags an issue. The source-tree
    section is informational — the caller decides which changed files warrant a
    doc update — so it never affects the exit code."""
    base = getattr(args, "base", "master")

    print("## broken links")
    rc_links = _check_links()
    print()

    print("## roadmap dependencies")
    rc_deps = _check_deps()
    print()

    print("## orphaned design/ + roadmap/ docs")
    rc_orphans = _check_orphans()
    print()

    print("## next items (derived from dependency graph)")
    rc_next = _check_next_items()
    print()

    print(f"## source-tree changes vs {base}")
    _report_src_changes(base)

    return max(rc_links, rc_deps, rc_orphans, rc_next)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="doclinks",
        description="Maintain links between docs and source for the koan repo.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_check = sub.add_parser(
        "check",
        help="run every gate: broken links, roadmap dep symmetry, orphaned "
             "docs, derived Next-items list, plus src-tree changes vs a git ref",
    )
    p_check.add_argument(
        "--base", default="master",
        help="git ref the source-tree section diffs against (default: master). "
             "The working tree is compared to this ref, so both committed and "
             "uncommitted edits are surfaced in one pass. Only affects the "
             "informational source-tree section, not the gating sections.",
    )
    p_check.set_defaults(func=cmd_check)

    p_sync = sub.add_parser(
        "sync-next",
        help="regenerate the global 'Next items' list plus each project "
             "README's slice from the roadmap dependency graph",
    )
    p_sync.add_argument(
        "--dry-run", action="store_true",
        help="report the add/remove changes without writing the file",
    )
    p_sync.set_defaults(func=cmd_sync_next)

    p_refs = sub.add_parser("refs", help="list every file that links to <path>")
    p_refs.add_argument("path", help="file path (absolute or relative to cwd)")
    p_refs.set_defaults(func=cmd_refs)

    p_fix_refs = sub.add_parser(
        "fix-refs",
        help="rewrite every link that resolves to OLD so it points at NEW "
             "instead — used after a file move or rename",
    )
    p_fix_refs.add_argument(
        "mapping", nargs="*",
        help="one or more OLD=NEW pairs (repo-relative paths)",
    )
    p_fix_refs.add_argument(
        "--from-file", help="read additional OLD=NEW mappings from a file "
                            "(blank lines and '#' comments allowed)",
    )
    p_fix_refs.add_argument(
        "--dry-run", action="store_true",
        help="report rewrites without writing any files",
    )
    p_fix_refs.add_argument(
        "--keep-text", action="store_true",
        help="preserve each link's visible text; otherwise the visible text is "
             "replaced with the new file's H1 title when the two differ",
    )
    p_fix_refs.set_defaults(func=cmd_fix_refs)

    p_rm = sub.add_parser(
        "rm-roadmap",
        help="delete a roadmap/*.md file and prune inbound dependency / "
             "roadmap/README.md bullets",
    )
    p_rm.add_argument("path", help="path to the roadmap item to delete")
    p_rm.add_argument(
        "--dry-run", action="store_true",
        help="report edits and the deletion without writing any files",
    )
    p_rm.set_defaults(func=cmd_rm_roadmap)

    p_dag = sub.add_parser(
        "dag",
        help="emit a Graphviz DOT digraph of roadmap Requires/Unblocks edges",
    )
    p_dag.set_defaults(func=cmd_dag)

    p_signals = sub.add_parser(
        "signals",
        help="emit mechanical doc-abstraction signals as JSON — co-cited src "
             "triples, backref density, comment-density spikes, shared phrases",
    )
    p_signals.add_argument("--min-docs", type=int, default=3,
                           help="min docs co-citing a triple (default: 3)")
    p_signals.add_argument("--min-files", type=int, default=3,
                           help="min files sharing a phrase (default: 3)")
    p_signals.add_argument("--ngram", type=int, default=5,
                           help="phrase length in words (default: 5)")
    p_signals.add_argument("--max-refs-per-doc", type=int, default=25,
                           help="skip docs with > N src refs in triple scoring "
                                "(default: 25 — caps execution-model.md noise)")
    p_signals.add_argument("--top", type=int, default=30,
                           help="rows per ranked section (default: 30)")
    p_signals.add_argument("--json", action="store_true",
                           help="emit JSON instead of human-readable text "
                                "(text is the default for direct reading)")
    p_signals.add_argument("--owner-threshold", type=float, default=0.5,
                           help="min share of mentions one doc must hold to be "
                                "considered the concept's owner (default: 0.5)")
    p_signals.set_defaults(func=cmd_signals)

    p_gap = sub.add_parser(
        "gap",
        help="rank src-file pairs by doc-coupling minus structural-coupling — "
             "concepts the design docs see that the cargo-modules graph doesn't",
    )
    p_gap.add_argument("--edges", default="observe/modules.dot",
                       help="cargo-modules DOT graph (default: observe/modules.dot)")
    p_gap.add_argument("--min-docs", type=int, default=2,
                       help="min docs co-citing a pair (default: 2)")
    p_gap.add_argument("--ngram", type=int, default=5,
                       help="phrase length in words for shared-phrase scoring "
                            "(default: 5)")
    p_gap.add_argument("--top", type=int, default=30,
                       help="rows to emit (default: 30)")
    p_gap.add_argument("--json", action="store_true",
                       help="emit JSON instead of human-readable text")
    p_gap.set_defaults(func=cmd_gap)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
