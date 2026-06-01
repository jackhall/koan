#!/usr/bin/env python3
"""Apply structural rewrites to a cargo-modules DOT graph + mirrored src/
tree, so a proposed refactor can be scored with `modgraph.py` without
touching real files.

Two subcommands, differing in granularity:

  module — rename whole modules
    Each `--rename OLD=NEW` rebinds every module path equal to OLD or
    starting with `OLD::`. The mirrored src/ tree uses the new paths.

      python3 tools/modgraph_rewrite.py module \\
          --edges /tmp/koan.dot --src-root src \\
          --output-edges /tmp/koan_proposed.dot \\
          --output-src /tmp/koan_proposed_src \\
          --rename koan::parse::kexpression=koan::ast

  item — extract individual items into new modules
    Uses a SCIP code-index from `rust-analyzer scip` to resolve each
    item's definition site and every module that references it.
    Brace-balanced extraction transplants the item's body to a new
    file; surgical edits to the DOT add/remove only the edges the
    move actually changes.

      rust-analyzer scip <REPO> --output /tmp/koan.scip
      python3 tools/modgraph_rewrite.py item \\
          --scip /tmp/koan.scip --edges /tmp/koan.dot --src-root src \\
          --output-edges /tmp/koan_proposed.dot \\
          --output-src /tmp/koan_proposed_src \\
          --move koan::machine::core::scope::Scope::register_nominal=koan::machine::core::nominal

Then score the proposal under either mode:
  python3 tools/modgraph.py --edges /tmp/koan_proposed.dot --root koan \\
                            --src-root /tmp/koan_proposed_src

`module` mode accepts `--rename-file <path>` (one `OLD=NEW` per line,
`#` for comments). `item` mode requires `rust-analyzer` on PATH
(`rustup component add rust-analyzer`).
"""
from __future__ import annotations

import argparse
import re
import shutil
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

MODULE_TOKEN = re.compile(r'"([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z0-9_]+)*)"')


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


def discover_modules(dot_text: str, root: str) -> set[str]:
    prefix = root + "::"
    return {
        m.group(1) for m in MODULE_TOKEN.finditer(dot_text)
        if m.group(1) == root or m.group(1).startswith(prefix)
    }


def module_to_file(module: str, src_root: Path) -> Path | None:
    parts = module.split("::")[1:]
    if not parts:
        flat = src_root / "lib.rs"
        return flat if flat.exists() else None
    flat = src_root.joinpath(*parts).with_suffix(".rs")
    if flat.exists():
        return flat
    nested = src_root.joinpath(*parts, "mod.rs")
    return nested if nested.exists() else None


def mirror_src(dot_text: str, src_root: Path, src_out: Path,
               renames: list[tuple[str, str]], root: str) -> int:
    if src_out.exists():
        shutil.rmtree(src_out)
    copied = 0
    for mod in discover_modules(dot_text, root):
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
        shutil.copy(src_path, target)
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

# ---------- SCIP wire-format reader (stdlib-only protobuf decoder) ----------

def _varint(buf: bytes, pos: int) -> tuple[int, int]:
    result, shift = 0, 0
    while True:
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


def _iter_fields(buf: bytes):
    """Yield (field_number, wire_type, payload) per protobuf field. Payload is
    bytes for wire 2, int for wire 0, raw N bytes for wire 1/5."""
    pos, n = 0, len(buf)
    while pos < n:
        tag, pos = _varint(buf, pos)
        fnum, wt = tag >> 3, tag & 0x7
        if wt == 0:
            v, pos = _varint(buf, pos)
            yield fnum, wt, v
        elif wt == 2:
            ln, pos = _varint(buf, pos)
            yield fnum, wt, buf[pos:pos + ln]
            pos += ln
        elif wt == 5:
            yield fnum, wt, buf[pos:pos + 4]
            pos += 4
        elif wt == 1:
            yield fnum, wt, buf[pos:pos + 8]
            pos += 8
        else:
            raise ValueError(f"unsupported wire type {wt} at byte {pos}")


# SCIP schema field numbers (from sourcegraph/scip/scip.proto):
#   Index { metadata=1, documents=2, external_symbols=3 }
#   Document { language=4, relative_path=1, occurrences=2, symbols=3 }
#   Occurrence { range=1, symbol=2, symbol_roles=3, ... }
# SymbolRole bits: Definition=1, Import=2, WriteAccess=4, ReadAccess=8
SYMBOL_ROLE_DEFINITION = 1


def parse_scip(path: Path) -> dict[str, dict]:
    """Return {document_relative_path: {"defs": [(symbol, line)],
                                        "refs": [(symbol, line)]}}.
    line is the 0-indexed start line from the SCIP Occurrence range."""
    data = path.read_bytes()
    out: dict[str, dict] = {}
    for fnum, wt, payload in _iter_fields(data):
        if fnum != 2 or wt != 2:
            continue
        doc = _parse_document(payload)
        if doc is None:
            continue
        out[doc[0]] = doc[1]
    return out


def _parse_document(buf: bytes) -> tuple[str, dict] | None:
    relpath = ""
    defs: list[tuple[str, int]] = []
    refs: list[tuple[str, int]] = []
    for fnum, wt, payload in _iter_fields(buf):
        if fnum == 1 and wt == 2:
            relpath = payload.decode("utf-8", "replace")
        elif fnum == 2 and wt == 2:
            occ = _parse_occurrence(payload)
            if occ is None:
                continue
            sym, line, is_def = occ
            (defs if is_def else refs).append((sym, line))
    if not relpath:
        return None
    return relpath, {"defs": defs, "refs": refs}


def _parse_occurrence(buf: bytes) -> tuple[str, int, bool] | None:
    sym = ""
    roles = 0
    start_line = 0
    for fnum, wt, payload in _iter_fields(buf):
        if fnum == 1 and wt == 2:
            # range is packed [start_line, start_col, end_line, end_col]
            # (or 3 ints if start_line == end_line). Plain int32, not zigzag.
            vals = []
            pos = 0
            while pos < len(payload):
                v, pos = _varint(payload, pos)
                vals.append(v)
            if vals:
                start_line = vals[0]
        elif fnum == 2 and wt == 2:
            sym = payload.decode("utf-8", "replace")
        elif fnum == 3 and wt == 0:
            roles = payload
    if not sym:
        return None
    return sym, start_line, bool(roles & SYMBOL_ROLE_DEFINITION)


# ---------- SCIP symbol → koan module-path mapping ----------

# rust-analyzer SCIP symbols look like:
#   "rust-analyzer cargo koan 0.1.0 machine/core/scope/register_nominal()."
# 5 space-separated tokens (scheme/manager/package/version + a space), then
# slash-separated descriptors with type-tagging suffix markers (`.` namespace,
# `#` type, `()` method, etc.). We strip the markers and emit a `koan::a::b`
# path.

_DESC_MARKER_RE = re.compile(r"(\(\)|\(\+(\d+)\))?[.#`!:]?$")
# impl#[`TypeName<...>`]method  — SCIP's wrapper around inherent-impl methods.
# We unwrap it to `TypeName::method` and strip generics/lifetimes so the path
# matches Rust-canonical spelling.
_IMPL_RE = re.compile(r"^impl#\[`?([A-Za-z_][A-Za-z0-9_]*)[^`]*`?\](.+)$")


def scip_symbol_to_path(sym: str, package: str = "koan") -> str | None:
    """Convert a SCIP symbol to `koan::a::b::name`, or None for locals."""
    parts = sym.split(" ", 4)
    if len(parts) < 5 or parts[0] == "local":
        return None
    descriptors = parts[4]
    segments: list[str] = []
    for seg in descriptors.split("/"):
        if not seg:
            continue
        seg = _DESC_MARKER_RE.sub("", seg)
        if seg in ("", "crate"):
            continue
        m = _IMPL_RE.match(seg)
        if m:
            segments.append(m.group(1))
            segments.append(_DESC_MARKER_RE.sub("", m.group(2)))
        else:
            segments.append(seg)
    if not segments:
        return None
    return package + "::" + "::".join(segments)


def relpath_to_module(relpath: str, package: str = "koan") -> str | None:
    """src/machine/core/scope.rs -> koan::machine::core::scope
       src/machine/core/scope/mod.rs -> koan::machine::core::scope"""
    if not relpath.startswith("src/") or not relpath.endswith(".rs"):
        return None
    stem = relpath[len("src/"):-len(".rs")]
    parts = stem.split("/")
    if parts and parts[-1] in ("mod", "lib"):
        parts = parts[:-1]
    if not parts:
        return package
    return package + "::" + "::".join(parts)


# ---------- move resolution ----------

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


# ---------- surgical edge diff ----------

def diff_edges(docs: dict[str, dict],
               moves: list[MoveSpec]) -> tuple[set[tuple[str, str]],
                                               set[tuple[str, str]]]:
    """Return (edges_to_add, edges_to_remove) for the proposed moves,
    relative to the original cargo-modules DOT.

    Add: for each module M that references a moved item, M -> NEW_MOD.
    Remove: M -> OLD_MOD only when M referenced *only* moved items from OLD_MOD
            (i.e. nothing left in OLD_MOD that M still touches)."""
    moved_syms = {mv.symbol: mv for mv in moves if mv.symbol}

    mod_refs: dict[str, set[str]] = defaultdict(set)
    sym_owner: dict[str, str] = {}
    for relpath, doc in docs.items():
        src_mod = relpath_to_module(relpath)
        if src_mod is None:
            continue
        for sym, _ in doc["refs"]:
            mod_refs[src_mod].add(sym)
        for sym, _ in doc["defs"]:
            sym_owner[sym] = src_mod

    add: set[tuple[str, str]] = set()
    remove: set[tuple[str, str]] = set()
    for mod, syms in mod_refs.items():
        moved_seen: set[str] = set()
        for sym in syms:
            mv = moved_syms.get(sym)
            if mv is None:
                continue
            if not mv.is_delete and mv.new_module != mod:
                add.add((mod, mv.new_module))
            moved_seen.add(sym_owner.get(sym, ""))
        for old_mod in moved_seen:
            if old_mod == mod or not old_mod:
                continue
            still_uses_old = any(
                s in syms and s not in moved_syms and sym_owner.get(s) == old_mod
                for s in syms
            )
            if not still_uses_old:
                remove.add((mod, old_mod))
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
    # Match cargo-modules' edge attributes so modgraph's EDGE_RE (which
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


# ---------- brace-balanced item extraction ----------

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
            end = find_item_end(lines, mv.def_line)
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

    add, remove = diff_edges(docs, args.move)
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


# ====================================================================
# CLI
# ====================================================================

def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = ap.add_subparsers(dest="cmd", required=True)

    pm = sub.add_parser("module", help="rename whole modules")
    pm.add_argument("--edges", required=True, type=Path,
                    help="cargo-modules DOT input")
    pm.add_argument("--output-edges", required=True, type=Path,
                    help="rewritten DOT output")
    pm.add_argument("--src-root", type=Path, default=Path("src"),
                    help="source root to mirror (default: src)")
    pm.add_argument("--output-src", type=Path,
                    help="where to mirror src/ under renamed paths "
                         "(default: skip mirror)")
    pm.add_argument("--root", default="koan",
                    help="crate root module name (default: koan)")
    pm.add_argument("--rename", action="append", type=parse_rename, default=[],
                    metavar="OLD=NEW", help="module rename; repeatable")
    pm.add_argument("--rename-file", type=Path,
                    help="file of OLD=NEW lines, one per rename")
    pm.set_defaults(func=cmd_module)

    pi = sub.add_parser("item",
                        help="extract individual items into new modules "
                             "(SCIP-driven)")
    pi.add_argument("--scip", required=True, type=Path,
                    help="path to SCIP index (produced by "
                         "`rust-analyzer scip <repo> --output ...`)")
    pi.add_argument("--edges", required=True, type=Path,
                    help="cargo-modules DOT graph to rewrite")
    pi.add_argument("--src-root", required=True, type=Path,
                    help="src/ directory to mirror")
    pi.add_argument("--output-edges", required=True, type=Path,
                    help="where to write the rewritten DOT")
    pi.add_argument("--output-src", required=True, type=Path,
                    help="where to mirror the modified src/ tree")
    pi.add_argument("--move", action="append", type=parse_move,
                    default=[], metavar="OLD=NEW",
                    help="move item OLD (full koan-path) into module NEW. "
                         "Repeatable.")
    pi.add_argument("--delete", action="append", type=parse_delete,
                    default=[], dest="delete_specs", metavar="ITEM",
                    help="delete item ITEM from its source file. "
                         "Item is removed from the mirrored src tree and "
                         "from the rewritten DOT — no destination module is "
                         "created, and `M -> source_module` uses-edges are "
                         "dropped iff M referenced only deleted items from "
                         "that module. Repeatable. Use to model pure dead-"
                         "code removal (which `--move` cannot).")
    pi.add_argument("--delete-file", action="append", default=[],
                    dest="delete_files", metavar="PATH",
                    help="delete the entire file at PATH (repo-relative, "
                         "must be under src/). Mirror removes the file, and "
                         "the DOT drops the corresponding module node plus "
                         "every edge touching it. Use to model orphaning a "
                         "wrapper module after `--move` migrates its items "
                         "elsewhere — `--move` shrinks the file but can't "
                         "remove the module node or its leftover scaffolding.")
    pi.set_defaults(func=cmd_item)

    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
