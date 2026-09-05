# Miri audit slate — cellgraph

The canonical list of tests Miri's tree-borrows mode signs off on for the
`cellgraph` crate's memory safety — the cell slab, the per-cell bump regions,
and the witnessed carrier seam that re-anchors an erased value at a step's
lifetime. Each test is a minimal-shape mirror of an unsafe site in the crate;
the slate passes when Miri reports zero process-exit leaks and zero UB across
the whole list.

Sibling to [koan's own slate](../../observe/miri_slate.md) and
[workgraph's](../../workgraph/observe/miri_slate.md) — split because these
tests live in the `cellgraph` crate's own lib test binary, a separate
`cargo test` target from either. Not wired into `tools/observe_tests.py`'s
automated drift check (that stays scoped to koan's own `src/`): this is plain
documentation, kept current by hand, for a manual run per
[.claude/skills/miri/SKILL.md](../../.claude/skills/miri/SKILL.md).

## The slate

0 tests. The crate carries no `unsafe` yet. Names below are the exact test
identifiers; pass them after `--` in the Miri command, or run the whole lib
binary:

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p cellgraph --lib
```

_(empty)_

## Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute,
raw-pointer round-trip, interior-mutation pattern under a live shared borrow,
or a cycle shape that storage-side reasoning can't rule out. Tests are
minimal-shape mirrors of the unsafe operation, not end-to-end feature tests;
they fail when Miri reports UB or a leak, not on values. Register the test here
**before** the slate run, never after it.

When you add or remove a slate test, update the list above and re-run the slate
to confirm the count matches.

## Recent full-slate run durations

The five most-recent full-slate runs, newest first. Append a new entry on every
full-slate run and trim to five so this list stays bounded. Use the most-recent
entry as the baseline expectation when scheduling a run.

<!-- slate-durations:start -->
<!-- slate-durations:end -->
