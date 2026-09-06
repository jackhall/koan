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
several times and refuses to report if any of the three moved between runs.
`nanos` gates only under `--gate-time`, with a tolerance and a per-row noise
bound, and only beside a rebuilt baseline. Wall time on this machine is not a
figure to hold to the digit — the sweep's own fastest-of-many is what makes it
readable at all — so a row counts as slower only when it clears three bars at
once: fastest trial more than `TIME_TOLERANCE` above the baseline's fastest, the
excess in nanoseconds above that row's own trial noise, and a baseline at least
`TIME_FLOOR_NANOS` so timer granularity on a sub-microsecond verb cannot trip it.

A sweep compares against the SHA it reports on by **building that commit's harness
and running it now**, alternating the two binaries so a machine that drifts through
the sweep drifts through both equally, and keeping each row's fastest time. The
recorded `nanos` is not comparable across sessions — this machine is also doing
other things, and the same commit measured a week apart can differ by half again —
so a reading taken beside the one it is judged against is the only honest time
column. The gating figures compare against that rebuilt binary too, since it is
what that commit actually costs rather than what was written down about it; a
disagreement between the two says the record is stale and is reported as such.

The baseline commit is checked out into a cached worktree under
`target/perf-baselines/`, so the build is paid once per SHA. When the SHA is not in
this repository — a record carried over from a branch that never landed — the sweep
says so and falls back to the recorded figures, with the time column reading as the
cross-session comparison it then is.

Totals, not means: a per-call figure is `allocations / calls` on read, so the row
keeps the exact count. The per-unit table is derived the same way
`tools/alloc_audit.py` derives its terms, as `(large - small) / (n_large -
n_small)` over a benchmark's two sizes.

Debug profile, matching every other measurement in the repo.

    python3 tools/cellgraph_perf.py            # sweep, compare against the newest recorded SHA
    python3 tools/cellgraph_perf.py --record   # sweep and append this HEAD's rows to the record
    python3 tools/cellgraph_perf.py --gate     # also exit 1 if allocations or bytes rose
    python3 tools/cellgraph_perf.py --gate-time  # also exit 1 if a row got slower
    python3 tools/cellgraph_perf.py --calibrate  # measure the time gate's noise floor
    python3 tools/cellgraph_perf.py --quiet    # a summary line, plus only the rows that moved
    python3 tools/cellgraph_perf.py --trials 5 # fewer interleaved runs per binary
"""

from __future__ import annotations

import argparse
import csv
import datetime
import io
import json
import shutil
import statistics
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RECORD = REPO / "cellgraph" / "observe" / "perf.csv"
KEEP_SHAS = 3

# Where a baseline commit is checked out and built. Under `target/`, which is already
# ignored, and one worktree per SHA so the build is paid once. Each worktree keeps its
# own `target/` rather than sharing the main one, so building a baseline never
# invalidates the fingerprints of the build under test.
BASELINES = REPO / "target" / "perf-baselines"

# Runs of each binary per sweep. A run is milliseconds, so the figure buys robustness
# almost free: the fastest of fifteen is a reading the machine's other work has not
# touched, and two runs of one binary would already settle the deterministic columns.
TRIALS = 15

# How far above the baseline's fastest trial a row may sit before it counts as slower.
# Wide, because the bar it guards is "this item did not make a verb slower", not "this
# machine is quiet": the noise bound below is what catches a real regression a narrower
# tolerance would only bury in false alarms.
TIME_TOLERANCE = 0.10

# The baseline reading below which time is not gated at all. A `create` is a handful of
# word writes, so its row is timer granularity rather than work, and a percentage of it
# means nothing.
TIME_FLOOR_NANOS = 5_000

# Rows at or above the floor whose spread the tolerance must cover. Set from `--calibrate`,
# which sweeps HEAD against a rebuild of HEAD: same source, two binaries, so every row's
# movement is the floor rather than a reading. The tolerance sits above that floor's own
# spread, not at it — a gate that fires on the machine teaches nothing.
CALIBRATION_COVERAGE = 0.95

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


def build(cwd: Path) -> Path:
    """Build the harness in `cwd` and return the binary cargo wrote.

    The path comes out of cargo's own JSON rather than being assembled from a target
    directory, which is what lets a baseline worktree keep its build wherever its
    configuration puts it.
    """
    run = subprocess.run(
        ["cargo", "build", "-p", "cellgraph", "--features", "perf", "--bin", "perf",
         "--message-format", "json-render-diagnostics"],
        cwd=cwd, capture_output=True, text=True)
    if run.returncode != 0:
        print(run.stderr, file=sys.stderr, end="")
        sys.exit(f"the harness failed to build in {_display(cwd)} "
                 f"(exit {run.returncode})")
    for line in run.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == "perf"
                and message.get("executable")):
            return Path(message["executable"])
    sys.exit("cargo built no executable for the harness")


def baseline(sha: str) -> Path | None:
    """The harness as `sha` built it, from a worktree cached under `target/`.

    `None` when the commit is not in this repository, which a record outlives easily:
    the rows are capped by recency and take no view on whether the branch they were
    swept on ever landed.
    """
    if _git("rev-parse", "--verify", "--quiet", f"{sha}^{{commit}}").returncode != 0:
        return None
    tree = BASELINES / sha
    if not tree.exists():
        BASELINES.mkdir(parents=True, exist_ok=True)
        added = _git("worktree", "add", "--detach", "--quiet", str(tree), sha)
        if added.returncode != 0:
            print(added.stderr, file=sys.stderr, end="")
            return None
    return build(tree)


def prune_baselines(keep: set[str]) -> None:
    """Drop every cached worktree for a SHA the record no longer carries, so the cache
    stays the size of the record rather than the size of the project's history."""
    if not BASELINES.exists():
        return
    for tree in BASELINES.iterdir():
        if tree.name not in keep:
            _git("worktree", "remove", "--force", str(tree))
            shutil.rmtree(tree, ignore_errors=True)


def measure(binaries: dict[str, Path], filter_: str | None, trials: int
            ) -> tuple[dict[str, dict[Key, Reading]], dict[str, dict[Key, int]]]:
    """Run every binary `trials` times and return each one's readings and its noise.

    The binaries alternate which goes first, so a machine that gets busier or cooler
    over the sweep does it to both alike. Every row keeps its fastest time: noise on a
    shared machine only ever adds, so the minimum is the reading with the least of it.
    """
    order = list(binaries)
    runs: dict[str, list[dict[Key, Reading]]] = {tag: [] for tag in order}
    for trial in range(trials):
        for tag in (order if trial % 2 == 0 else order[::-1]):
            runs[tag].append(_run(binaries[tag], filter_))
    folded = {tag: _fold(tag, results) for tag, results in runs.items()}
    return ({tag: readings for tag, (readings, _) in folded.items()},
            {tag: noise for tag, (_, noise) in folded.items()})


def _fold(tag: str, runs: list[dict[Key, Reading]]
          ) -> tuple[dict[Key, Reading], dict[Key, int]]:
    """One binary's runs collapsed to one reading per row, fastest time kept, beside
    how noisy that row was across the sweep.

    Noise is the median trial minus the fastest — how far this row drifted upward while
    the machine was doing whatever else it was doing, measured on the same runs the
    reading came from. It is the second bar the time gate holds a row to, so a row this
    machine cannot measure steadily cannot be called a regression.

    The runs are also what proves the gating figures are deterministic. The crate has
    two `HashMap`s under `RandomState`, whose allocation pattern depends only on how
    many entries go in — but a claim like that is worth checking on every sweep rather
    than reasoning about once.
    """
    first = runs[0]
    keys = set().union(*(run.keys() for run in runs))
    drifted = [key for key in sorted(keys)
               if any(_gating(run.get(key)) != _gating(first.get(key))
                      for run in runs)]
    if drifted:
        print(f"non-deterministic: these rows differ between runs of the {tag} harness",
              file=sys.stderr)
        for key in drifted:
            readings = sorted({_gating(run.get(key)) for run in runs}, key=str)
            print(f"  {_name(key)}: {' then '.join(str(r) for r in readings)}",
                  file=sys.stderr)
        sys.exit(1)

    times = {key: sorted(run[key].nanos for run in runs if key in run)
             for key in first}
    readings = {key: Reading(reading.cap, reading.calls, reading.allocations,
                             reading.bytes, times[key][0])
                for key, reading in first.items()}
    noise = {key: round(statistics.median(trials) - trials[0])
             for key, trials in times.items()}
    return readings, noise


def _run(binary: Path, filter_: str | None) -> dict[Key, Reading]:
    """One run of the harness, parsed out of the CSV it writes to stdout."""
    command = [str(binary), filter_] if filter_ else [str(binary)]
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


def _slowed(now: Reading, then: Reading | None, noise: int | None) -> bool:
    """Whether this row is a time regression: all three of the bars at once.

    Wall time is gated only beside a baseline rebuilt and run in the same sweep, so
    `then` and `noise` are both readings taken on this machine minutes ago. Any of the
    three bars failing means the sweep cannot tell a regression from the machine, and a
    reading it cannot tell apart is not one to fail a run over.
    """
    if then is None or noise is None:
        return False
    if then.nanos < TIME_FLOOR_NANOS:
        return False
    excess = now.nanos - then.nanos
    return excess > then.nanos * TIME_TOLERANCE and excess > noise


def _percent(now: float, then: float | None) -> str:
    """Time's movement, as a percentage. Time gates only under `--gate-time`, so for
    every other sweep this column reads as a trend rather than as a figure to hold."""
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
           noise: dict[Key, int] | None, quiet: bool
           ) -> tuple[int, int, int, list[str]]:
    """Print one table per benchmark and a per-unit table under them, each figure
    beside its movement against the recorded SHA. Returns how many rows moved, how
    many rose in a gating figure, and how many got slower.

    `noise` is the baseline's per-row trial spread, and `None` where time cannot be
    gated — no rebuilt baseline, or the caller did not ask. Without it the time column
    is the trend it has always been and no row is marked."""
    lines: list[str] = []
    moved = 0
    risen = 0
    slowed = 0

    header = (f"{'n':>5} {'verb':<11} {'calls':>7} {'allocations':>12} {'Δ':>7} "
              f"{'bytes':>10} {'Δ':>8} {'nanos':>11} {'Δ%':>11}")
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
            slower = _slowed(now, then, noise.get(key) if noise else None)
            # A row with nothing on record has not moved — it has never been read
            # before. That keeps a quiet first sweep to its one summary line.
            row_moved = then is not None and (allocations != "=" or byte_delta != "=")
            moved += row_moved
            risen += rose
            slowed += slower
            percent = _percent(now.nanos, then.nanos if then else None)
            rows.append((row_moved or slower,
                         f"{n:>5} {verb:<11} {now.calls:>7} {now.allocations:>12} "
                         f"{allocations:>7} {now.bytes:>10} {byte_delta:>8} "
                         f"{now.nanos:>11} "
                         f"{percent + ' slow' if slower else percent:>11}"))
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
    return moved, risen, slowed, lines


def _plural(count: int, noun: str) -> str:
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


def _basis(against: tuple[str, str], rebuilt: bool) -> str:
    """How the comparison figures were come by — the sweep's own claim about how far
    its time column can be trusted."""
    return ("rebuilt and run beside this sweep" if rebuilt
            else f"as its figures were recorded {against[0]}")


def _report_drift(measured: dict[Key, Reading],
                  recorded: dict[Key, Reading], against: tuple[str, str]) -> None:
    """Say so when the baseline commit's harness does not reproduce what was written
    down for it. The gating figures are deterministic, so a disagreement is about the
    record — swept from a dirty tree, or under a different toolchain — and the rebuilt
    reading is the one to believe."""
    drifted = [key for key in sorted(measured.keys() & recorded.keys())
               if _gating(measured[key]) != _gating(recorded[key])]
    if not drifted:
        return
    print(f"stale record: {_plural(len(drifted), 'row')} recorded at {against[1]} "
          f"differ from what that commit measures now", file=sys.stderr)
    for key in drifted:
        print(f"  {_name(key)}: recorded {_gating(recorded[key])}, "
              f"measures {_gating(measured[key])}", file=sys.stderr)


def _summary(rows: int, moved: int, risen: int, slowed: int, timed: bool,
             against: tuple[str, str] | None, rebuilt: bool) -> str:
    """The one line a quiet sweep leads with. Every figure it names is either zero —
    nothing below it to read — or the row count of a table printed underneath."""
    if against is None:
        return f"cellgraph perf: {_plural(rows, 'row')} measured; no recorded sweep"
    basis = f"vs {against[1]} {_basis(against, rebuilt)}"
    movement = (f"{_plural(moved, 'row')} moved" if moved
                else f"{_plural(rows, 'row')}, all at parity")
    time = f"; {_plural(slowed, 'row')} slowed" if timed else ""
    return (f"cellgraph perf: {movement} {basis}; "
            f"{_plural(risen, 'row')} rose{time}")


def calibrate(readings: dict[Key, Reading], floor: dict[Key, Reading],
              noise: dict[Key, int]) -> list[str]:
    """What this machine's own spread is, one binary of a commit against another built from
    the same source, and what tolerance covers it.

    The sweep's protocol — alternating the binaries, fastest of many trials — takes the
    machine's *load* out of a reading, but not the two binaries' code layout, which is
    fixed at the link and moves a row by a stable amount all sweep long. That is what this
    measures, and what `TIME_TOLERANCE` has to clear: without it a tolerance is a guess,
    and a gate set below the floor fires on the linker.
    """
    spreads = sorted(
        (abs(readings[key].nanos - floor[key].nanos) / floor[key].nanos, key)
        for key in readings.keys() & floor.keys()
        if floor[key].nanos >= TIME_FLOOR_NANOS)
    if not spreads:
        return ["", f"no row reached the {TIME_FLOOR_NANOS} ns floor; nothing to calibrate"]

    covered = spreads[min(int(len(spreads) * CALIBRATION_COVERAGE), len(spreads) - 1)][0]
    lines = ["", f"{'row':<28} {'spread':>8} {'noise':>8}", ]
    for spread, key in spreads:
        lines.append(f"{_name(key):<28} {spread:>7.1%} {noise.get(key, 0):>8}")
    lines += [
        "",
        f"{_plural(len(spreads), 'row')} above the {TIME_FLOOR_NANOS} ns floor: "
        f"spread runs to {spreads[-1][0]:.1%}, "
        f"{CALIBRATION_COVERAGE:.0%} of rows within {covered:.1%}",
        f"TIME_TOLERANCE is {TIME_TOLERANCE:.0%}"
        f"{' — below the floor it has to clear' if TIME_TOLERANCE <= covered else ''}",
    ]
    return lines


def _display(path: Path) -> str:
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def _calibrate(args, sha: str, dirty: bool) -> int:
    """Sweep HEAD against a worktree build of HEAD. Same source both sides, so every row's
    movement is this machine's floor rather than a reading about the code."""
    if dirty:
        sys.exit("calibration compares HEAD against a worktree build of HEAD, so it needs "
                 "a clean tree — commit or stash first")
    rebuilt = baseline(sha)
    if rebuilt is None:
        sys.exit(f"could not build {sha} in a worktree")
    measured, noise = measure({"head": build(REPO), "floor": rebuilt},
                              args.filter, args.trials)
    print(f"calibrating against {sha}, built twice from the same source")
    for line in calibrate(measured["head"], measured["floor"], noise["floor"]):
        print(line)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--record", action="store_true",
                        help="append this sweep to the record (default: read-only)")
    parser.add_argument("--gate", action="store_true",
                        help="exit 1 if allocations or bytes rose on any row")
    parser.add_argument("--gate-time", action="store_true",
                        # `%` is argparse's own formatting character, so the tolerance
                        # is spelled out rather than interpolated.
                        help="exit 1 if any row got slower than the rebuilt baseline "
                             "by more than the tolerance and its own trial noise")
    parser.add_argument("--calibrate", action="store_true",
                        help="sweep HEAD against a rebuild of HEAD and report the "
                             "spread the time tolerance has to clear")
    parser.add_argument("--quiet", action="store_true",
                        help="print one summary line, plus only the rows that moved")
    parser.add_argument("--file", type=Path, default=RECORD, dest="record_path",
                        help=f"where the record lives (default: {_display(RECORD)})")
    parser.add_argument("--filter", default=None,
                        help="run only the benchmarks whose name contains this")
    parser.add_argument("--trials", type=int, default=TRIALS,
                        help=f"runs of each binary per sweep (default: {TRIALS})")
    args = parser.parse_args()
    args.record_path = args.record_path.resolve()

    date, sha, dirty = _stamp()
    if args.calibrate:
        return _calibrate(args, sha, dirty)
    rows = read_record(args.record_path)
    # A recording sweep reports against the newest SHA it is not about to replace; a
    # read-only one reports against the newest on record, its own commit included.
    against = newest_sha(rows, exclude=sha if args.record else None)
    # Only the real record decides what the cache holds: a sweep pointed at a record of
    # its own is asking a question, not redefining which baselines are worth keeping.
    if args.record_path == RECORD:
        prune_baselines({row["sha"] for row in rows})

    binaries = {"head": build(REPO)}
    rebuilt = baseline(against[1]) if against else None
    if rebuilt is not None:
        binaries["baseline"] = rebuilt
    measured, noise = measure(binaries, args.filter, args.trials)
    readings = measured["head"]

    if rebuilt is not None:
        comparison = measured["baseline"]
        _report_drift(comparison, readings_at(rows, against[1]), against)
    else:
        comparison = readings_at(rows, against[1]) if against else {}
        if against is not None:
            print(f"{against[1]} is not in this repository — comparing against the "
                  f"recorded figures, whose time column was read in another session",
                  file=sys.stderr)

    # Time is gated only against a binary rebuilt and run beside this sweep: a recorded
    # `nanos` was read in another session on a machine doing other things, and holding
    # this run to it would be comparing two different afternoons.
    timed = args.gate_time and rebuilt is not None
    if args.gate_time and rebuilt is None:
        print(f"time cannot be gated: no baseline was rebuilt beside this sweep, so "
              f"there is nothing to compare {'against' if against is None else against[1]}"
              f"'s time column against", file=sys.stderr)
    moved, risen, slowed, lines = report(
        readings, comparison, noise.get("baseline") if timed else None, args.quiet)
    if args.quiet:
        print(_summary(len(readings), moved, risen, slowed, timed,
                       against, rebuilt is not None))
    elif against is None:
        print(f"no recorded sweep to compare against ({_display(args.record_path)})")
    else:
        print(f"against {against[1]}, {_basis(against, rebuilt is not None)} "
              f"({_display(args.record_path)})")
    for line in lines:
        print(line)

    if args.record:
        record_sweep(readings, date, sha, dirty, args.record_path)
        print(f"\nrecorded {date} {sha}"
              f"{' (dirty)' if dirty else ''} to {_display(args.record_path)}")
    return 1 if (args.gate and risen) or (args.gate_time and slowed) else 0


if __name__ == "__main__":
    sys.exit(main())
