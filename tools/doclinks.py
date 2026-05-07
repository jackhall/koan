#!/usr/bin/env python3
"""Maintain links between docs and source for the koan repo.

Subcommands:
  check                run all four audits in one pass: broken links, roadmap
                       Requires/Unblocks symmetry, orphaned design/ + roadmap/
                       docs, and src/**/*.rs files changed vs a git ref. Exits
                       non-zero if any of the first three (the gates) fail; the
                       source-tree section is informational so the caller can
                       decide which changed files warrant a doc update.
  refs <path>          list every file that links to <path>
  fix-refs OLD=NEW ... rewrite every link that resolves to OLD so it points at
                       NEW instead — used to fix inbound references after a
                       file move or rename
  rm-roadmap <path>    delete a roadmap/*.md file and prune inbound/outbound bullets
  dag                  emit a Graphviz DOT digraph of the roadmap/*.md
                       Requires/Unblocks edges — pipe to `dot -Tpng > dag.png` or
                       paste into an online viewer
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

MD_GLOBS = ("*.md", "design/*.md", "roadmap/*.md")
SRC_GLOBS = ("src/**/*.rs",)

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

def parse_dep_section(path: Path) -> tuple[set[str], set[str]]:
    """Return (requires, unblocks) — sets of roadmap basenames (e.g. 'traits.md').

    Reads only the **Dependencies** section. A section ends at the next h2 header
    (`## ...`) or EOF. Targets outside roadmap/ (e.g. design/foo.md) are ignored —
    only intra-roadmap edges have a symmetric partner.
    """
    requires: set[str] = set()
    unblocks: set[str] = set()
    text = path.read_text(encoding="utf-8")

    in_deps = False
    current: set[str] | None = None
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
            target = m.group(2).split("#", 1)[0]
            # only intra-roadmap links — paths with no slash, ending in .md
            if "/" in target or not target.endswith(".md"):
                continue
            current.add(target)
    return requires, unblocks


def _check_deps() -> int:
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

def _check_orphans() -> int:
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

    if args.dry_run:
        return 0

    print("\nNote: design-doc 'Open work' entries, source comments, and the "
          "'What's shipped so far' paragraph are not auto-handled. Running "
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
    items = sorted(roadmap_dir.glob("*.md"))

    titles: dict[str, str] = {}
    edges: set[tuple[str, str]] = set()
    for f in items:
        titles[f.name] = read_h1_title(f) or f.stem
    for f in items:
        req, unb = parse_dep_section(f)
        for r in req:
            if r in titles:
                edges.add((r, f.name))
        for u in unb:
            if u in titles:
                edges.add((f.name, u))

    def node_id(name: str) -> str:
        return "n_" + re.sub(r"[^A-Za-z0-9]", "_", name[:-3] if name.endswith(".md") else name)

    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace('"', '\\"')

    print("digraph roadmap {")
    print("  rankdir=LR;")
    print("  node [shape=box, style=\"rounded,filled\", fillcolor=\"#f5f5f5\", "
          "fontname=\"Helvetica\"];")
    print("  edge [color=\"#555555\"];")
    for name in sorted(titles):
        print(f'  {node_id(name)} [label="{esc(titles[name])}", '
              f'tooltip="{esc(name)}", URL="{esc(name)}"];')
    for src, dst in sorted(edges):
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
    """Run `git diff --name-status -M <base> -- src` and return rows for `.rs`
    files only. Each row is (status, current_path, old_path), where old_path is
    set for renames/copies and for deletions (so callers can look up inbound
    links to the path that no longer exists)."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(REPO), "diff", "--name-status", "-M",
             base, "--", "src"],
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


def cmd_check(args: argparse.Namespace) -> int:
    """Run all four audits in one pass. The first three (links, deps, orphans)
    are gates: if any of them flag an issue, we exit non-zero. The source-tree
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

    print(f"## source-tree changes vs {base}")
    _report_src_changes(base)

    return max(rc_links, rc_deps, rc_orphans)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="doclinks",
        description="Maintain links between docs and source for the koan repo.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_check = sub.add_parser(
        "check",
        help="run all four audits: broken links, roadmap dep symmetry, "
             "orphaned docs, and src-tree changes vs a git ref",
    )
    p_check.add_argument(
        "--base", default="master",
        help="git ref the source-tree section diffs against (default: master). "
             "The working tree is compared to this ref, so both committed and "
             "uncommitted edits are surfaced in one pass. Only affects the "
             "informational source-tree section, not the gating sections.",
    )
    p_check.set_defaults(func=cmd_check)

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
             "ROADMAP.md bullets",
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

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
