#!/usr/bin/env python3
"""Apply module renames to a cargo-modules DOT graph and mirror the source
tree to a new location, so a proposed module refactor can be scored with
`modgraph.py --root <module>` without actually moving files.

Each `--rename OLD=NEW` rebinds every module path that equals OLD or
starts with `OLD::`. Renames apply against the original module path only
(not chained), so a chain like `A=B`, `B=C` must be expressed as `A=C`.

Usage:
  python3 tools/modgraph_rewrite.py \\
      --edges /tmp/koan.dot \\
      --src-root src \\
      --output-edges /tmp/koan_proposed.dot \\
      --output-src /tmp/koan_proposed_src \\
      --rename koan::parse::kexpression=koan::ast \\
      --rename koan::execute=koan::dispatch::execute

Then score the proposal:
  python3 tools/modgraph.py --edges /tmp/koan_proposed.dot --root koan \\
                            --src-root /tmp/koan_proposed_src

Renames may also be read from a file, one `OLD=NEW` per line (blank
lines and `#` comments are skipped):
  python3 tools/modgraph_rewrite.py ... --rename-file proposal.txt
"""
from __future__ import annotations

import argparse
import re
import shutil
from pathlib import Path

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


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("--edges", required=True, type=Path,
                    help="cargo-modules DOT input")
    ap.add_argument("--output-edges", required=True, type=Path,
                    help="rewritten DOT output")
    ap.add_argument("--src-root", type=Path, default=Path("src"),
                    help="source root to mirror (default: src)")
    ap.add_argument("--output-src", type=Path,
                    help="where to mirror src/ under renamed paths "
                         "(default: skip mirror)")
    ap.add_argument("--root", default="koan",
                    help="crate root module name (default: koan)")
    ap.add_argument("--rename", action="append", type=parse_rename, default=[],
                    metavar="OLD=NEW",
                    help="module rename; repeatable")
    ap.add_argument("--rename-file", type=Path,
                    help="file of OLD=NEW lines, one per rename")
    args = ap.parse_args()

    renames: list[tuple[str, str]] = list(args.rename)
    if args.rename_file:
        renames.extend(load_rename_file(args.rename_file))
    if not renames:
        ap.error("at least one --rename or --rename-file entry required")

    args.output_edges.parent.mkdir(parents=True, exist_ok=True)
    dot_text = rewrite_edges(args.edges, args.output_edges, renames)
    print(f"wrote {args.output_edges}")

    if args.output_src:
        copied = mirror_src(dot_text, args.src_root, args.output_src,
                            renames, args.root)
        print(f"mirrored {copied} file(s) to {args.output_src}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
