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
`nanos` gates only under `--gate-time`, against a tolerance, and only beside a
rebuilt baseline. Every row is held to the one bar: its fastest trial no more than
`TIME_TOLERANCE` above the baseline's fastest. What makes a small row readable at
all is the harness, which runs each shape in blocks sized so every row's block
clears 20 µs and reports the fastest block per run; and what makes two binaries
comparable is that every trial execs a fresh copy of its binary, so neither side
is read from one fixed draw of where the kernel put its pages. The tolerance is
set from `--calibrate`, which is the only honest way to know what this machine's
spread is.

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
import subprocess
import sys
import tempfile
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

# Where each trial's fresh copy of a binary is written, and removed after the run. Under
# `target/` rather than the system temp directory, which may be mounted `noexec`.
TRIAL_COPIES = REPO / "target" / "perf-trials"

# Runs of each binary per sweep. A run is milliseconds, so the figure buys robustness
# almost free: the fastest of fifteen is a reading the machine's other work has not
# touched, and two runs of one binary would already settle the deterministic columns.
TRIALS = 15

# How far above the baseline's fastest trial a row may sit before it counts as slower.
# Set from `--calibrate`, which sweeps HEAD against a rebuild of HEAD — same source, two
# binaries, so every row's movement is this machine's spread rather than a reading — at
# twice the worst row it reads: a gate that fires on the machine teaches nothing. Two
# consecutive calibrations read every row within 5%.
TIME_TOLERANCE = 0.10

# The record's columns, in order. The first three stamp the sweep, the next four key
# the row, and the last four are the reading. `runs` is not recorded: it is how the
# harness sized a row's block, not a figure about the code.
COLUMNS = ["date", "sha", "dirty",
           "benchmark", "n", "cap", "verb",
           "calls", "allocations", "bytes", "nanos"]


@dataclass(frozen=True)
class Reading:
    """One verb's exclusive cost inside one benchmark at one size, per run of the shape.

    `runs` is the block the harness read `nanos` from, carried so the report can show it;
    a reading off the record has none to show."""

    cap: int
    calls: int
    allocations: int
    bytes: int
    nanos: int
    runs: int | None = None


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
            ) -> dict[str, dict[Key, Reading]]:
    """Run every binary `trials` times and return each one's readings.

    The binaries alternate which goes first, so a machine that gets busier or cooler
    over the sweep does it to both alike. Every row keeps its fastest time: noise on a
    shared machine only ever adds, so the minimum is the reading with the least of it.

    Every trial execs a fresh copy of its binary. Two files with identical bytes read a
    few rows a stable 4–12% apart for as long as their pages stay cached — the physical
    placement of the file's own pages, drawn once when the file is written — so a copy
    per trial has both sides sample that draw rather than each sit on one.
    """
    order = list(binaries)
    runs: dict[str, list[dict[Key, Reading]]] = {tag: [] for tag in order}
    TRIAL_COPIES.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=TRIAL_COPIES) as copies:
        for trial in range(trials):
            for tag in (order if trial % 2 == 0 else order[::-1]):
                fresh = Path(copies) / f"{tag}-{trial}"
                shutil.copy(binaries[tag], fresh)
                runs[tag].append(_run(fresh, filter_))
                fresh.unlink()
    return {tag: _fold(tag, results) for tag, results in runs.items()}


def _fold(tag: str, runs: list[dict[Key, Reading]]) -> dict[Key, Reading]:
    """One binary's runs collapsed to one reading per row, fastest time kept.

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

    fastest = {key: min((run[key] for run in runs if key in run),
                        key=lambda reading: reading.nanos)
               for key in first}
    return {key: Reading(reading.cap, reading.calls, reading.allocations,
                         reading.bytes, fastest[key].nanos, fastest[key].runs)
            for key, reading in first.items()}


def _run(binary: Path, filter_: str | None) -> dict[Key, Reading]:
    """One run of the harness, parsed out of the CSV it writes to stdout. A baseline
    built from a commit whose harness printed no `runs` column reads as having none."""
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
                                int(row["nanos"]),
                                int(row["runs"]) if "runs" in row else None)
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


def _bar(then: Reading | None) -> int | None:
    """The most a row may read before it counts as slower: the baseline plus the
    tolerance, rounded the way the report prints it so the verdict and the figure agree.

    Wall time is gated only beside a baseline rebuilt and run in the same sweep, so
    `then` is a reading taken on this machine minutes ago, and every row has a bar: the
    harness sizes each row's block so none is too small to hold to a percentage.
    """
    if then is None:
        return None
    return round(then.nanos * (1 + TIME_TOLERANCE))


def _slowed(now: Reading, then: Reading | None) -> bool:
    """Whether this row read above its bar."""
    bar = _bar(then)
    return bar is not None and now.nanos > bar


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
           timed: bool, quiet: bool) -> tuple[int, int, int, list[str]]:
    """Print one table per benchmark and a per-unit table under them, each figure
    beside its movement against the recorded SHA. Returns how many rows moved, how
    many rose in a gating figure, and how many got slower.

    `timed` is false where time cannot be gated — no rebuilt baseline, or the caller
    did not ask. Then the time column is the trend it has always been and no row is
    marked. Under `timed` every row also shows its `bar`, the most it could have read
    and passed, so what the gate held each row to is on the run itself."""
    lines: list[str] = []
    moved = 0
    risen = 0
    slowed = 0

    header = (f"{'n':>5} {'verb':<11} {'calls':>7} {'allocations':>12} {'Δ':>7} "
              f"{'bytes':>10} {'Δ':>8} {'nanos':>11} {'runs':>5} {'Δ%':>11}"
              + (f" {'bar':>11}" if timed else ""))
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
            slower = timed and _slowed(now, then)
            # A row with nothing on record has not moved — it has never been read
            # before. That keeps a quiet first sweep to its one summary line.
            row_moved = then is not None and (allocations != "=" or byte_delta != "=")
            moved += row_moved
            risen += rose
            slowed += slower
            percent = _percent(now.nanos, then.nanos if then else None)
            bar = _bar(then)
            rows.append((row_moved or slower,
                         f"{n:>5} {verb:<11} {now.calls:>7} {now.allocations:>12} "
                         f"{allocations:>7} {now.bytes:>10} {byte_delta:>8} "
                         f"{now.nanos:>11} {now.runs if now.runs else '—':>5} "
                         f"{percent + ' slow' if slower else percent:>11}"
                         + (f" {bar if bar is not None else '—':>11}" if timed else "")))
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


def calibrate(readings: dict[Key, Reading], floor: dict[Key, Reading]) -> list[str]:
    """What this machine's own spread is, one binary of a commit against another built from
    the same source, and whether the tolerance covers it.

    The sweep's protocol takes the machine's load out of a reading (alternating binaries,
    fastest of many trials), the process's cold start out of the small rows (the harness's
    blocks), and the placement of each file's pages out of the pair (a fresh copy per
    trial). What is left is what `TIME_TOLERANCE` has to clear, on every row: a gate set
    below it fires on the machine rather than on the change.
    """
    spreads = sorted(
        (abs(readings[key].nanos - floor[key].nanos) / floor[key].nanos, key)
        for key in readings.keys() & floor.keys())
    if not spreads:
        return ["", "no row was read on both sides; nothing to calibrate"]

    lines = ["", f"{'row':<28} {'spread':>8}"]
    for spread, key in spreads:
        lines.append(f"{_name(key):<28} {spread:>7.1%}")
    worst = spreads[-1][0]
    lines += [
        "",
        f"{_plural(len(spreads), 'row')}: median spread {spreads[len(spreads) // 2][0]:.1%}, "
        f"worst {worst:.1%}",
        f"TIME_TOLERANCE is {TIME_TOLERANCE:.0%}"
        f"{' — below the spread it has to clear' if TIME_TOLERANCE <= worst else ''}",
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
    measured = measure({"head": build(REPO), "floor": rebuilt}, args.filter, args.trials)
    print(f"calibrating against {sha}, built twice from the same source")
    for line in calibrate(measured["head"], measured["floor"]):
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
                             "by more than the tolerance")
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
    measured = measure(binaries, args.filter, args.trials)
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
    moved, risen, slowed, lines = report(readings, comparison, timed, args.quiet)
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
