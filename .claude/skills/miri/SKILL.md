---
name: miri
description: Use this skill when running Miri against the koan repo — exercising the leak/UB audit slate, attributing process-exit leaks to allocation sites, validating an unsafe-site fix under tree borrows, or any other `cargo +nightly miri test` invocation. Sets the standard command of record, captures the run-in-background-and-wait pattern that avoids wasted compile-cache warmth and stray monitoring processes, and points at the roadmap items that own the slate.
---

# miri

Standardized workflow for running Miri in the koan repo. The audit slate is the load-bearing memory-safety check; this skill exists so every agent that runs Miri uses the same command, the same scheduling pattern, and the same parsing.

## Assumptions (do not re-verify)

- `cargo +nightly miri` is installed. **Never** probe with `cargo +nightly miri --version`, `which miri`, `rustup component list`, etc. — assume it works and run.
- Tree borrows is the borrow-checker mode. All audit runs use `MIRIFLAGS="-Zmiri-tree-borrows"`.
- The canonical audit-slate test list lives in [`observe/miri_slate.md`](../../../observe/miri_slate.md), grouped by unsafe-site shape. Read that file for the test names; do not hard-code the list elsewhere.
- The memory-model invariants the slate verifies are documented in [`design/memory-model.md`](../../../design/memory-model.md).

## The command of record

Run Miri through [`tools/miri.py`](../../../tools/miri.py) — never hand-roll `cargo miri test`. The
script encapsulates the correct invocation and returns only the summary, so the output can't be
misread:

```
python3 tools/miri.py                         # full audit slate
python3 tools/miri.py --tests <name> [<name>…]  # triage specific tests
python3 tools/miri.py --tests <name> --track <alloc-id>   # + -Zmiri-track-alloc-id
python3 tools/miri.py --log                    # full slate; on a clean run, log the duration entry
```

It runs `MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --lib …`. The **`--lib` is
load-bearing**: every slate test lives in the lib unit-test binary, so restricting to it skips the
`tests/*.rs` integration binaries that otherwise print a misleading `0 passed; all filtered out`. The
slate names come from [`observe/miri_slate.md`](../../../observe/miri_slate.md) via
`tools/observe_tests.py slate` — never hard-code them.

The script writes the full output to `observe/miri-last-run.log`, prints one line
(`Slate: <N> passed, <failed>, <leaks> leaks, <UB> UB, <secs>s`), and **exits non-zero on any
failed test, UB, leak, or a run count that doesn't equal the slate list** — the last guard turns a
no-match filter (the classic "everything filtered out" footgun) into a loud error instead of a
silent green. Read that one line; do not re-inspect the raw log unless triaging a specific failure.

## Keeping the slate in sync

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer round-trip, interior-mutation pattern under a live shared borrow, or a cycle shape that storage-side reasoning can't rule out. Slate tests are minimal-shape mirrors of the unsafe operation, not end-to-end feature tests; they fail when Miri reports UB or a leak, not on values.

Run `python3 tools/observe_tests.py slate-audit` to surface drift between live `src/` unsafe sites and slate coverage. The tool reports files with `unsafe` but no slate group, files in the slate whose `unsafe` has been refactored out, and per-file `unsafe`-count drift against the cached `<!-- slate-fingerprint -->` block at the top of [`observe/miri_slate.md`](../../../observe/miri_slate.md). After confirming the slate is current, run `slate-audit --update` to refresh the fingerprint. The tool is file-granular — a slate test can legitimately pin behavior in a file other than its own, so a "stale group" finding may be a false positive. When the anchor file genuinely has no `unsafe` because the group pins a safe-code invariant (e.g. a `RefCell` discipline that tree borrows can still violate), add the path as a `` - `src/...` — <reason> `` bullet to the `## Stale-group whitelist` block at the top of [`observe/miri_slate.md`](../../../observe/miri_slate.md) (between the `<!-- slate-audit-whitelist:start -->` / `<!-- slate-audit-whitelist:end -->` sentinels). The audit then skips the stale-group check for that path while still flagging actual coverage gaps and fingerprint drift elsewhere.

When a slate test is added, removed, or renamed:

1. Edit [`observe/miri_slate.md`](../../../observe/miri_slate.md). New tests go under the group they pin down, or under a fresh group if the shape isn't already represented.
2. Update [`design/memory-model.md`](../../../design/memory-model.md)'s `## Verification` section if the test is named there.
3. Re-run the full slate (`python3 tools/miri.py --log`); the script's count guard confirms it still holds.
4. `python3 tools/doclinks.py check` to catch any broken inbound links.

A non-slate change to a test in `dispatch/`, `execute/`, or `parse/` does not trigger this rule — only changes that affect a test named in `observe/miri_slate.md` do.

## Scheduling: background + wait, never poll

Miri runs are slow. First-time compilation under Miri is several minutes; per-test runs are 1–3 min; the bulk audit slate is 15–25 min. The `Bash` tool's foreground timeout is 10 min, so any Miri invocation beyond a single per-test run will time out in the foreground.

**Rules:**

1. **Always launch Miri with `Bash(run_in_background=true)`.** One background invocation per Miri command.
2. **Wait for the harness's completion notification.** Do not poll with `BashOutput` in a loop, do not run `sleep N; <check>` constructs, do not spawn a separate watcher process. The harness pings when the background command finishes; do other work or wait, and pick it up then.
3. **Read the script's one-line summary at completion.** It has already parsed leaks/UB/pass-fail from the full log and enforced the count guard — don't re-tail the raw output. Move on.
4. **Run Miri invocations back-to-back, not interleaved with non-Miri builds.** Miri's target dir is separate from the regular `cargo` target dir, but switching back and forth thrashes the file cache. Plan your work: bulk baseline run, then per-test triage runs in sequence, then a final bulk verification — one continuous chain.

## Triage workflow (when leaks are reported)

The triage pattern documented in past audit work:

1. **Bulk run** to record the baseline leak count and which tests leak.
2. **Per-test runs** for each test that contributed to the leak count. Each per-test run isolates that test's allocation backtraces.
3. **Pinned-id run** for any test still leaking after step 2: add `-Zmiri-track-alloc-id=<id>` (alongside the tree-borrows flag) to surface the allocation+drop history of a specific allocation. The id comes from the leak report in step 2.
4. **Cluster** the per-test attributions into root-cause buckets. A single Rc cycle can produce many leak entries (RawVec storage, Rc heap allocation, hashbrown rehashes, etc.) — attribute to root cause, not to leaf allocation.

## Reading the output

`tools/miri.py` does the reading: it parses the full captured log (not a tail), sums passes/leaks/UB
across binaries, enforces the run-count guard, and prints the one-line summary. Trust that line.

Only when **triaging a specific failure** do you open `observe/miri-last-run.log` directly. What to
look for there:

- **UB:** `error: Undefined Behavior: <kind>` — never acceptable. Stop and surface.
- **Leaks:** `error: memory leaked: <N> allocations (<bytes> bytes)` — the detector runs at process exit.
- **Tree-borrows-specific:** "protector", "activation", "disabled tag" in error messages — all UB classes.

If you ever run `cargo miri test` by hand instead of the script, remember why the script exists:
slate tests live in the lib binary (`cargo test` runs it *first*); the `tests/*.rs` binaries run
*last*, match none of the slate filter, and tail a wall of `0 passed; N filtered out` that looks like
"Miri ran nothing." Exit code 0 alone is not proof of a pass — `cargo test` exits 0 when 0 tests run.
The script's count guard is exactly this check; prefer it.

## Reporting full-slate runs

Pass `--log` to a full-slate run: on a clean result the script prepends today's entry to the
`<!-- slate-durations:start -->` / `<!-- slate-durations:end -->` block in
[`observe/miri_slate.md`](../../../observe/miri_slate.md) and trims it to five, in the format

```
- YYYY-MM-DD: <duration>s — <N> tests, <leaks> leaks, <ub> UB
```

Relay the script's summary line to the user. `--log` applies to full-slate runs only; triage
(`--tests`) never logs.
