"""Shared rendering for the repo's tracked trend logs.

Three files record one row per commit and print a delta against the newest row:
`observe/coverage.txt` (`tools/coverage.py`), `observe/complexity.txt`
(`tools/modgraph/baseline.py`) and `observe/alloc.txt` (`tools/alloc_audit.py`).
They share this renderer so a reader who has learned to read one has learned to
read all three.

The layout puts every column's name **directly over its own column**, rather than
listing the names in a prose header a reader has to count against the row. The name
row is a comment, so it carries a `# ` prefix; the data rows are indented by the same
two characters to line up under it. Every reader here splits on whitespace after
stripping, so the indent is invisible to them.

A log whose columns come in named groups — `observe/alloc.txt`, two readings per
shape — passes `groups`, which renders a second, wider name row above, each group's
name centred over the columns it spans.

    # managed by ...; newest first, capped to 5 entries
    #                          empty        wide_n10
    # date       short-sha  alloc  sym    alloc  sym
      2026-08-26 7a1f6158+   1047  367     8813  668
"""

from __future__ import annotations


def render(notes: list[str], columns: list[str], rows: list[list[str]],
           groups: list[tuple[str, int]] | None = None) -> str:
    """One trend log's whole text: `notes` as `#` lines, the column names over their
    columns, then `rows`.

    `columns` names every field; each row carries one already-formatted string per
    name. `groups` is `[(name, span), ...]` covering the columns after the leading
    ungrouped ones, each name centred over its span. A width is the widest of the
    column's name and its values, so the table stays aligned as figures grow.
    """
    widths = [max(len(columns[i]), *(len(row[i]) for row in rows)) if rows
              else len(columns[i]) for i in range(len(columns))]
    # A group name wider than the columns it spans widens them, so the group row never
    # runs past the table it labels. The slack is spread left to right, a character at
    # a time, so the widening lands evenly rather than all on one column.
    if groups:
        at = len(columns) - sum(span for _, span in groups)
        for name, span in groups:
            slack = len(name) - (sum(widths[at:at + span]) + span - 1)
            for i in range(max(slack, 0)):
                widths[at + i % span] += 1
            at += span

    # Column 0 is the date and column 1 the SHA — text, left-aligned. Everything
    # after them is a figure, right-aligned so digits line up place by place.
    def line(prefix: str, fields: list[str]) -> str:
        cells = [f"{field:<{widths[i]}}" if i < 2 else f"{field:>{widths[i]}}"
                 for i, field in enumerate(fields)]
        return (prefix + " ".join(cells)).rstrip()

    out = [f"# {note}" for note in notes]
    if groups:
        ungrouped = len(columns) - sum(span for _, span in groups)
        cells = [" " * widths[i] for i in range(ungrouped)]
        at = ungrouped
        for name, span in groups:
            # The span's own width, plus the single space between each of its columns.
            width = sum(widths[at:at + span]) + span - 1
            cells.append(f"{name:^{width}}")
            at += span
        out.append(("# " + " ".join(cells)).rstrip())
    out.append(line("# ", columns))
    out += [line("  ", row) for row in rows]
    return "\n".join(out) + "\n"
