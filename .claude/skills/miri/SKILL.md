---
name: miri
description: Use this skill when running Miri against the koan repo — exercising the leak/UB audit slate, attributing process-exit leaks to allocation sites, validating an unsafe-site fix under tree borrows, or any other `cargo +nightly miri test` invocation. Sets the standard command of record, captures the run-in-background-and-wait pattern that avoids wasted compile-cache warmth and stray monitoring processes, and points at the roadmap items that own the slate.
---

# miri

Standardized workflow for running Miri in the koan repo. The audit slate is the load-bearing memory-safety check; this skill exists so every agent that runs Miri uses the same command, the same scheduling pattern, and the same parsing.

## Assumptions (do not re-verify)

- `cargo +nightly miri` is installed. **Never** probe with `cargo +nightly miri --version`, `which miri`, `rustup component list`, etc. — assume it works and run.
- Tree borrows is the borrow-checker mode. All audit runs use `MIRIFLAGS="-Zmiri-tree-borrows"`.
- The audit slate (16 named tests) is owned by [`roadmap/post-stage-1-audit-redo.md`](../../../roadmap/post-stage-1-audit-redo.md). Read that file for the canonical test list when you need it; do not hard-code the list elsewhere.
- The memory-model invariants the slate verifies are documented in [`design/memory-model.md`](../../../design/memory-model.md).

## The command of record

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- <test-names>
```

For the bulk audit slate, `<test-names>` is the 16-item list from `roadmap/post-stage-1-audit-redo.md`. For triage, it's a single test name at a time.

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
