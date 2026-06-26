#!/usr/bin/env python3
"""Run Miri (tree borrows) over the koan audit slate and return only the summary.

Encapsulates the command of record so callers never hand-roll `cargo miri test`
or — the recurring footgun — `tail` its output and misread a `0 passed`
integration-binary line as the slate result.

Two things make the raw command easy to misread:

  * `cargo miri test -- <names>` runs EVERY test binary (the lib unit-test
    binary plus each `tests/*.rs` integration binary) and applies the name
    filter to each independently. All slate tests live in the lib binary, so
    every other binary prints `0 passed; N filtered out` — normal, but it buries
    the real `<N> passed` line and traps anything reading the last summary.
  * The leak detector and UB checks only surface at process exit.

This script fixes both: it runs `--lib` only (where every slate test lives, so
there are no spurious "all filtered out" binaries), parses the whole output
once, and — crucially — asserts that the number of tests Miri actually ran
equals the slate count. A filter that matches nothing becomes a loud error
instead of a silent pass.

Usage:
  python3 tools/miri.py                      # full slate
  python3 tools/miri.py --tests A B C        # triage specific tests
  python3 tools/miri.py --tests A --track 1234   # + -Zmiri-track-alloc-id=1234
  python3 tools/miri.py --log                # on a clean full-slate run, prepend
                                             # the duration entry to observe/miri_slate.md

Exit code is 0 only when: 0 failed, 0 UB, 0 leaks, and (full slate) the run
count equals the slate count. The full captured log path is printed so a
triage run can be read in full when needed.
"""

import argparse
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SLATE_MD = ROOT / "observe" / "miri_slate.md"
MIRIFLAGS = "-Zmiri-tree-borrows"

RESULT_RE = re.compile(
    r"test result: \w+\. (\d+) passed; (\d+) failed; \d+ ignored"
)
# Miri reports leaks in one of two forms depending on the toolchain: an aggregate
# `memory leaked: N allocations (M bytes)` (older), or one `error: memory leaked: alloc<NN> (…)`
# line per leaked allocation (current). Count both so a format change can't silently zero the count
# (which would mask the leak abort behind a "0 leaks" summary while the process still exits non-zero).
LEAK_AGG_RE = re.compile(r"memory leaked: (\d+) allocations? \(\d+ bytes?\)")
LEAK_ONE_RE = re.compile(r"memory leaked: alloc\d+\b")
UB_RE = re.compile(r"error: Undefined Behavior")
DURATIONS_BLOCK = re.compile(
    r"(<!-- slate-durations:start -->\n)(.*?)(\n<!-- slate-durations:end -->)",
    re.DOTALL,
)


def slate_names() -> list[str]:
    """The slate test list — single source of truth is `observe_tests.py slate`."""
    out = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "observe_tests.py"), "slate"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return out.split()


def list_lib_tests() -> list[str]:
    """Full `module::path::leaf` names of every lib unit test, via a normal (non-Miri)
    `cargo test --list`. Cheap (normal profile, already cached) and the name set is
    identical to the Miri build, so it's a sound source for resolving slate names."""
    out = subprocess.run(
        ["cargo", "test", "--lib", "--quiet", "--", "--list"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
        env=_env(),
    ).stdout
    # Lines look like `machine::core::arena::tests::foo: test`.
    return [ln[: -len(": test")] for ln in out.splitlines() if ln.endswith(": test")]


def resolve_slate(names: list[str]) -> list[str]:
    """Map each slate leaf name to its full `module::path::leaf`, requiring an exact
    leaf match to exactly one test. Catches a renamed/removed slate entry (0 matches)
    and an ambiguous one (>1) loudly, before the ~25-min Miri run — and the full paths
    let the run filter with `--exact`, so a slate name that is a prefix of another test
    (e.g. `…_alloc` vs `…_alloc_via_raw_ptr_roundtrip`) can't silently pull in siblings."""
    all_tests = list_lib_tests()
    by_leaf: dict[str, list[str]] = {}
    for full in all_tests:
        by_leaf.setdefault(full.rsplit("::", 1)[-1], []).append(full)
    resolved, problems = [], []
    for name in names:
        hits = by_leaf.get(name, [])
        if len(hits) == 1:
            resolved.append(hits[0])
        else:
            problems.append(f"  {name}: {len(hits)} match(es)" + (f" — {hits}" if hits else ""))
    if problems:
        raise SystemExit(
            "ERROR: slate name(s) did not resolve to exactly one lib test "
            "(renamed, removed, or ambiguous):\n" + "\n".join(problems)
        )
    return resolved


def run_miri(names: list[str], track: int | None, exact: bool) -> tuple[int, str, float]:
    miriflags = MIRIFLAGS
    if track is not None:
        miriflags += f" -Zmiri-track-alloc-id={track}"
    filt = ["--exact", *names] if exact else list(names)
    cmd = ["cargo", "+nightly", "miri", "test", "--lib", "--quiet", "--", *filt]
    start = time.monotonic()
    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        capture_output=True,
        text=True,
        env={**_env(), "MIRIFLAGS": miriflags},
    )
    elapsed = time.monotonic() - start
    return proc.returncode, proc.stdout + proc.stderr, elapsed


def _env() -> dict:
    import os

    return dict(os.environ)


def parse(output: str) -> dict:
    passed = sum(int(m.group(1)) for m in RESULT_RE.finditer(output))
    failed = sum(int(m.group(2)) for m in RESULT_RE.finditer(output))
    leaks = sum(int(m.group(1)) for m in LEAK_AGG_RE.finditer(output)) + len(
        LEAK_ONE_RE.findall(output)
    )
    ub = len(UB_RE.findall(output))
    return {"passed": passed, "failed": failed, "leaks": leaks, "ub": ub}


def update_duration_log(n: int, leaks: int, ub: int, seconds: float) -> None:
    """Prepend today's entry to the slate-durations block, trimmed to five."""
    today = time.strftime("%Y-%m-%d")
    entry = f"- {today}: {seconds:.0f}s — {n} tests, {leaks} leaks, {ub} UB"
    text = SLATE_MD.read_text()
    m = DURATIONS_BLOCK.search(text)
    if not m:
        print("warning: no slate-durations block found; skipping log update", file=sys.stderr)
        return
    existing = [ln for ln in m.group(2).splitlines() if ln.strip().startswith("- ")]
    trimmed = "\n".join([entry, *existing][:5])
    SLATE_MD.write_text(text[: m.start(2)] + trimmed + text[m.end(2):])
    print(f"logged: {entry}")


def main() -> int:
    ap = argparse.ArgumentParser(description="Run the Miri audit slate and report only the summary.")
    ap.add_argument("--tests", nargs="+", metavar="NAME",
                    help="triage: run these tests instead of the full slate")
    ap.add_argument("--track", type=int, metavar="ID",
                    help="add -Zmiri-track-alloc-id=ID (triage)")
    ap.add_argument("--log", action="store_true",
                    help="on a clean full-slate run, prepend the duration entry to the slate doc")
    args = ap.parse_args()

    is_slate = args.tests is None
    names = slate_names() if is_slate else args.tests
    if not names:
        print("no tests to run", file=sys.stderr)
        return 2
    # Slate names are resolved to exact full paths up front so a renamed/removed entry
    # fails fast (before the long Miri build) rather than as a post-run miscount; triage
    # keeps substring matching so a partial name still works interactively.
    if is_slate:
        names = resolve_slate(names)
    expected = len(names)

    label = "slate" if is_slate else f"triage ({expected} test(s))"
    print(f"running Miri {label} under {MIRIFLAGS} (--lib)…", file=sys.stderr)
    code, output, seconds = run_miri(names, args.track, exact=is_slate)

    log_path = ROOT / "observe" / "miri-last-run.log"
    log_path.write_text(output)

    r = parse(output)
    # The anti-footgun: a name filter that matches nothing (or matches in the
    # wrong binary) shows up as ran < expected rather than a misleading "ok".
    miscount = is_slate and r["passed"] + r["failed"] != expected

    ok = code == 0 and r["failed"] == 0 and r["ub"] == 0 and r["leaks"] == 0 and not miscount

    summary = (
        f"{'Slate' if is_slate else 'Miri'}: {r['passed']} passed, "
        f"{r['failed']} failed, {r['leaks']} leaks, {r['ub']} UB, {seconds:.0f}s"
    )
    print(summary)
    print(f"full log: {log_path}")

    if miscount:
        print(
            f"ERROR: ran {r['passed'] + r['failed']} test(s) but the slate lists "
            f"{expected} — a filter matched nothing (wrong binary? renamed test?). "
            f"Did you mean to run with --lib? See {log_path}.",
            file=sys.stderr,
        )
    if r["ub"]:
        print(f"ERROR: Miri reported {r['ub']} Undefined Behavior site(s) — see {log_path}.", file=sys.stderr)
    if r["leaks"]:
        print(f"ERROR: Miri reported {r['leaks']} leaked allocation(s) — see {log_path}.", file=sys.stderr)

    if ok and is_slate and args.log:
        update_duration_log(r["passed"], r["leaks"], r["ub"], seconds)

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
