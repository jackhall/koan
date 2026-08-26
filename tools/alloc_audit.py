#!/usr/bin/env python3
"""Sweep the recorded allocation shapes in `audit/shapes/`, print each one's
whole-program allocation and symbol-mint totals, difference them into the
marginal terms the roadmap cites, and check the bounds in
`tests/allocation_baseline.rs` still have the headroom they claim.

With `--baseline`, the readings are recorded into `observe/alloc/`: one file per
sweep, named for the commit it was taken on (`<short-sha>.txt`, every shape in
it), plus `terms.txt`, the trend log of the derived terms across the last 5
sweeps. Without it the sweep is read-only and prints a delta against the newest
recorded sweep. That record is the figure of reference: nothing quotes a number
at it from prose, so a reader never has to re-measure a base revision to learn
what HEAD costs.

Three readings, from two builds:

  * **whole-program** — `--features alloc-count` wraps the binary's allocator in
    the delegating counter (`audit/counting_alloc.rs`) and arms the symbol-mint
    tally; each shape's run prints `allocations: N` and `symbols_minted: N` to
    stderr. Process totals, so interpreter startup and parse are in them.
  * **bracketed** — `tests/allocation_baseline.rs` brackets one interpret call
    per shape and prints `bracketed <path> <n>` under `--nocapture`. Startup is
    outside the bracket, which is why it sits a little under the whole-program
    figure and why the bounds are stated against it.
  * **terms** — a marginal cost per step, dispatch, call, or cycle, differenced
    over a pair of whole-program readings so parse and startup cancel.

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


REPO = Path(__file__).resolve().parent.parent
SHAPES_DIR = REPO / "audit" / "shapes"
RECORD_DIR = REPO / "observe" / "alloc"
BOUNDS_TEST = REPO / "tests" / "allocation_baseline.rs"
KEEP_ENTRIES = 5

def run_header(date: str, sha: str) -> str:
    """The stamp and column key at the head of one sweep's file."""
    tree = " (uncommitted changes in the tree)" if sha.endswith("+") else ""
    return (
        f"# allocation audit sweep, measured {date} at {sha.rstrip('+')}{tree}\n"
        "# columns: shape  allocations  symbols  bracketed\n"
        "# whole-program totals through the counting allocator; `bracketed` is the same run\n"
        "# without interpreter startup, from tests/allocation_baseline.rs, `-` where no test\n"
        "# brackets that shape. The terms differenced out of these are in terms.txt.\n"
        "# written by tools/alloc_audit.py --baseline; one file per commit swept\n"
    )


TERMS_HEADER = (
    "# columns: date  short-sha  term  allocations  symbols\n"
    "# marginal cost of one unit of each term, differenced over the shape readings in the\n"
    "# sweep file of the same SHA; `fixed` is the empty program's own absolute total —\n"
    "# startup and builtin seeding, which every shape carries\n"
    "# managed by tools/alloc_audit.py --baseline; newest first, capped to 5 sweeps\n"
)


@dataclass(frozen=True)
class Term:
    """One marginal cost, as `(minuend - subtrahend) / units`.

    `minuend` and `subtrahend` name either a shape or an earlier term — a term over
    terms is how the per-parameter slope and the per-scope walk growth are read.
    `subtrahend` is None for `fixed`, which is an absolute reading.
    """

    name: str
    minuend: str
    subtrahend: str | None
    units: int
    basis: str


TERMS = (
    Term("fixed", "empty", None, 1, "interpreter startup and builtin seeding"),
    Term("step", "tail_loop_steps100", "tail_loop_steps10", 90, "per tail-recursive step"),
    Term("leading_loop", "leading_loop_steps100", "leading_loop_steps10", 90,
         "per leading-carrying tail step"),
    Term("try_loop", "try_loop_steps100", "try_loop_steps10", 90,
         "per TRY-carrying tail step"),
    Term("dispatch", "operator_chain_operands128", "operator_chain_operands16", 112,
         "per operator dispatch"),
    Term("scope_walk_depth2", "scope_walk_depth2_calls40", "scope_walk_depth2_calls8", 32,
         "per dispatch down a 2-deep scope walk"),
    Term("scope_walk_depth10", "scope_walk_depth10_calls40", "scope_walk_depth10_calls8", 32,
         "per dispatch down a 10-deep scope walk"),
    Term("scope_walk_scope", "scope_walk_depth10", "scope_walk_depth2", 8,
         "per extra scope walked, per dispatch"),
    Term("builtin_call", "builtin_call_calls40", "builtin_call_calls8", 32,
         "per three-parameter builtin call"),
    Term("user_fn_params1", "user_fn_params1_calls40", "user_fn_params1_calls8", 32,
         "per one-parameter user-function call"),
    Term("user_fn_params8", "user_fn_params8_calls40", "user_fn_params8_calls8", 32,
         "per eight-parameter user-function call"),
    Term("user_fn_parameter", "user_fn_params8", "user_fn_params1", 7,
         "per extra parameter, per user-function call"),
    Term("tagged_construct", "tagged_construct_calls40", "tagged_construct_calls8", 32,
         "per tagged construct-and-match cycle"),
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
    Bound("the_tail_loop_shape_stays_within_its_step_churn_bound", "BOUND",
          ("tail_loop_steps100",), 100),
    Bound("the_leading_loop_shape_stays_within_its_step_churn_bound", "BOUND",
          ("leading_loop_steps100",), 100),
    Bound("the_try_loop_shape_stays_within_its_step_churn_bound", "BOUND",
          ("try_loop_steps100",), 100),
    Bound("the_operator_chain_shape_stays_within_its_dispatch_churn_bound", "BOUND",
          ("operator_chain_operands128",), 127),
    Bound("per_dispatch_cost_does_not_grow_with_scope_walk_depth", "BOUND",
          (("scope_walk_depth10_calls40", "scope_walk_depth10_calls8"),
           ("scope_walk_depth2_calls40", "scope_walk_depth2_calls8")), 256),
    Bound("the_builtin_call_shape_stays_within_its_per_call_bound", "BOUND",
          (("builtin_call_calls40", "builtin_call_calls8"),), 32),
    Bound("the_user_fn_call_shape_stays_within_its_per_parameter_bound", "PER_CALL_BOUND",
          (("user_fn_params1_calls40", "user_fn_params1_calls8"),), 32),
    Bound("the_user_fn_call_shape_stays_within_its_per_parameter_bound", "PER_PARAMETER_BOUND",
          (("user_fn_params8_calls40", "user_fn_params8_calls8"),
           ("user_fn_params1_calls40", "user_fn_params1_calls8")), 224),
    Bound("the_tagged_construct_shape_stays_within_its_per_construction_bound", "BOUND",
          (("tagged_construct_calls40", "tagged_construct_calls8"),), 32),
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


def _entries(path: Path) -> list[list[str]]:
    """A file's data lines, in order, as whitespace-split fields."""
    if not path.exists():
        return []
    rows = []
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            rows.append(stripped.split())
    return rows


def _display(path: Path) -> str:
    """A path as written in a report: repo-relative when it is under the repo."""
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def _run_path(record_dir: Path, sha: str) -> Path:
    """One sweep's file. The dirty marker is part of the stamp, not the name — a
    second sweep at one commit replaces that commit's file rather than adding one."""
    return record_dir / f"{sha.rstrip('+')}.txt"


def record_run(readings: dict[str, tuple[int, int]], bracketed: dict[str, int],
               date: str, sha: str, record_dir: Path) -> Path:
    """Write this sweep's file: every shape's readings under one stamp."""
    width = max(len(shape) for shape in readings)
    rows = []
    for shape, (allocations, symbols) in sorted(readings.items()):
        bracket = bracketed.get(shape)
        rows.append(f"{shape:<{width}} {allocations:>7} {symbols:>7} "
                    f"{bracket if bracket is not None else '-':>7}")
    path = _run_path(record_dir, sha)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(run_header(date, sha) + "\n".join(rows) + "\n")
    return path


def record_terms(terms: dict[str, tuple[float, float]], date: str, sha: str,
                 record_dir: Path) -> None:
    """Prepend this sweep's terms to the trend log, replacing any entry already
    recorded at this SHA, dropping entries whose SHA has left HEAD's history, and
    capping it at `KEEP_ENTRIES` sweeps."""
    kept = [row for row in _entries(record_dir / "terms.txt")
            if row[1] != sha and _is_ancestor(row[1])]
    shas: list[str] = []
    for row in kept:
        if row[1] not in shas:
            shas.append(row[1])
    kept = [row for row in kept if row[1] in shas[:KEEP_ENTRIES - 1]]

    rows = [f"{date} {sha} {name} {alloc:.2f} {symbols:.2f}"
            for name, (alloc, symbols) in terms.items()]
    path = record_dir / "terms.txt"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(TERMS_HEADER + "\n".join(rows + [" ".join(row) for row in kept]) + "\n")


def prune_runs(record_dir: Path) -> list[Path]:
    """Delete sweep files the trend log no longer carries, so the two stay one
    record: a sweep is in `observe/alloc/` exactly when its terms are in terms.txt."""
    live = {row[1].rstrip("+") for row in _entries(record_dir / "terms.txt")}
    dropped = []
    for path in sorted(record_dir.glob("*.txt")):
        if path.name != "terms.txt" and path.stem not in live:
            path.unlink()
            dropped.append(path)
    return dropped


def run_order(record_dir: Path) -> list[tuple[str, str]]:
    """The recorded sweeps as `(date, sha)`, newest first, per the trend log."""
    order: list[tuple[str, str]] = []
    for row in _entries(record_dir / "terms.txt"):
        stamp = (row[0], row[1])
        if stamp not in order:
            order.append(stamp)
    return order


def read_run(record_dir: Path, sha: str) -> dict[str, tuple[int, int]]:
    """One recorded sweep's shape readings, `{shape: (allocations, symbols)}`."""
    return {row[0]: (int(row[1]), int(row[2]))
            for row in _entries(_run_path(record_dir, sha)) if len(row) >= 3}


def prior_sweep(record_dir: Path, exclude: str | None = None) -> tuple[str, str] | None:
    """The sweep a reading is reported against — the newest recorded one, skipping
    the SHA this run is about to overwrite."""
    for date, sha in run_order(record_dir):
        if exclude is None or sha != exclude:
            return date, sha
    return None


def prior_terms(record_dir: Path, sha: str) -> dict[str, tuple[float, float]]:
    """One recorded sweep's terms, `{term: (allocations, symbols)}`."""
    return {row[2]: (float(row[3]), float(row[4]))
            for row in _entries(record_dir / "terms.txt")
            if row[1] == sha and len(row) >= 5}


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
           terms: dict[str, tuple[float, float]], record_dir: Path,
           against: tuple[str, str] | None,
           quiet: bool = False) -> tuple[int, int, list[str]]:
    """Print the sweep, each figure beside its delta against the recorded sweep
    `against` — the newest one on record, or none when the record is empty.

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
                         f"({_display(_run_path(record_dir, against[1]))})")
    recorded = read_run(record_dir, against[1]) if against else {}

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

    recorded_terms = prior_terms(record_dir, against[1]) if against else {}
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
    parser.add_argument("--dir", type=Path, default=RECORD_DIR,
                        help=f"where the record lives (default: {_display(RECORD_DIR)})")
    parser.add_argument("--quiet", action="store_true",
                        help="print one summary line, plus only the rows that moved "
                             "against the recorded sweep and the bounds that drifted")
    parser.add_argument("--no-bounds", action="store_true",
                        help="skip the bracketed run behind the bound check; not for a "
                             "recording sweep, whose entry carries that reading")
    args = parser.parse_args()
    args.dir = args.dir.resolve()

    # A recorded entry carries the bracketed column, so recording without measuring it
    # would write a `-` over a figure the prior entry has.
    if args.baseline and args.no_bounds:
        parser.error("--no-bounds cannot be combined with --baseline")

    readings = sweep_shapes()
    if not readings:
        print("no shapes measured", file=sys.stderr)
        return 1
    bracketed = {} if args.no_bounds else bracket_shapes()
    terms = derive_terms(readings)

    date, sha = _stamp()
    # A recording sweep reports against the newest sweep it is not about to replace;
    # a read-only one reports against the newest on record, its own commit included.
    against = prior_sweep(args.dir, exclude=sha if args.baseline else None)
    moved_shapes, moved_terms, lines = report(readings, bracketed, terms, args.dir,
                                              against, args.quiet)
    drifted, bound_lines = (0, []) if args.no_bounds else report_bounds(bracketed, args.quiet)
    if args.quiet:
        print(_summary(len(readings), moved_shapes, moved_terms,
                       len(BOUNDS) if not args.no_bounds else 0, drifted, against))
    for line in lines + bound_lines:
        print(line)

    if args.baseline:
        path = record_run(readings, bracketed, date, sha, args.dir)
        record_terms(terms, date, sha, args.dir)
        dropped = prune_runs(args.dir)
        print(f"\nrecorded {date} {sha} to {_display(path)}")
        for gone in dropped:
            print(f"pruned {_display(gone)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
