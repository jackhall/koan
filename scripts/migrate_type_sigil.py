#!/usr/bin/env python3
"""Bulk-migrate Koan source/test fixtures in Rust string literals from `<>` type-param
syntax and `name: <slot>` triples to the Design-B sigil syntax.

Transformations applied INSIDE every "..." or r#?"..."#? literal (multi-line aware):
  List<X>                      -> :(List X)
  Dict<K, V>                   -> :(Dict K V)
  Function<(A, B) -> R>        -> :(Function (A B) -> R)
  name: <Type-shape>           -> name :<Type-shape>     (ascription)
  name: <literal-or-lowercase> -> name = <...>           (named-value pair)

INSIDE-DICT colons (e.g. `{a: 1}` literal-dict syntax) are preserved by detecting an
enclosing `{...}` span in the string body and skipping the colon rewrite within those
ranges. The detector is balanced-brace + escape-aware; it ignores `{` after a backslash
or inside an inner string literal.

Operates per-file: scans each `"..."` or `r#?"..."#?` literal in source order (multi-line
strings, line continuations, and `\n\` joins all included).

Usage:
  python3 scripts/migrate_type_sigil.py <file> [<file>...]
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

_TYPE_SHAPE_OK = re.compile(r"^[A-Z][A-Za-z0-9]*$")


def _is_type_token(tok: str) -> bool:
    if not _TYPE_SHAPE_OK.match(tok):
        return False
    return any(c.islower() for c in tok)


def _rewrite_types(s: str) -> str:
    def fn_repl(m: re.Match) -> str:
        args = m.group(1)
        ret = m.group(2).strip()
        args_clean = re.sub(r"\s*,\s*", " ", args).strip()
        return f":(Function ({args_clean}) -> {ret})"

    prev = None
    out = s
    while prev != out:
        prev = out
        out = re.sub(
            r"Function<\(([^()<>]*)\)\s*->\s*([^<>]+?)>",
            fn_repl,
            out,
        )

    prev = None
    while prev != out:
        prev = out
        out = re.sub(
            r"Dict<([^,<>]+),\s*([^<>]+)>",
            lambda m: f":(Dict {m.group(1).strip()} {m.group(2).strip()})",
            out,
        )

    prev = None
    while prev != out:
        prev = out
        out = re.sub(
            r"List<([^<>]+)>",
            lambda m: f":(List {m.group(1).strip()})",
            out,
        )

    # Generic parameterized type `Name<args>` (single-arg, comma-separated args). Runs
    # last so List / Dict / Function shapes already migrated.
    prev = None
    while prev != out:
        prev = out
        out = re.sub(
            r"([A-Z][A-Za-z0-9]*)<([^<>]+)>",
            lambda m: f":({m.group(1)} {re.sub(r',\s*', ' ', m.group(2)).strip()})",
            out,
        )

    return out


_COLON_TRIPLE = re.compile(
    r"(?P<name>\b[A-Za-z_][A-Za-z_0-9]*)"
    r":\s+"
    r"(?P<rhs>[^\s,()\[\]{}=]+)"
)


def _dict_brace_ranges(body: str) -> list[tuple[int, int]]:
    """Find ranges (in `body`) of every Koan-source `{...}` dict literal so the colon-
    triple rewrite can skip them. Balanced-brace scan that:
    - ignores `{` / `}` after a `\\` (Rust escape).
    - ignores `{` / `}` inside an embedded single-quoted or double-quoted *Koan*-source
      string literal — Koan uses `'...'` and `"..."` for its own strings.
    - tracks brace depth so nested dicts close properly.

    Returns a list of `(start, end)` half-open intervals (`start` = position of `{`,
    `end` = position just past `}`). Used by `_rewrite_colons` to skip matches whose
    span overlaps any dict-frame interval.
    """
    ranges: list[tuple[int, int]] = []
    i = 0
    depth = 0
    starts: list[int] = []
    in_kstr: str | None = None  # `'` or `"` if we're inside an embedded Koan string.
    while i < len(body):
        c = body[i]
        if c == "\\" and i + 1 < len(body):
            # Rust source escape — skip the escaped char so `\"` doesn't toggle in_kstr.
            i += 2
            continue
        if in_kstr is not None:
            if c == in_kstr:
                in_kstr = None
            i += 1
            continue
        if c in ("'", '"'):
            in_kstr = c
            i += 1
            continue
        if c == "{":
            starts.append(i)
            depth += 1
        elif c == "}":
            if starts:
                start = starts.pop()
                ranges.append((start, i + 1))
            if depth > 0:
                depth -= 1
        i += 1
    return ranges


def _in_any_range(pos: int, ranges: list[tuple[int, int]]) -> bool:
    for (s, e) in ranges:
        if s <= pos < e:
            return True
    return False


def _rewrite_colons(s: str) -> str:
    skip = _dict_brace_ranges(s)
    out = []
    pos = 0
    for m in _COLON_TRIPLE.finditer(s):
        if _in_any_range(m.start(), skip):
            continue
        out.append(s[pos:m.start()])
        name = m.group("name")
        rhs = m.group("rhs")
        if rhs.startswith(":"):
            # Already-rewritten type sigil — keep the existing `:` glue, drop the
            # whitespace between name and sigil.
            out.append(f"{name} {rhs}")
        elif _is_type_token(rhs):
            out.append(f"{name} :{rhs}")
        else:
            out.append(f"{name} = {rhs}")
        pos = m.end()
    out.append(s[pos:])
    return "".join(out)


def _rewrite_string_body(body: str) -> str:
    body = _rewrite_types(body)
    body = _rewrite_colons(body)
    return body


# Multi-line aware string-literal scanner. Tokens recognized in source order:
#   1. raw string `r#*"..."#*` with matching hash count (uses DOTALL match).
#   2. regular string `"..."` with `\\` / `\"` / `\<other>` / `\<newline>` escapes.
#   3. line comment `//... \n` (skip without rewriting).
#   4. block comment `/* ... */` (skip without rewriting; non-nesting per Rust).
#   5. char literal `'<single>'` (skip without rewriting).
# Returns the rewritten file text.
def migrate(text: str) -> str:
    out: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        # Block comment.
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            end = text.find("*/", i + 2)
            if end == -1:
                out.append(text[i:])
                break
            out.append(text[i:end + 2])
            i = end + 2
            continue
        # Line comment.
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            end = text.find("\n", i + 2)
            if end == -1:
                out.append(text[i:])
                break
            out.append(text[i:end + 1])
            i = end + 1
            continue
        # Raw string.
        if c == "r" and i + 1 < n and text[i + 1] in ("#", '"'):
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                # Body is everything until the closing `"` followed by `hashes` `#`s.
                close_pattern = '"' + ("#" * hashes)
                end = text.find(close_pattern, j + 1)
                if end == -1:
                    out.append(text[i:])
                    break
                body = text[j + 1:end]
                body2 = _rewrite_string_body(body)
                out.append("r" + ("#" * hashes) + '"' + body2 + close_pattern)
                i = end + len(close_pattern)
                continue
            # Fall through — `r` was just an identifier char.
            out.append(c)
            i += 1
            continue
        # Regular string.
        if c == '"':
            j = i + 1
            while j < n:
                ch = text[j]
                if ch == "\\" and j + 1 < n:
                    j += 2
                    continue
                if ch == '"':
                    break
                j += 1
            if j >= n:
                out.append(text[i:])
                break
            body = text[i + 1:j]
            body2 = _rewrite_string_body(body)
            out.append('"' + body2 + '"')
            i = j + 1
            continue
        # Char literal — skip without rewriting. Pattern: `'<one-char-or-escape>'`.
        if c == "'":
            # Cheap heuristic: find next `'` (handling `\'`). If the span is short
            # (≤4 chars including quotes), treat as char literal; else leave as
            # plain text (Rust lifetimes like `'a` won't have a closing `'` in the
            # short distance, so they fall through to the identifier path).
            j = i + 1
            steps = 0
            while j < n and steps < 4:
                if text[j] == "\\" and j + 1 < n:
                    j += 2
                    steps += 1
                    continue
                if text[j] == "'":
                    break
                j += 1
                steps += 1
            if j < n and text[j] == "'" and (j - i) <= 4:
                out.append(text[i:j + 1])
                i = j + 1
                continue
            out.append(c)
            i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    changed = 0
    for arg in argv[1:]:
        p = Path(arg)
        text = p.read_text()
        new = migrate(text)
        if new != text:
            p.write_text(new)
            changed += 1
            print(f"migrated: {p}")
    print(f"total files changed: {changed}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
