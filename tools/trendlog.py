"""Shared rendering for the repo's tracked trend logs.

Two files record one row per commit and print a delta against the newest row:
`observe/coverage.txt` (`tools/coverage.py`) and `observe/complexity.txt`
(`tools/modgraph/baseline.py`). They share this renderer so a reader who has
learned to read one has learned to read both.

The layout puts every column's name **directly over its own column**, rather than
listing the names in a prose header a reader has to count against the row. The name
row is a comment, so it carries a `# ` prefix; the data rows are indented by the same
two characters to line up under it. Every reader here splits on whitespace after
stripping, so the indent is invisible to them.
"""

from __future__ import annotations


def render(notes: list[str], columns: list[str], rows: list[list[str]]) -> str:
    """One trend log's whole text: `notes` as `#` lines, the column names over their
    columns, then `rows`.

    `columns` names every field; each row carries one already-formatted string per
    name. A width is the widest of the column's name and its values, so the table stays aligned as figures grow.
    """
    widths = [max(len(columns[i]), *(len(row[i]) for row in rows)) if rows
              else len(columns[i]) for i in range(len(columns))]
    # Column 0 is the date and column 1 the SHA — text, left-aligned. Everything
    # after them is a figure, right-aligned so digits line up place by place.
    def line(prefix: str, fields: list[str]) -> str:
        cells = [f"{field:<{widths[i]}}" if i < 2 else f"{field:>{widths[i]}}"
                 for i, field in enumerate(fields)]
        return (prefix + " ".join(cells)).rstrip()

    out = [f"# {note}" for note in notes]
    out.append(line("# ", columns))
    out += [line("  ", row) for row in rows]
    return "\n".join(out) + "\n"
