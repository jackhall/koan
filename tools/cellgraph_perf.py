#!/usr/bin/env python3
"""Run the `cellgraph` measurement harness, print what each public verb cost, and
keep every reading in a tidy dataframe at `cellgraph/observe/perf.csv`.

The harness is a `[[bin]]` in the crate behind the `perf` feature
(`cellgraph/perf/`). It wraps every door call in a meter that subtracts nested
doors, so a row is what that verb alone spent: `enter` reports the step machinery
and not the `alloc` inside it. Its own bookkeeping that has to happen inside a
step rides a `harness` row, so nothing is hidden and nothing inflates a real verb.

The record is a dataframe, not a log: one row per `(date, sha, dirty, benchmark,
n, cap, verb)` in long format, so every analysis is a filter and a pivot.

    import pandas as pd
    perf = pd.read_csv("cellgraph/observe/perf.csv")
    perf.pivot_table(index=["benchmark", "n", "verb"], columns="sha", values="allocations")

It is committed, appended in chronological order, and capped to the three most
recently recorded SHAs — it exists because reasoning about performance is varied,
not to carry history, which git already has. This tool reads and writes it with
the stdlib `csv` module so the repo's tooling stays dependency-free; pandas is the
analyst's own.

Two of the four figures gate and two do not. `calls`, `allocations` and `bytes`
are deterministic, which is checked rather than assumed: the sweep runs the binary
twice and refuses to report if any of the three moved between runs. `nanos` is the
minimum of those two runs and is never asserted — it is a trend reading on a
machine that is also doing other things.

Totals, not means: a per-call figure is `allocations / calls` on read, so the row
keeps the exact count. The per-unit table is derived the same way
`tools/alloc_audit.py` derives its terms, as `(large - small) / (n_large -
n_small)` over a benchmark's two sizes.

Debug profile, matching every other measurement in the repo.

    python3 tools/cellgraph_perf.py            # sweep, compare against the newest recorded SHA
    python3 tools/cellgraph_perf.py --record   # sweep and append this HEAD's rows to the record
    python3 tools/cellgraph_perf.py --gate     # also exit 1 if allocations or bytes rose
    python3 tools/cellgraph_perf.py --quiet    # a summary line, plus only the rows that moved
"""

from __future__ import annotations

import argparse
import csv
import datetime
import io
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RECORD = REPO / "cellgraph" / "observe" / "perf.csv"
KEEP_SHAS = 3

# The record's columns, in order. The first three stamp the sweep, the next four key
# the row, and the last four are the reading.
COLUMNS = ["date", "sha", "dirty",
           "benchmark", "n", "cap", "verb",
           "calls", "allocations", "bytes", "nanos"]


@dataclass(frozen=True)
class Reading:
    """One verb's exclusive cost inside one benchmark at one size."""

    cap: int
    calls: int
    allocations: int
    bytes: int
    nanos: int


# A row's identity in both the sweep and the record: which benchmark, at which size,
# for which door.
Key = tuple[str, int, str]


# --- measurement ------------------------------------------------------------


def sweep(filter_: str | None = None) -> dict[Key, Reading]:
    """Run the harness twice and return `{key: reading}`, keeping the faster time.

    The two runs are what proves the gating figures are deterministic. The crate has
    two `HashMap`s under `RandomState`, whose allocation pattern depends only on how
    many entries go in — but a claim like that is worth checking on every sweep
    rather than reasoning about once.
    """
    first, second = _run(filter_), _run(filter_)
    drifted = [key for key in sorted(first.keys() | second.keys())
               if _gating(first.get(key)) != _gating(second.get(key))]
    if drifted:
        print("non-deterministic: these rows differ between two runs of the harness",
              file=sys.stderr)
        for key in drifted:
            print(f"  {_name(key)}: {_gating(first.get(key))} then "
                  f"{_gating(second.get(key))}", file=sys.stderr)
        sys.exit(1)

    return {key: Reading(reading.cap, reading.calls, reading.allocations,
                         reading.bytes, min(reading.nanos, second[key].nanos))
            for key, reading in first.items()}


def _run(filter_: str | None) -> dict[Key, Reading]:
    """One run of the harness, parsed out of the CSV it writes to stdout."""
    command = ["cargo", "run", "--quiet", "-p", "cellgraph",
               "--features", "perf", "--bin", "perf"]
    if filter_:
        command += ["--", filter_]
    run = subprocess.run(command, cwd=REPO, capture_output=True, text=True)
    if run.returncode != 0:
        print(run.stderr, file=sys.stderr, end="")
        sys.exit(f"the harness failed to run (exit {run.returncode})")

    readings: dict[Key, Reading] = {}
    for row in csv.DictReader(io.StringIO(run.stdout)):
        key = (row["benchmark"], int(row["n"]), row["verb"])
        readings[key] = Reading(int(row["cap"]), int(row["calls"]),
                                int(row["allocations"]), int(row["bytes"]),
                                int(row["nanos"]))
    if not readings:
        sys.exit("the harness printed no rows")
    return readings


def _gating(reading: Reading | None) -> tuple[int, int, int] | None:
    """The three figures a sweep holds the harness to, as one comparable value."""
    if reading is None:
        return None
    return (reading.calls, reading.allocations, reading.bytes)


def _name(key: Key) -> str:
    benchmark, n, verb = key
    return f"{benchmark}/{n}/{verb}"


# --- the record -------------------------------------------------------------


def _git(*args) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)


def _stamp() -> tuple[str, str, bool]:
    """Today, HEAD's short SHA, and whether the tree differs from it.

    `dirty` is its own column rather than a suffix on the SHA, so filtering a
    dataframe down to the readings taken on committed code is a boolean test.
    """
    sha = _git("rev-parse", "--short=8", "HEAD")
    short = sha.stdout.strip() if sha.returncode == 0 else "no-git"
    dirty = _git("diff", "--quiet", "HEAD").returncode != 0
    return datetime.date.today().isoformat(), short, dirty


def read_record(record: Path) -> list[dict[str, str]]:
    """The recorded rows, in file order. A row missing a column is skipped: the
    columns are named, so a row written under a different schema carries no reading
    this tool can key."""
    if not record.exists():
        return []
    with record.open(newline="") as handle:
        return [row for row in csv.DictReader(handle)
                if all(row.get(column) not in (None, "") for column in COLUMNS)]


def newest_sha(rows: list[dict[str, str]],
               exclude: str | None = None) -> tuple[str, str] | None:
    """The `(date, sha)` a sweep reports against — the last one in file order,
    skipping the SHA a recording run is about to overwrite."""
    for row in reversed(rows):
        if exclude is None or row["sha"] != exclude:
            return row["date"], row["sha"]
    return None


def readings_at(rows: list[dict[str, str]], sha: str) -> dict[Key, Reading]:
    """One SHA's rows, keyed like a sweep so the two compare directly."""
    return {(row["benchmark"], int(row["n"]), row["verb"]):
            Reading(int(row["cap"]), int(row["calls"]), int(row["allocations"]),
                    int(row["bytes"]), int(row["nanos"]))
            for row in rows if row["sha"] == sha}


def record_sweep(readings: dict[Key, Reading], date: str, sha: str, dirty: bool,
                 record: Path) -> None:
    """Append this sweep to the record, replacing any rows already at this SHA and
    then dropping every SHA but the `KEEP_SHAS` most recently recorded.

    The cap is by file order, newest last, with no ancestry test: a sweep taken on a
    branch that never landed is still a reading, and it falls off the end on its own
    once three more are taken.
    """
    kept = [row for row in read_record(record) if row["sha"] != sha]
    for key in sorted(readings):
        benchmark, n, verb = key
        reading = readings[key]
        kept.append({"date": date, "sha": sha, "dirty": str(dirty).lower(),
                     "benchmark": benchmark, "n": str(n), "cap": str(reading.cap),
                     "verb": verb, "calls": str(reading.calls),
                     "allocations": str(reading.allocations),
                     "bytes": str(reading.bytes), "nanos": str(reading.nanos)})

    order: list[str] = []
    for row in kept:
        if row["sha"] not in order:
            order.append(row["sha"])
    recent = set(order[-KEEP_SHAS:])

    record.parent.mkdir(parents=True, exist_ok=True)
    with record.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, COLUMNS, lineterminator="\n")
        writer.writeheader()
        writer.writerows(row for row in kept if row["sha"] in recent)


# --- reporting --------------------------------------------------------------


def _delta(now: float, then: float | None, places: int = 0) -> str:
    """The figure's movement, or `=` when it has not moved and `—` when there is
    nothing on record to move from. Rounded to the printed precision before the
    comparison, so the verdict and the figure agree."""
    if then is None:
        return "—"
    difference = round(now - then, places)
    if difference == 0:
        return "="
    return f"{difference:+.{places}f}"


def _percent(now: float, then: float | None) -> str:
    """Time's movement, as a percentage. Time never gates, so this is the one column
    that reads as a trend rather than as a figure to hold."""
    if then is None:
        return "—"
    if then == 0:
        return "="
    change = round((now - then) / then * 100)
    return "=" if change == 0 else f"{change:+d}%"


def _sizes(readings: dict[Key, Reading]) -> dict[str, tuple[int, int]]:
    """Each benchmark's small and large size, read off the keys rather than declared:
    the harness owns the sizes, and the record is whatever it ran."""
    sizes: dict[str, set[int]] = {}
    for benchmark, n, _ in readings:
        sizes.setdefault(benchmark, set()).add(n)
    return {benchmark: (min(seen), max(seen))
            for benchmark, seen in sizes.items() if len(seen) > 1}


def _per_unit(readings: dict[Key, Reading], benchmark: str, verb: str,
              small: int, large: int) -> tuple[float, float] | None:
    """A verb's marginal allocations and bytes per unit of `n`, differenced over the
    benchmark's two sizes so whatever the shape costs to set up cancels."""
    low = readings.get((benchmark, small, verb))
    high = readings.get((benchmark, large, verb))
    if low is None or high is None or large == small:
        return None
    span = large - small
    return ((high.allocations - low.allocations) / span,
            (high.bytes - low.bytes) / span)


def _table(header: str, rows: list[tuple[bool, str]], quiet: bool) -> list[str]:
    """Render `header` over `rows`, keeping only the moved ones under `quiet`. A
    table left with no rows renders as nothing at all."""
    kept = [row for moved, row in rows if moved or not quiet]
    return ["", header, *kept] if kept else []


def report(readings: dict[Key, Reading], recorded: dict[Key, Reading],
           quiet: bool) -> tuple[int, int, list[str]]:
    """Print one table per benchmark and a per-unit table under them, each figure
    beside its movement against the recorded SHA. Returns how many rows moved and
    how many rose in a gating figure."""
    lines: list[str] = []
    moved = 0
    risen = 0

    header = (f"{'n':>5} {'verb':<11} {'calls':>7} {'allocations':>12} {'Δ':>7} "
              f"{'bytes':>10} {'Δ':>8} {'nanos':>11} {'Δ%':>6}")
    for benchmark in sorted({key[0] for key in readings}):
        rows = []
        for key in sorted(key for key in readings if key[0] == benchmark):
            _, n, verb = key
            now = readings[key]
            then = recorded.get(key)
            allocations = _delta(now.allocations, then.allocations if then else None)
            byte_delta = _delta(now.bytes, then.bytes if then else None)
            rose = then is not None and (now.allocations > then.allocations
                                         or now.bytes > then.bytes)
            # A row with nothing on record has not moved — it has never been read
            # before. That keeps a quiet first sweep to its one summary line.
            row_moved = then is not None and (allocations != "=" or byte_delta != "=")
            moved += row_moved
            risen += rose
            rows.append((row_moved,
                         f"{n:>5} {verb:<11} {now.calls:>7} {now.allocations:>12} "
                         f"{allocations:>7} {now.bytes:>10} {byte_delta:>8} "
                         f"{now.nanos:>11} "
                         f"{_percent(now.nanos, then.nanos if then else None):>6}"))
        lines += _table(f"{benchmark}\n{header}", rows, quiet)

    unit_header = (f"{'benchmark':<16} {'verb':<11} {'alloc/unit':>11} {'Δ':>8} "
                   f"{'bytes/unit':>11} {'Δ':>9}")
    unit_rows = []
    sizes = _sizes(readings)
    recorded_sizes = _sizes(recorded)
    for benchmark, (small, large) in sorted(sizes.items()):
        verbs = sorted({key[2] for key in readings if key[0] == benchmark})
        for verb in verbs:
            unit = _per_unit(readings, benchmark, verb, small, large)
            if unit is None:
                continue
            prior_sizes = recorded_sizes.get(benchmark)
            prior = (_per_unit(recorded, benchmark, verb, *prior_sizes)
                     if prior_sizes else None)
            allocation_delta = _delta(unit[0], prior[0] if prior else None, 2)
            byte_delta = _delta(unit[1], prior[1] if prior else None, 2)
            unit_rows.append((prior is not None
                              and (allocation_delta != "=" or byte_delta != "="),
                              f"{benchmark:<16} {verb:<11} {unit[0]:>11.2f} "
                              f"{allocation_delta:>8} {unit[1]:>11.2f} "
                              f"{byte_delta:>9}"))
    lines += _table(unit_header, unit_rows, quiet)
    return moved, risen, lines


def _plural(count: int, noun: str) -> str:
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def _summary(rows: int, moved: int, risen: int,
             against: tuple[str, str] | None) -> str:
    """The one line a quiet sweep leads with. Every figure it names is either zero —
    nothing below it to read — or the row count of a table printed underneath."""
    if against is None:
        return f"cellgraph perf: {_plural(rows, 'row')} measured; no recorded sweep"
    basis = f"vs {against[0]} {against[1]}"
    movement = (f"{_plural(moved, 'row')} moved" if moved
                else f"{_plural(rows, 'row')}, all at parity")
    return f"cellgraph perf: {movement} {basis}; {_plural(risen, 'row')} rose"


def _display(path: Path) -> str:
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--record", action="store_true",
                        help="append this sweep to the record (default: read-only)")
    parser.add_argument("--gate", action="store_true",
                        help="exit 1 if allocations or bytes rose on any row")
    parser.add_argument("--quiet", action="store_true",
                        help="print one summary line, plus only the rows that moved")
    parser.add_argument("--file", type=Path, default=RECORD, dest="record_path",
                        help=f"where the record lives (default: {_display(RECORD)})")
    parser.add_argument("--filter", default=None,
                        help="run only the benchmarks whose name contains this")
    args = parser.parse_args()
    args.record_path = args.record_path.resolve()

    readings = sweep(args.filter)
    date, sha, dirty = _stamp()
    rows = read_record(args.record_path)
    # A recording sweep reports against the newest SHA it is not about to replace; a
    # read-only one reports against the newest on record, its own commit included.
    against = newest_sha(rows, exclude=sha if args.record else None)
    recorded = readings_at(rows, against[1]) if against else {}

    moved, risen, lines = report(readings, recorded, args.quiet)
    if args.quiet:
        print(_summary(len(readings), moved, risen, against))
    elif against is None:
        print(f"no recorded sweep to compare against ({_display(args.record_path)})")
    else:
        print(f"against the rows recorded {against[0]} at {against[1]} "
              f"({_display(args.record_path)})")
    for line in lines:
        print(line)

    if args.record:
        record_sweep(readings, date, sha, dirty, args.record_path)
        print(f"\nrecorded {date} {sha}"
              f"{' (dirty)' if dirty else ''} to {_display(args.record_path)}")
    return 1 if args.gate and risen else 0


if __name__ == "__main__":
    sys.exit(main())
