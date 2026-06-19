"""What-if structural rewrites of a DOT graph + mirrored src/ tree.

Two granularities, so a proposed refactor can be scored before touching real
files:

  * `cmd_module` — rename whole modules (text-level token rewrite of the DOT,
    plus a mirrored src/ tree under the new paths; colliding merges concatenate).
  * `cmd_item` — SCIP-driven extraction of individual items into new modules,
    with a surgical edge diff and brace-balanced body transplant.
"""
from __future__ import annotations

import argparse
import re
import shutil
import sys
from collections import defaultdict
from pathlib import Path

import reexport
from graph import parse_dot
from modules import module_to_file, relpath_to_module
from scip import parse_scip, scip_symbol_to_path

REPO = Path(__file__).resolve().parents[2]

MODULE_TOKEN = re.compile(r'"([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z0-9_]+)*)"')


# ====================================================================
# module mode — whole-module renames
# ====================================================================

def parse_rename(s: str) -> tuple[str, str]:
    if "=" not in s:
        raise argparse.ArgumentTypeError(f"expected OLD=NEW, got {s!r}")
    old, new = s.split("=", 1)
    old, new = old.strip(), new.strip()
    if not old or not new:
        raise argparse.ArgumentTypeError(f"empty side in rename {s!r}")
    return old, new


def load_rename_file(path: Path) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        out.append(parse_rename(line))
    return out


def rewrite_path(name: str, renames: list[tuple[str, str]]) -> str:
    for old, new in renames:
        if name == old:
            return new
        if name.startswith(old + "::"):
            return new + name[len(old):]
    return name


def rewrite_edges(dot_in: Path, dot_out: Path,
                  renames: list[tuple[str, str]]) -> str:
    text = dot_in.read_text()
    new_text = MODULE_TOKEN.sub(
        lambda m: f'"{rewrite_path(m.group(1), renames)}"', text
    )
    dot_out.write_text(new_text)
    return text


def _discover_in_dot(dot_text: str, root: str) -> set[str]:
    """Module tokens in the DOT text (edges + node decls) under `root`."""
    prefix = root + "::"
    return {
        m.group(1) for m in MODULE_TOKEN.finditer(dot_text)
        if m.group(1) == root or m.group(1).startswith(prefix)
    }


def mirror_src(dot_text: str, src_root: Path, src_out: Path,
               renames: list[tuple[str, str]], root: str) -> int:
    if src_out.exists():
        shutil.rmtree(src_out)
    copied = 0
    written: set[Path] = set()
    # Sorted so colliding merges concatenate in a stable order — file_loc's
    # #[cfg(test)] brace-skipping is order-sensitive, so an unstable order
    # makes a merge's production-LOC (and thus its score) nondeterministic.
    for mod in sorted(_discover_in_dot(dot_text, root)):
        src_path = module_to_file(mod, src_root)
        if src_path is None:
            continue
        new_mod = rewrite_path(mod, renames)
        parts_new = new_mod.split("::")[1:]
        if not parts_new:
            target = src_out / "lib.rs"
        elif src_path.name == "mod.rs":
            target = src_out.joinpath(*parts_new, "mod.rs")
        else:
            target = src_out.joinpath(*parts_new).with_suffix(".rs")
        target.parent.mkdir(parents=True, exist_ok=True)
        # Multiple source modules can rename onto the same target (a merge);
        # concatenate their bodies so the merged file's LOC — and thus the
        # size term — reflects the real total instead of only the last copy.
        if target in written:
            with open(target, "a") as out:
                out.write("\n")
                out.write(src_path.read_text())
        else:
            shutil.copy(src_path, target)
            written.add(target)
        copied += 1
    return copied


def cmd_module(args: argparse.Namespace) -> int:
    renames: list[tuple[str, str]] = list(args.rename)
    if args.rename_file:
        renames.extend(load_rename_file(args.rename_file))
    if not renames:
        print("error: at least one --rename or --rename-file entry required",
              file=sys.stderr)
        return 2

    args.output_edges.parent.mkdir(parents=True, exist_ok=True)
    dot_text = rewrite_edges(args.edges, args.output_edges, renames)
    print(f"wrote {args.output_edges}")

    if args.output_src:
        copied = mirror_src(dot_text, args.src_root, args.output_src,
                            renames, args.root)
        print(f"mirrored {copied} file(s) to {args.output_src}")
    return 0


# ====================================================================
# item mode — SCIP-driven item-level extraction
# ====================================================================

class MoveSpec:
    def __init__(self, old_path: str, new_module: str, is_delete: bool = False):
        self.old_path = old_path
        self.new_module = new_module
        self.is_delete = is_delete
        # filled in by resolve_moves()
        self.source_module: str = ""
        self.source_file: str = ""
        self.def_line: int = -1
        self.symbol: str = ""
        # filled in by compute_body_ends() (line after the item's last line);
        # delimits the body span whose refs are the item's outbound edges.
        self.body_end: int = -1


def parse_move(s: str) -> MoveSpec:
    if "=" not in s:
        raise argparse.ArgumentTypeError(f"expected OLD=NEW, got {s!r}")
    old, new = s.split("=", 1)
    old, new = old.strip(), new.strip()
    if not old or not new:
        raise argparse.ArgumentTypeError(f"empty side in move {s!r}")
    return MoveSpec(old, new)


def parse_delete(s: str) -> MoveSpec:
    s = s.strip()
    if not s:
        raise argparse.ArgumentTypeError("--delete expects an item path")
    return MoveSpec(s, new_module="", is_delete=True)


def resolve_moves(moves: list[MoveSpec],
                  docs: dict[str, dict]) -> list[str]:
    """Fill in source_module / source_file / def_line / symbol per move.
    Returns a list of error messages; empty on success."""
    path_index: dict[str, list[tuple[str, str, int]]] = defaultdict(list)
    for relpath, doc in docs.items():
        for sym, line in doc["defs"]:
            path = scip_symbol_to_path(sym)
            if path is None:
                continue
            path_index[path].append((sym, relpath, line))

    errors: list[str] = []
    for mv in moves:
        hits = path_index.get(mv.old_path, [])
        if not hits:
            errors.append(f"{mv.old_path}: no SCIP definition found")
            continue
        if len({h[1] for h in hits}) > 1:
            errors.append(f"{mv.old_path}: ambiguous — defined in "
                          f"{sorted({h[1] for h in hits})}")
            continue
        sym, relpath, line = hits[0]
        mod = relpath_to_module(relpath)
        if mod is None:
            errors.append(f"{mv.old_path}: definition in non-src file {relpath}")
            continue
        mv.symbol = sym
        mv.source_module = mod
        mv.source_file = relpath
        mv.def_line = line
    return errors


def _resolve_dep(sym: str,
                 moved_syms: dict[str, "MoveSpec"],
                 source_facade: dict[str, str],
                 known: set[str],
                 *, redirect_moved: bool) -> str | None:
    """Resolve a body-ref SCIP symbol to the module its outbound edge targets,
    seen from the source module that currently holds the referencing item.

    `redirect_moved` selects the granularity:
      * True (add side) — a ref to a moved sibling targets its *new* module, so a
        reference between two members of one group becomes a self-edge the caller
        drops.
      * False (canonical side) — every symbol resolves to its pre-move module, so
        a removal names the edge actually present in the canonical DOT.

    A name the source module imports through a re-export facade
    (`source_facade`, from `reexport.attributions`) resolves to that facade,
    matching the canonical DOT; otherwise the symbol's def-site path collapses to
    its deepest known module. Returns None for locals, external-crate refs, and
    paths under no known module."""
    if redirect_moved:
        mv = moved_syms.get(sym)
        if mv is not None and not mv.is_delete:
            return mv.new_module
    path = scip_symbol_to_path(sym)
    if path is None:
        return None
    name = path.rsplit("::", 1)[-1]
    if name in source_facade:
        return source_facade[name]
    return reexport.written_module(path.split("::"), known)


def diff_edges(docs: dict[str, dict],
               moves: list[MoveSpec],
               attribution: dict[str, list[tuple[str, str]]],
               known: set[str],
               ) -> tuple[set[tuple[str, str]], set[tuple[str, str]]]:
    """Return (edges_to_add, edges_to_remove) for the proposed moves, relative
    to the re-export-corrected canonical DOT. Models both sides of a move:

    Inbound — the edges a moved item's *consumers* carry.
      Add: for each consumer M that references a moved item, M -> NEW_MOD (SCIP
           refs say *whether* M uses the item; this is precise).
      Remove: the canonical DOT attributes M's import to the *facade* module M
           literally named (`reexport.correct`), not the item's def-site. So a
           removal must target that facade. `attribution[M]` is the per-leaf
           (written_module, item_name) list; we drop M -> FACADE only when, after
           the move, no item M imports still enters FACADE — i.e. every leaf at
           FACADE is a moved item redirected away from it.

      This subsumes the plain deep-import case (where the written facade equals
      the def-site, so the dropped edge matches today's behavior) and the
      re-exported case (facade != def-site) that a def-site removal silently
      missed.

    Outbound — the edges a moved item's *own body* carries (the symmetric case,
      with the source module standing in for the consumer). The refs inside the
      item's body span [def_line, body_end) become NEW_MOD -> dep edges, facade-
      corrected from the source module's imports; a source_module -> dep edge is
      dropped only when no item still in source_module references dep. This is
      what makes a relocated dependency-bearing item score its reconstituting
      back-edges instead of looking strictly cheaper than reality.

    `known` is the canonical DOT's module node set, used to collapse a def-site
    path to the deepest module that is actually a graph node."""
    moved_syms = {mv.symbol: mv for mv in moves if mv.symbol}

    # A moving item's own body refs leave the source module with it, so they are
    # the outbound side's concern; excluding them here keeps the inbound pass from
    # charging the source module an edge to a sibling's new module (the intra-group
    # case). Spans are only known once compute_body_ends has run; an unset body_end
    # (e.g. a unit test driving diff_edges directly) leaves the inbound pass intact.
    spans_by_file: dict[str, list[tuple[int, int]]] = defaultdict(list)
    for mv in moves:
        if mv.symbol and not mv.is_delete and mv.body_end >= 0:
            spans_by_file[mv.source_file].append((mv.def_line, mv.body_end))

    mod_refs: dict[str, set[str]] = defaultdict(set)
    for relpath, doc in docs.items():
        src_mod = relpath_to_module(relpath)
        if src_mod is None:
            continue
        spans = spans_by_file.get(relpath, [])
        for sym, line in doc["refs"]:
            if any(start <= line < end for start, end in spans):
                continue
            mod_refs[src_mod].add(sym)

    add: set[tuple[str, str]] = set()
    remove: set[tuple[str, str]] = set()
    for mod, syms in mod_refs.items():
        referenced = [moved_syms[s] for s in syms if s in moved_syms]
        if not referenced:
            continue
        # Map each moved item this consumer references by its simple name — the
        # segment a `use` leaf ends in, which is how `attribution` keys it.
        moved_names: dict[str, MoveSpec] = {
            mv.old_path.rsplit("::", 1)[-1]: mv for mv in referenced
        }
        for mv in referenced:
            if not mv.is_delete and mv.new_module != mod:
                add.add((mod, mv.new_module))

        facade_items: dict[str, list[str]] = defaultdict(list)
        for facade, name in attribution.get(mod, []):
            facade_items[facade].append(name)
        for facade, names in facade_items.items():
            if facade == mod:
                continue
            loses = any(name in moved_names and moved_names[name].new_module != facade
                        for name in names)
            if not loses:
                continue
            stays = any(name not in moved_names or moved_names[name].new_module == facade
                        for name in names)
            if not stays:
                remove.add((mod, facade))

    # ---- outbound: the dependency edges each moved item carries with it ----
    real_moves = [mv for mv in moves
                  if mv.symbol and not mv.is_delete and mv.body_end >= 0]
    by_source: dict[str, list[MoveSpec]] = defaultdict(list)
    for mv in real_moves:
        by_source[mv.source_file].append(mv)

    for source_file, group in by_source.items():
        source_module = group[0].source_module
        source_facade = {name: facade
                         for facade, name in attribution.get(source_module, [])}
        all_refs = docs.get(source_file, {}).get("refs", [])

        # The moved items' body refs reconstitute as new_module -> dep edges;
        # their canonical (pre-move) deps are the source_module edges at risk.
        moved_canonical_deps: set[str] = set()
        surviving_deps: set[str] = set()
        for sym, line in all_refs:
            owner = next((mv for mv in group
                          if mv.def_line <= line < mv.body_end), None)
            if owner is None:
                dep = _resolve_dep(sym, moved_syms, source_facade, known,
                                   redirect_moved=False)
                if dep is not None:
                    surviving_deps.add(dep)
                continue
            dep_add = _resolve_dep(sym, moved_syms, source_facade, known,
                                   redirect_moved=True)
            if dep_add is not None and dep_add != owner.new_module:
                add.add((owner.new_module, dep_add))
            dep_canon = _resolve_dep(sym, moved_syms, source_facade, known,
                                     redirect_moved=False)
            if dep_canon is not None and dep_canon != source_module:
                moved_canonical_deps.add(dep_canon)

        # A source edge survives iff some item still in source_module justifies
        # it; one no surviving item references is dropped.
        for dep in moved_canonical_deps - surviving_deps:
            remove.add((source_module, dep))

    return add, remove


_EDGE_RE = re.compile(r'^\s*"([^"]+)"\s*->\s*"([^"]+)"\s*;')


def render_dot_diff(original_dot: str,
                    add: set[tuple[str, str]],
                    remove: set[tuple[str, str]],
                    new_modules: set[str],
                    delete_modules: set[str] = frozenset()) -> str:
    """Surgical edit of the cargo-modules DOT: drop edges in `remove`, plus
    drop every node/edge touching a module in `delete_modules` (used when a
    `--delete-file` orphans the whole module), keep everything else, append
    `add` edges plus stubs for fresh modules."""
    lines = original_dot.splitlines()
    out: list[str] = []
    closing_brace_idx = -1
    seen_nodes: set[str] = set()
    # Any edge whose source OR destination is a deleted module is dropped,
    # and the deleted module's own node-declaration line is filtered out.
    edge_any_re = re.compile(r'\s*"([^"]+)"\s*->\s*"([^"]+)"')
    node_decl_re = re.compile(r'^\s*"([^"]+)"\s*\[')
    for line in lines:
        em = edge_any_re.match(line)
        if em:
            src, dst = em.group(1), em.group(2)
            if src in delete_modules or dst in delete_modules:
                continue
            seen_nodes.add(src)
            seen_nodes.add(dst)
            if (src, dst) in remove:
                continue
        else:
            nm = node_decl_re.match(line)
            if nm and nm.group(1) in delete_modules:
                continue
        if line.strip() == "}":
            closing_brace_idx = len(out)
        out.append(line)
    extra: list[str] = []
    # Match cargo-modules' edge attributes so the scorer's edge match (which
    # requires [label="uses"]) and direct_children (which derives the tree
    # from owns edges + node names) both see the new module.
    uses_attrs = '[label="uses", color="#7f7f7f", style="dashed"] [constraint=false]; // "uses" edge'
    owns_attrs = '[label="owns", color="#000000", style="solid"] [constraint=true]; // "owns" edge'
    for nm in sorted(new_modules - seen_nodes):
        extra.append(f'    "{nm}" [label="{nm}"];')
        parent = nm.rsplit("::", 1)[0] if "::" in nm else None
        if parent:
            extra.append(f'    "{parent}" -> "{nm}" {owns_attrs}')
    for src, dst in sorted(add):
        extra.append(f'    "{src}" -> "{dst}" {uses_attrs}')
    if closing_brace_idx >= 0:
        out[closing_brace_idx:closing_brace_idx] = extra
    else:
        out.extend(extra)
        out.append("}")
    return "\n".join(out) + "\n"


def find_item_end(lines: list[str], def_line: int) -> int:
    """Return the line index *after* the last line of the item starting at
    `def_line`. Finds the first `{` at or after def_line, then walks forward
    matching braces (skipping `//` line comments, `/* ... */` block comments,
    and string/char literals). Falls back to def_line+1 if no `{` shows up
    within 64 lines (covers `const X = ...;` and other one-liners)."""
    n = len(lines)
    open_line, open_col = -1, -1
    for i in range(def_line, min(def_line + 64, n)):
        for j, ch in enumerate(_strip_for_brace_scan(lines[i])):
            if ch == "{":
                open_line, open_col = i, j
                break
        if open_line >= 0:
            break
    if open_line < 0:
        return def_line + 1
    depth = 0
    in_block_comment = False
    for i in range(open_line, n):
        text = lines[i]
        j = open_col if i == open_line else 0
        line = _strip_for_brace_scan(text, in_block_comment=in_block_comment,
                                     skip_up_to=j)
        for ch in line:
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    return i + 1
    return n


def _strip_for_brace_scan(line: str, in_block_comment: bool = False,
                          skip_up_to: int = 0) -> str:
    """Return `line` with string/char literals + `//` and block comments
    collapsed to spaces, so a brace scan only sees real braces."""
    out: list[str] = []
    i = 0
    n = len(line)
    while i < n:
        if i < skip_up_to:
            out.append(" ")
            i += 1
            continue
        c = line[i]
        if in_block_comment:
            if c == "*" and i + 1 < n and line[i + 1] == "/":
                in_block_comment = False
                out.append(" ")
                i += 2
                continue
            out.append(" ")
            i += 1
            continue
        if c == "/" and i + 1 < n and line[i + 1] == "/":
            break
        if c == "/" and i + 1 < n and line[i + 1] == "*":
            in_block_comment = True
            i += 2
            continue
        if c == '"':
            j = i + 1
            while j < n:
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == '"':
                    j += 1
                    break
                j += 1
            out.append(" ")
            i = j
            continue
        if c == "'":
            j = i + 1
            while j < min(n, i + 5):
                if line[j] == "'":
                    j += 1
                    break
                j += 1
            else:
                j = i + 1
            out.append(" ")
            i = j
            continue
        out.append(c)
        i += 1
    return "".join(out)


def compute_body_ends(moves: list[MoveSpec], src_root: Path) -> None:
    """Fill `mv.body_end` (the line index after each item's last line) by
    brace-scanning its source file. Done once here so `find_item_end` runs a
    single time per move and the same span feeds both the outbound edge diff
    (`diff_edges`) and the body transplant (`write_item_mirror`)."""
    by_file: dict[str, list[MoveSpec]] = defaultdict(list)
    for mv in moves:
        if mv.source_file and mv.def_line >= 0:
            by_file[mv.source_file].append(mv)
    for source_rel, group in by_file.items():
        rel_under_src = source_rel[len("src/"):]
        path = src_root / rel_under_src
        lines = (path.read_text(encoding="utf-8").splitlines()
                 if path.exists() else [])
        for mv in group:
            mv.body_end = (find_item_end(lines, mv.def_line)
                           if mv.def_line < len(lines) else mv.def_line + 1)


def write_item_mirror(src_root: Path, mirror_root: Path,
                      moves: list[MoveSpec]) -> None:
    """Copy `src_root` to `mirror_root`, then transplant each moved item's
    body from source to destination. New destination files are created."""
    if mirror_root.exists():
        shutil.rmtree(mirror_root)
    shutil.copytree(src_root, mirror_root)

    moves_by_source: dict[str, list[MoveSpec]] = defaultdict(list)
    for mv in moves:
        moves_by_source[mv.source_file].append(mv)

    for source_rel, group in moves_by_source.items():
        rel_under_src = source_rel[len("src/"):]
        source_path = mirror_root / rel_under_src
        if not source_path.exists():
            continue
        lines = source_path.read_text(encoding="utf-8").splitlines(keepends=True)

        extracted: list[tuple[MoveSpec, int, int, list[str]]] = []
        for mv in group:
            if mv.def_line >= len(lines):
                continue
            end = mv.body_end if mv.body_end >= 0 else find_item_end(lines, mv.def_line)
            extracted.append((mv, mv.def_line, end, lines[mv.def_line:end]))

        for mv, start, end, _blob in sorted(extracted, key=lambda x: -x[1]):
            del lines[start:end]
        source_path.write_text("".join(lines), encoding="utf-8")

        dst_blobs: dict[Path, list[str]] = defaultdict(list)
        for mv, _start, _end, blob in extracted:
            if mv.is_delete:
                continue
            dst_rel = mv.new_module.split("::")[1:]
            dst_path = mirror_root.joinpath(*dst_rel).with_suffix(".rs")
            dst_blobs[dst_path].extend(blob)
        for dst_path, blob in dst_blobs.items():
            dst_path.parent.mkdir(parents=True, exist_ok=True)
            existing = dst_path.read_text(encoding="utf-8") if dst_path.exists() else ""
            dst_path.write_text(existing + "".join(blob), encoding="utf-8")


def cmd_item(args: argparse.Namespace) -> int:
    deletes = getattr(args, "delete_specs", []) or []
    delete_files = [Path(p) for p in getattr(args, "delete_files", []) or []]
    if not args.move and not deletes and not delete_files:
        print("error: at least one --move, --delete, or --delete-file required",
              file=sys.stderr)
        return 2
    args.move = list(args.move) + deletes

    docs = parse_scip(args.scip)
    errors = resolve_moves(args.move, docs)
    if errors:
        for e in errors:
            print(f"error: {e}", file=sys.stderr)
        return 1
    compute_body_ends(args.move, args.src_root)

    # The canonical DOT is re-export-corrected, so removals must target the
    # facade module each consumer literally imports through, not the def-site.
    # `attribution` carries that mapping; `known` is the DOT's module node set
    # (item mode is koan-specific throughout, matching resolve_moves' default).
    known = parse_dot(args.edges).nodes
    attribution = reexport.attributions(known, args.src_root, "koan")
    add, remove = diff_edges(docs, args.move, attribution, known)
    original_dot = args.edges.read_text(encoding="utf-8")
    new_modules = {mv.new_module for mv in args.move if not mv.is_delete}

    delete_modules: set[str] = set()
    for f in delete_files:
        rel = str(f) if not f.is_absolute() else str(f.relative_to(REPO))
        if not rel.startswith("src/"):
            print(f"error: --delete-file must point under src/, got {rel}",
                  file=sys.stderr)
            return 1
        mod = relpath_to_module(rel)
        if mod is None:
            print(f"error: --delete-file {rel} has no module path",
                  file=sys.stderr)
            return 1
        delete_modules.add(mod)

    args.output_edges.write_text(
        render_dot_diff(original_dot, add, remove, new_modules, delete_modules),
        encoding="utf-8",
    )
    write_item_mirror(args.src_root, args.output_src, args.move)

    for f in delete_files:
        rel = f if not f.is_absolute() else f.relative_to(REPO)
        rel_under_src = str(rel)[len("src/"):]
        mirror_path = args.output_src / rel_under_src
        if mirror_path.exists():
            mirror_path.unlink()

    print(f"wrote {args.output_edges}")
    print(f"mirrored src tree to {args.output_src}")
    for mv in args.move:
        if mv.is_delete:
            print(f"  deleted {mv.old_path}  ({mv.source_file}:{mv.def_line})")
        else:
            print(f"  moved {mv.old_path}  ({mv.source_file}:{mv.def_line})  "
                  f"-> {mv.new_module}")
    for f in delete_files:
        print(f"  deleted file {f}")
    return 0
