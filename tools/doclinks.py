#!/usr/bin/env python3
"""Maintain links between docs and source for the koan repo.

Subcommands:
  check                report markdown links whose target does not exist
  deps                 report Requires/Unblocks asymmetries in roadmap/
  orphans              report design/ and roadmap/ files no other file links to
  refs <path>          list every file that links to <path>
  rewrite OLD=NEW ...  apply path-mapping rewrites to every matching link
  rm-roadmap <path>    delete a roadmap/*.md file and prune inbound bullets
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

MD_GLOBS = ("*.md", "design/*.md", "roadmap/*.md")
SRC_GLOBS = ("src/**/*.rs",)

# [text](target) — target stops at the first ')' that isn't escaped, no nesting,
# no whitespace, no '#' fragment included in the resolved path.
LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)\s]+)\)")


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


def extract_links(path: Path) -> list[Link]:
    """Pull every [text](target) link out of a file.

    For .rs files we only consider lines that look like doc comments (`//!` or `///`)
    or ordinary `//` comments — code-string literals containing `[x](y)` are rare and
    not worth special-casing.
    """
    out: list[Link] = []
    is_rust = path.suffix == ".rs"
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return out
    for lineno, line in enumerate(text.splitlines(), start=1):
        if is_rust and "//" not in line:
            continue
        for m in LINK_RE.finditer(line):
            text_part, target = m.group(1), m.group(2)
            # strip URL fragment and query for filesystem resolution
            fs_part = target.split("#", 1)[0].split("?", 1)[0]
            if not fs_part or fs_part.startswith(("http://", "https://", "mailto:")):
                continue
            # rustdoc intra-doc links (`super::foo`, `crate::a::b`) aren't paths.
            if is_rust and "::" in fs_part:
                continue
            resolved = (path.parent / fs_part).resolve()
            out.append(Link(path, lineno, text_part, target, resolved))
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


# ---------- check ----------

def cmd_check(_args: argparse.Namespace) -> int:
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


# ---------- deps ----------

DEP_HEADER_RE = re.compile(r"^\*\*(Requires|Unblocks):\*\*\s*$")
NONE_RE = re.compile(r"\bnone\b", re.IGNORECASE)


def parse_dep_section(path: Path) -> tuple[set[str], set[str]]:
    """Return (requires, unblocks) — sets of roadmap basenames (e.g. 'traits.md').

    Reads only the **Dependencies** section. A section ends at the next blank line
    that is followed by non-list content, or at the next `**Header:**` line, or EOF.
    Targets outside roadmap/ (e.g. design/foo.md) are ignored — only intra-roadmap
    edges have a symmetric partner.
    """
    requires: set[str] = set()
    unblocks: set[str] = set()
    text = path.read_text(encoding="utf-8")

    in_deps = False
    current: set[str] | None = None
    for line in text.splitlines():
        if line.strip().lower().startswith("## dependencies"):
            in_deps = True
            continue
        if not in_deps:
            continue
        # next top-level header ends the section
        if line.startswith("## "):
            break
        m = DEP_HEADER_RE.match(line.strip())
        if m:
            kind = m.group(1).lower()
            current = requires if kind == "requires" else unblocks
            continue
        if current is None:
            # text on the **Requires:** / **Unblocks:** line itself, e.g.
            # "**Requires:** none." — handle by re-scanning the line.
            continue
        for m in LINK_RE.finditer(line):
            target = m.group(2).split("#", 1)[0]
            # only intra-roadmap links — paths with no slash, ending in .md
            if "/" in target or not target.endswith(".md"):
                continue
            current.add(target)
    return requires, unblocks


def cmd_deps(_args: argparse.Namespace) -> int:
    roadmap_dir = REPO / "roadmap"
    items = sorted(roadmap_dir.glob("*.md"))
    deps: dict[str, tuple[set[str], set[str]]] = {}
    for f in items:
        deps[f.name] = parse_dep_section(f)

    issues: list[str] = []
    for name, (req, unb) in deps.items():
        for target in sorted(req):
            if target not in deps:
                issues.append(f"{name}: requires '{target}' but file does not exist")
                continue
            if name not in deps[target][1]:
                issues.append(
                    f"{name} requires {target}, but {target} does not list {name} under Unblocks"
                )
        for target in sorted(unb):
            if target not in deps:
                issues.append(f"{name}: unblocks '{target}' but file does not exist")
                continue
            if name not in deps[target][0]:
                issues.append(
                    f"{name} unblocks {target}, but {target} does not list {name} under Requires"
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

def cmd_orphans(_args: argparse.Namespace) -> int:
    targets: list[Path] = []
    for sub in ("design", "roadmap"):
        targets.extend((REPO / sub).glob("*.md"))
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
    header line and end is exclusive (next `## ` or EOF). Case-insensitive on the
    header text.
    """
    target = f"## {header}".lower()
    n = len(lines)
    for i, line in enumerate(lines):
        if line.strip().lower().startswith(target):
            j = i + 1
            while j < n and not lines[j].startswith("## "):
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


# ---------- rewrite ----------

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


def cmd_rewrite(args: argparse.Namespace) -> int:
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

    by_resolved: dict[Path, tuple[Path, str, str]] = {}
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
        by_resolved[old_resolved] = (new_abs, old_disp, new_disp)

    edits: dict[Path, list[tuple[int, str, str, str, str]]] = defaultdict(list)
    for link in all_links():
        if link.resolved not in by_resolved:
            continue
        new_abs, old_disp, new_disp = by_resolved[link.resolved]
        new_path_part = os.path.relpath(new_abs, link.source.parent)
        _, suffix = _split_target(link.target)
        new_raw = new_path_part + suffix
        old_substr = f"[{link.text}]({link.target})"
        new_substr = f"[{link.text}]({new_raw})"
        if old_substr == new_substr:
            continue
        edits[link.source].append(
            (link.line, old_substr, new_substr, old_disp, new_disp)
        )

    if not edits:
        print("No links matched the given mappings.")
        return 0

    total = 0
    for f in sorted(edits):
        items = edits[f]
        text = f.read_text(encoding="utf-8")
        lines = text.splitlines(keepends=True)
        for lineno, old_sub, new_sub, _, _ in items:
            lines[lineno - 1] = lines[lineno - 1].replace(old_sub, new_sub, 1)
        if not args.dry_run:
            f.write_text("".join(lines), encoding="utf-8")
        verb = "would rewrite" if args.dry_run else "rewrote"
        for lineno, _, _, old_disp, new_disp in items:
            print(f"{rel(f)}:{lineno}: {verb} {old_disp} -> {new_disp}")
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

    # 1. Other roadmap items: prune Dependencies bullets pointing at target.
    for f in sorted(roadmap_dir.glob("*.md")):
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

    # 2. ROADMAP.md: prune bullets in Next items + Open items.
    roadmap_idx = REPO / "ROADMAP.md"
    if roadmap_idx.exists():
        idx_lines = roadmap_idx.read_text(encoding="utf-8").splitlines(keepends=True)
        cur_lines = idx_lines
        idx_removed = 0
        for header in ("next items", "open items"):
            section = find_section(cur_lines, header)
            if not section:
                continue
            kept, removed = remove_matching_bullets(
                cur_lines, section[0] + 1, section[1], target, roadmap_idx.parent,
            )
            if removed:
                cur_lines = cur_lines[:section[0] + 1] + kept + cur_lines[section[1]:]
                idx_removed += removed
        if idx_removed:
            plan.append((roadmap_idx, cur_lines, idx_removed))

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

    if not args.dry_run:
        print("\nNote: design-doc 'Open work' entries, source comments, and the "
              "'What's shipped so far' paragraph are not auto-handled.")
        print("Run `python3 tools/doclinks.py check` to find any remaining stale "
              "references.")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="doclinks",
        description="Maintain links between docs and source for the koan repo.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("check", help="report markdown links whose target does not exist") \
        .set_defaults(func=cmd_check)
    sub.add_parser("deps", help="report Requires/Unblocks asymmetries in roadmap/") \
        .set_defaults(func=cmd_deps)
    sub.add_parser("orphans", help="report design/ and roadmap/ files no other file links to") \
        .set_defaults(func=cmd_orphans)

    p_refs = sub.add_parser("refs", help="list every file that links to <path>")
    p_refs.add_argument("path", help="file path (absolute or relative to cwd)")
    p_refs.set_defaults(func=cmd_refs)

    p_rewrite = sub.add_parser(
        "rewrite",
        help="apply OLD=NEW path mappings to every link that resolves to OLD",
    )
    p_rewrite.add_argument(
        "mapping", nargs="*",
        help="one or more OLD=NEW pairs (repo-relative paths)",
    )
    p_rewrite.add_argument(
        "--from-file", help="read additional OLD=NEW mappings from a file "
                            "(blank lines and '#' comments allowed)",
    )
    p_rewrite.add_argument(
        "--dry-run", action="store_true",
        help="report rewrites without writing any files",
    )
    p_rewrite.set_defaults(func=cmd_rewrite)

    p_rm = sub.add_parser(
        "rm-roadmap",
        help="delete a roadmap/*.md file and prune inbound dependency / "
             "ROADMAP.md bullets",
    )
    p_rm.add_argument("path", help="path to the roadmap item to delete")
    p_rm.add_argument(
        "--dry-run", action="store_true",
        help="report edits and the deletion without writing any files",
    )
    p_rm.set_defaults(func=cmd_rm_roadmap)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
