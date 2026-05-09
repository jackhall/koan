---
name: miri
description: Use this skill when running Miri against the koan repo — exercising the leak/UB audit slate, attributing process-exit leaks to allocation sites, validating an unsafe-site fix under tree borrows, or any other `cargo +nightly miri test` invocation. Sets the standard command of record, captures the run-in-background-and-wait pattern that avoids wasted compile-cache warmth and stray monitoring processes, and points at the roadmap items that own the slate.
---

# miri

Standardized workflow for running Miri in the koan repo. The audit slate is the load-bearing memory-safety check; this skill exists so every agent that runs Miri uses the same command, the same scheduling pattern, and the same parsing.

## Assumptions (do not re-verify)

- `cargo +nightly miri` is installed. **Never** probe with `cargo +nightly miri --version`, `which miri`, `rustup component list`, etc. — assume it works and run.
- Tree borrows is the borrow-checker mode. All audit runs use `MIRIFLAGS="-Zmiri-tree-borrows"`.
- The canonical audit-slate test list lives in [`TEST.md`](../../../TEST.md) under "The slate", grouped by unsafe-site shape. Read that file for the test names; do not hard-code the list elsewhere.
- The memory-model invariants the slate verifies are documented in [`design/memory-model.md`](../../../design/memory-model.md).

## The command of record

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- <test-names>
```

For the bulk audit slate, `<test-names>` is the list from [`TEST.md`](../../../TEST.md). For triage, it's a single test name at a time.

## Keeping the slate in sync

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer round-trip, interior-mutation pattern under a live shared borrow, or a cycle shape that storage-side reasoning can't rule out. Slate tests are minimal-shape mirrors of the unsafe operation, not end-to-end feature tests; they fail when Miri reports UB or a leak, not on values.

When a slate test is added, removed, or renamed:

1. Edit [`TEST.md`](../../../TEST.md)'s slate section. New tests go under the group they pin down, or under a fresh group if the shape isn't already represented.
2. Update [`design/memory-model.md`](../../../design/memory-model.md)'s `## Verification` section if the test is named there.
3. Re-run the full slate (the command of record above) and confirm the count and pass-line in `TEST.md` still hold.
4. `python3 tools/doclinks.py check` to catch any broken inbound links.

A non-slate change to a test in `dispatch/`, `execute/`, or `parse/` does not trigger this rule — only changes that affect a test named in `TEST.md`'s slate list do.

## Scheduling: background + wait, never poll

Miri runs are slow. First-time compilation under Miri is several minutes; per-test runs are 1–3 min; the bulk audit slate is 15–25 min. The `Bash` tool's foreground timeout is 10 min, so any Miri invocation beyond a single per-test run will time out in the foreground.

**Rules:**

1. **Always launch Miri with `Bash(run_in_background=true)`.** One background invocation per Miri command.
2. **Wait for the harness's completion notification.** Do not poll with `BashOutput` in a loop, do not run `sleep N; <check>` constructs, do not spawn a separate watcher process. The harness pings when the background command finishes; do other work or wait, and pick it up then.
3. **Read the output once at completion** with a single `BashOutput` call. Parse the leak count, UB lines, and per-test pass/fail from that one read. Move on.
4. **Run Miri invocations back-to-back, not interleaved with non-Miri builds.** Miri's target dir is separate from the regular `cargo` target dir, but switching back and forth thrashes the file cache. Plan your work: bulk baseline run, then per-test triage runs in sequence, then a final bulk verification — one continuous chain.

## Triage workflow (when leaks are reported)

The triage pattern documented in past audit work:

1. **Bulk run** to record the baseline leak count and which tests leak.
2. **Per-test runs** for each test that contributed to the leak count. Each per-test run isolates that test's allocation backtraces.
3. **Pinned-id run** for any test still leaking after step 2: add `-Zmiri-track-alloc-id=<id>` (alongside the tree-borrows flag) to surface the allocation+drop history of a specific allocation. The id comes from the leak report in step 2.
4. **Cluster** the per-test attributions into root-cause buckets. A single Rc cycle can produce many leak entries (RawVec storage, Rc heap allocation, hashbrown rehashes, etc.) — attribute to root cause, not to leaf allocation.

## Reading the output

- **Pass:** `test result: ok. <N> passed; 0 failed; ...`
- **UB:** `error: Undefined Behavior: <kind>` — never acceptable. Stop and surface.
- **Leaks:** `error: memory leaked: <N> allocations (<bytes> bytes) in <K> allocations` — the leak detector runs at process exit. Sum across tests for a slate-wide count.
- **Tree-borrows-specific:** look for "protector", "activation", "disabled tag" in error messages — all are UB classes.

## Reporting full-slate runs

Every full-slate run ends with a single user-facing line of the form:

```
Slate: <N> tests, <leaks> leaks, <ub> UB, <duration>s — last full-slate baseline was <prev>s.
```

Read `<prev>` from the top entry of the `<!-- slate-durations:start -->` /
`<!-- slate-durations:end -->` block in [`TEST.md`](../../../TEST.md) (write
`first run` if the block is empty). Then prepend the new entry and trim the
list to five entries so the bound holds. Entry format:

```
- YYYY-MM-DD: <duration>s — <N> tests, 0 leaks, 0 UB
```

Use today's date (the harness has it in `currentDate`). A run that fails
(non-zero leaks or UB) still gets logged so the list reflects real timings,
with the failure counts in place of the zeros.

This rule applies to **full-slate runs only** — single-test triage runs and
non-slate Miri invocations do not append.
