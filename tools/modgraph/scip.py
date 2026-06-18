"""Minimal stdlib-only reader for a `rust-analyzer scip` index.

Decodes just enough of the SCIP protobuf to recover, per document, the symbols
defined and referenced there, and maps a SCIP symbol to a Rust-canonical
`koan::a::b::name` path. Used by the item-level rewriter to resolve move targets
and the edges a move changes.
"""
from __future__ import annotations

import re
from pathlib import Path


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
