#!/usr/bin/env python3
"""Maintain links between docs and source for the koan repo.

Subcommands:
  check         report markdown links whose target does not exist
  deps          report Requires/Unblocks asymmetries in roadmap/
  orphans       report design/ and roadmap/ files no other file links to
  refs <path>   list every file that links to <path>
"""

from __future__ import annotations

import argparse
import re
import sys
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

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
