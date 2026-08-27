#!/usr/bin/env python3
"""Sweep the recorded allocation shapes in `audit/shapes/`, print each one's
whole-program allocation and symbol-mint totals, difference them into the
marginal terms the roadmap cites, and check the bounds in
`tests/allocation_baseline.rs` still have the headroom they claim.

With `--baseline`, the readings are recorded into `observe/alloc.txt`: one row per
commit swept, newest first, capped to the last 5. Without it the sweep is read-only
and prints a delta against the newest recorded row. That record is the figure of
reference: nothing quotes a number at it from prose, so a reader never has to
re-measure a base revision to learn what HEAD costs.

The row stores the shape readings and nothing else; the marginal terms are derived
from them on read. A term is a difference of two columns over the gap in `n`, so a
stored term would round away the exact per-shape figure the row exists to keep.

Three readings, from two builds:

  * **whole-program** — `--features alloc-count` wraps the binary's allocator in
    the delegating counter (`audit/counting_alloc.rs`) and arms the symbol-mint
    tally; each shape's run prints `allocations: N` and `symbols_minted: N` to
    stderr. Process totals, so interpreter startup and parse are in them.
  * **bracketed** — `tests/allocation_baseline.rs` brackets one interpret call
    per shape and prints `bracketed <path> <n>` under `--nocapture`. Startup is
    outside the bracket, which is why it sits a little under the whole-program
    figure and why the bounds are stated against it. Measured fresh every run and
    not recorded: it is read to check a bound's headroom, never differenced.
  * **terms** — a marginal cost per step, per live frame, or per declared name,
    differenced over a pair of whole-program readings so parse and startup cancel.

Debug profile, matching every other measurement in the repo: a release build
inlines enough to move the count, and these shapes exist to compare against each
other over time rather than to state a shipped cost.

    python3 tools/alloc_audit.py                # sweep, read-only
    python3 tools/alloc_audit.py --baseline     # sweep and record
"""

from __future__ import annotations

import argparse
import datetime
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from trendlog import render  # noqa: E402  (needs the path above)


REPO = Path(__file__).resolve().parent.parent
SHAPES_DIR = REPO / "audit" / "shapes"
RECORD = REPO / "observe" / "alloc.txt"
BOUNDS_TEST = REPO / "tests" / "allocation_baseline.rs"
KEEP_ENTRIES = 5

# The record's column order, and the shape set a sweep expects to find on disk. Declared
# rather than globbed so a row's positional columns mean the same thing in every entry.
SHAPES = (
    "empty",
    "wide_n10", "wide_n100",
    "deep_n10", "deep_n100",
    "declare_n10", "declare_n100",
)

NOTES = [
    "whole-program totals through the counting allocator, one row per commit swept. The",
    "marginal terms are derived from these on read, not stored — a term is a difference of",
    "two columns divided by the gap in `n`, so storing it would round away the exact",
    "per-shape figure the row is here to keep.",
    "managed by tools/alloc_audit.py --baseline; newest first, capped to 5 sweeps",
]
COLUMNS = ["date", "short-sha"] + ["alloc", "sym"] * len(SHAPES)
GROUPS = [(shape, 2) for shape in SHAPES]


@dataclass(frozen=True)
class Term:
    """One marginal cost, as `(minuend - subtrahend) / units`.

    `minuend` and `subtrahend` name either a shape or an earlier term, so a term can
    be differenced out of two others. `subtrahend` is None for `fixed`, which is an
    absolute reading.
    """

    name: str
    minuend: str
    subtrahend: str | None
    units: int
    basis: str


TERMS = (
    Term("fixed", "empty", None, 1, "interpreter startup and builtin seeding"),
    Term("wide_step", "wide_n100", "wide_n10", 90,
         "per tail-recursive step through the wide body"),
    Term("deep_frame", "deep_n100", "deep_n10", 90,
         "per live frame, at recursion depth 10 to 100"),
    Term("declare_name", "declare_n100", "declare_n10", 90,
         "per declared name, across five declaration forms"),
)


@dataclass(frozen=True)
class Bound:
    """One `const` in `tests/allocation_baseline.rs`, and what it bounds.

    `measure` is an expression over bracketed shape readings — a shape name, or a
    difference written as a pair, or a difference of two such pairs. `repetitions`
    is how many times the bounded path runs across that measurement, which is what
    one newly added allocation would cost: a bound is well-set when its headroom is
    non-negative (the test passes) and under that (the test can still see the one).
    """

    test: str
    const: str
    measure: tuple
    repetitions: int


BOUNDS = (
    Bound("the_empty_program_stays_within_its_startup_bound", "BOUND",
          ("empty",), 32),
    Bound("the_wide_shape_stays_within_its_per_step_bound", "BOUND",
          ("wide_n100",), 100),
    Bound("the_deep_shape_stays_within_its_per_frame_bound", "BOUND",
          ("deep_n100",), 100),
    Bound("the_declare_shape_stays_within_its_per_name_bound", "BOUND",
          (("declare_n100", "declare_n10"),), 450),
)


# --- measurement ------------------------------------------------------------


def sweep_shapes() -> dict[str, tuple[int, int]]:
    """Build the counted binary and run every shape, returning
    `{shape: (allocations, symbols)}` from the counts it writes to stderr."""
    subprocess.run(["cargo", "build", "--quiet", "--features", "alloc-count"],
                   cwd=REPO, check=True)
    binary = REPO / "target" / "debug" / "koan"

    readings: dict[str, tuple[int, int]] = {}
    for shape in sorted(SHAPES_DIR.glob("*.koan")):
        # The counts ride stderr beside any region-audit output; the shape's own
        # PRINT goes to stdout and is dropped. A shape that fails to run is left
        # out of the readings — its absence is the report.
        run = subprocess.run([str(binary), str(shape.relative_to(REPO))],
                             cwd=REPO, capture_output=True, text=True)
        allocations = _scrape(run.stderr, "allocations")
        symbols = _scrape(run.stderr, "symbols_minted")
        if allocations is None or symbols is None:
            print(f"shape failed to run: {shape.name}", file=sys.stderr)
            continue
        readings[shape.stem] = (allocations, symbols)
    return readings


def _scrape(text: str, label: str) -> int | None:
    match = re.search(rf"^{label}: (\d+)$", text, re.MULTILINE)
    return int(match.group(1)) if match else None


def bracket_shapes() -> dict[str, int]:
    """Run the bounded regression test and return `{shape: bracketed allocations}`.

    Read from the `bracketed <path> <n>` lines `allocations_for` prints, which are
    written before its assertion — so a *failing* bound still reports its
    measurement, which is exactly the run a rebaseline needs to read.
    """
    run = subprocess.run(
        ["cargo", "test", "--quiet", "--test", "allocation_baseline", "--", "--nocapture"],
        cwd=REPO, capture_output=True, text=True)
    bracketed: dict[str, int] = {}
    # The harness interleaves its own progress dots with these lines under
    # `--nocapture`, so the match is unanchored.
    for path, count in re.findall(r"bracketed (\S+\.koan) (\d+)\b", run.stdout):
        bracketed[Path(path).stem] = int(count)
    if not bracketed:
        print("no bracketed figures: the allocation_baseline test printed none",
              file=sys.stderr)
    return bracketed


def derive_terms(readings: dict[str, tuple[int, int]]) -> dict[str, tuple[float, float]]:
    """Difference the shape readings into the marginal terms, in declaration order —
    a term over terms reads the ones already derived."""
    values: dict[str, tuple[float, float]] = {k: (float(a), float(s))
                                              for k, (a, s) in readings.items()}
    derived: dict[str, tuple[float, float]] = {}
    for term in TERMS:
        if term.minuend not in values:
            continue
        if term.subtrahend is None:
            alloc, symbols = values[term.minuend]
        else:
            if term.subtrahend not in values:
                continue
            minuend, subtrahend = values[term.minuend], values[term.subtrahend]
            alloc = minuend[0] - subtrahend[0]
            symbols = minuend[1] - subtrahend[1]
        result = (alloc / term.units, symbols / term.units)
        derived[term.name] = result
        values[term.name] = result
    return derived


def _evaluate(measure: tuple, bracketed: dict[str, int]) -> int | None:
    """Fold a bound's measurement expression over the bracketed readings."""
    total = 0
    for index, operand in enumerate(measure):
        if isinstance(operand, tuple):
            if operand[0] not in bracketed or operand[1] not in bracketed:
                return None
            value = bracketed[operand[0]] - bracketed[operand[1]]
        else:
            if operand not in bracketed:
                return None
            value = bracketed[operand]
        total = value if index == 0 else total - value
    return total


def read_bound_constants() -> dict[tuple[str, str], int]:
    """Scrape `{(test fn, const name): value}` out of the bounded regression test."""
    constants: dict[tuple[str, str], int] = {}
    current = ""
    for line in BOUNDS_TEST.read_text().splitlines():
        fn = re.match(r"fn (\w+)\(", line.strip())
        if fn:
            current = fn.group(1)
            continue
        const = re.match(r"const (\w+): u64 = ([\d_]+);", line.strip())
        if const and current:
            constants[(current, const.group(1))] = int(const.group(2).replace("_", ""))
    return constants


# --- the record -------------------------------------------------------------


def _git(*args) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)


def _stamp() -> tuple[str, str]:
    sha = _git("rev-parse", "--short", "HEAD")
    short = sha.stdout.strip() if sha.returncode == 0 else "no-git"
    dirty = _git("diff", "--quiet", "HEAD").returncode != 0
    return datetime.date.today().isoformat(), f"{short}+" if dirty else short


def _is_ancestor(sha: str) -> bool:
    bare = sha[:-1] if sha.endswith("+") else sha
    return _git("merge-base", "--is-ancestor", bare, "HEAD").returncode == 0


def _display(path: Path) -> str:
    """A path as written in a report: repo-relative when it is under the repo."""
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def _entries(path: Path) -> list[list[str]]:
    """The record's data lines, in order, as whitespace-split fields."""
    if not path.exists():
        return []
    rows = []
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            rows.append(stripped.split())
    return rows


def read_rows(record: Path) -> list[tuple[str, str, dict[str, tuple[int, int]]]]:
    """The recorded sweeps as `(date, sha, {shape: (allocations, symbols)})`, newest
    first. A row shorter than the column key is skipped rather than read short: the
    columns are positional, so a row written under a different shape set carries no
    reading this one can name."""
    width = 2 + 2 * len(SHAPES)
    rows = []
    for fields in _entries(record):
        if len(fields) != width:
            continue
        readings = {shape: (int(fields[2 + 2 * i]), int(fields[3 + 2 * i]))
                    for i, shape in enumerate(SHAPES)}
        rows.append((fields[0], fields[1], readings))
    return rows


def prior_row(record: Path, exclude: str | None = None):
    """The sweep a reading is reported against — the newest recorded one, skipping the
    SHA this run is about to overwrite."""
    for date, sha, readings in read_rows(record):
        if exclude is None or sha != exclude:
            return date, sha, readings
    return None


def record_row(readings: dict[str, tuple[int, int]], date: str, sha: str,
               record: Path) -> None:
    """Prepend this sweep to the record, replacing any entry already at this SHA,
    dropping entries whose SHA has left HEAD's history, and capping the file at
    `KEEP_ENTRIES` sweeps."""
    kept = [fields for fields in _entries(record)
            if len(fields) >= 2 and fields[1] != sha and _is_ancestor(fields[1])]
    row = [date, sha]
    for shape in SHAPES:
        allocations, symbols = readings.get(shape, (0, 0))
        row += [str(allocations), str(symbols)]
    rows = [row] + kept[:KEEP_ENTRIES - 1]
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(render(NOTES, COLUMNS, rows, GROUPS))


# --- reporting --------------------------------------------------------------


def _delta(now: float, then: float | None, places: int = 0,
           tolerance: float = 0.0) -> str:
    """The figure's movement, or `=` when it has not moved by more than `tolerance`.

    The difference is rounded to the printed precision before either the comparison
    or the formatting, so the two agree and so the noise a float difference carries
    below that precision cannot decide parity. The default tolerance calls a figure
    moved as soon as it prints as moved; a derived figure passes one unit in the
    last printed place, the smallest tolerance there is above zero."""
    if then is None:
        return "—"
    difference = round(now - then, places)
    if abs(difference) <= tolerance:
        return "="
    return f"{difference:+.{places}f}"


def _plural(count: int, noun: str) -> str:
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def _summary(shapes: int, moved_shapes: int, moved_terms: int,
             bounds: int, drifted: int, against: tuple[str, str] | None) -> str:
    """The one line a quiet sweep leads with: what moved, against what, and how the
    bounds stand. Every figure it names is either zero — nothing below it to read —
    or the row count of a table printed underneath."""
    basis = f"vs {against[0]} {against[1]}" if against else "no recorded sweep"
    if against is None:
        movement = f"{_plural(shapes, 'shape')} measured"
    elif moved_shapes or moved_terms:
        movement = (f"{_plural(moved_shapes, 'shape')}, "
                    f"{_plural(moved_terms, 'term')} moved")
    else:
        movement = f"{_plural(shapes, 'shape')}, all terms at parity"
    if not bounds:
        verdict = "bounds not checked"
    elif drifted:
        verdict = f"{bounds - drifted}/{bounds} bounds ok, {drifted} drifted"
    else:
        verdict = f"{bounds} bounds ok"
    return f"allocation audit: {movement} {basis}; {verdict}"


def report(readings: dict[str, tuple[int, int]], bracketed: dict[str, int],
           terms: dict[str, tuple[float, float]], record: Path,
           against, quiet: bool = False) -> tuple[int, int, list[str]]:
    """Print the sweep, each figure beside its delta against the recorded sweep
    `against` — the newest row on record as `(date, sha, readings)`, or none when the
    record is empty. The row's terms are derived from its shape readings here rather
    than read back, so a comparison is always between two terms computed the same way.

    Under `quiet`, a row whose figures both match that sweep is dropped, and a
    table left with no rows is not printed at all: a sweep that has not moved
    then says so in the caller's one-line summary rather than in forty. Returns
    the count of moved shape rows and moved term rows either way."""
    lines: list[str] = []
    if not quiet:
        if against is None:
            lines.append("no recorded sweep to compare against")
        else:
            lines.append(f"against the sweep recorded {against[0]} at {against[1]} "
                         f"({_display(record)})")
    recorded = against[2] if against else {}

    shape_rows = []
    for shape, (allocations, symbols) in sorted(readings.items()):
        prior = recorded.get(shape)
        bracket = bracketed.get(shape)
        allocation_delta = _delta(allocations, prior[0] if prior else None)
        symbol_delta = _delta(symbols, prior[1] if prior else None)
        shape_rows.append((allocation_delta != "=" or symbol_delta != "=",
                           f"{shape:<30} {allocations:>12} {allocation_delta:>7} "
                           f"{symbols:>8} {symbol_delta:>6} "
                           f"{bracket if bracket is not None else '-':>10}"))

    recorded_terms = derive_terms(recorded) if recorded else {}
    term_rows = []
    for term in TERMS:
        if term.name not in terms:
            continue
        alloc, symbols = terms[term.name]
        prior = recorded_terms.get(term.name)
        # A term is differenced and divided out of a pair of readings, so it carries
        # rounding noise in its last printed place that no allocation caused. One
        # unit of tolerance is what separates that noise from a term that moved.
        allocation_delta = _delta(alloc, prior[0] if prior else None, 2, 10 ** -2)
        symbol_delta = _delta(symbols, prior[1] if prior else None, 2, 10 ** -2)
        term_rows.append((allocation_delta != "=" or symbol_delta != "=",
                          f"{term.name:<22} {alloc:>12.2f} {allocation_delta:>7} "
                          f"{symbols:>8.2f} {symbol_delta:>6}  {term.basis}"))

    lines += _table(f"{'shape':<30} {'allocations':>12} {'Δ':>7} {'symbols':>8} "
                    f"{'Δ':>6} {'bracketed':>10}", shape_rows, quiet)
    lines += _table(f"{'term':<22} {'allocations':>12} {'Δ':>7} {'symbols':>8} "
                    f"{'Δ':>6}  basis", term_rows, quiet)
    return (sum(moved for moved, _ in shape_rows),
            sum(moved for moved, _ in term_rows), lines)


def _table(header: str, rows: list[tuple[bool, str]], quiet: bool) -> list[str]:
    """Render `header` over `rows`, keeping only the moved ones under `quiet` and
    every one otherwise. A table left with no rows renders as nothing at all."""
    kept = [row for moved, row in rows if moved or not quiet]
    return ["", header, *kept] if kept else []


def report_bounds(bracketed: dict[str, int],
                  quiet: bool = False) -> tuple[int, list[str]]:
    """Print each bound's headroom over its measurement. A bound earns its place by
    sitting above the measurement (the test passes) and under one repetition-set of
    it (the test can still see a single added allocation); anything else is a bound
    that has drifted from what its own doc comment claims. Under `quiet`, only the
    drifted bounds are kept. Returns how many drifted, beside the lines.
    """
    constants = read_bound_constants()
    labels = {bound: f"{bound.test}::{bound.const}" for bound in BOUNDS}
    width = max(len(label) for label in labels.values())
    rows = []
    drifted = 0
    for bound in BOUNDS:
        value = constants.get((bound.test, bound.const))
        measured = _evaluate(bound.measure, bracketed)
        label = labels[bound]
        if value is None or measured is None:
            rows.append((True, f"{label:<{width}} {'—':>9} {'—':>8} "
                               f"{'—':>9}  unreadable"))
            drifted += 1
            continue
        headroom = value - measured
        if headroom < 0:
            verdict = "OVER — the test fails"
        elif headroom >= bound.repetitions:
            verdict = f"loose — one added allocation costs {bound.repetitions}"
        else:
            verdict = "ok"
        if verdict != "ok":
            drifted += 1
        rows.append((verdict != "ok",
                     f"{label:<{width}} {measured:>9} {value:>8} {headroom:>9}  {verdict}"))
    return drifted, _table(
        f"{'bound':<{width}} {'measured':>9} {'bound':>8} {'headroom':>9}  verdict",
        rows, quiet)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--baseline", action="store_true",
                        help="record this sweep into the trend logs (default: read-only)")
    parser.add_argument("--file", type=Path, default=RECORD, dest="record",
                        help=f"where the record lives (default: {_display(RECORD)})")
    parser.add_argument("--quiet", action="store_true",
                        help="print one summary line, plus only the rows that moved "
                             "against the recorded sweep and the bounds that drifted")
    parser.add_argument("--no-bounds", action="store_true",
                        help="skip the bracketed run behind the bound check")
    args = parser.parse_args()
    args.record = args.record.resolve()

    readings = sweep_shapes()
    if not readings:
        print("no shapes measured", file=sys.stderr)
        return 1
    missing = [shape for shape in SHAPES if shape not in readings]
    if missing:
        print(f"shapes named in the record's columns but not measured: {', '.join(missing)}",
              file=sys.stderr)
        return 1
    bracketed = {} if args.no_bounds else bracket_shapes()
    terms = derive_terms(readings)

    date, sha = _stamp()
    # A recording sweep reports against the newest sweep it is not about to replace;
    # a read-only one reports against the newest on record, its own commit included.
    against = prior_row(args.record, exclude=sha if args.baseline else None)
    moved_shapes, moved_terms, lines = report(readings, bracketed, terms, args.record,
                                              against, args.quiet)
    drifted, bound_lines = (0, []) if args.no_bounds else report_bounds(bracketed, args.quiet)
    if args.quiet:
        print(_summary(len(readings), moved_shapes, moved_terms,
                       len(BOUNDS) if not args.no_bounds else 0, drifted, against))
    for line in lines + bound_lines:
        print(line)

    if args.baseline:
        record_row(readings, date, sha, args.record)
        print(f"\nrecorded {date} {sha} to {_display(args.record)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
