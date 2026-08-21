# `audit/`

Measurement scaffolding that no build ships.

**The rule: `src/` is production code.** An instrument that only ever runs behind a `cfg`
is not production code, however deeply it hooks into it, so its body lives here. What
stays in `src/` is the declaration that names it and the hooks at the moment being
measured, which by definition cannot move. The payoff is that reading, grepping, or
counting `src/` answers a question about the shipped interpreter and nothing else.

Not a complexity-score argument: `cargo modules` builds the default configuration, where
a `cfg`-gated instrument is already absent from the graph `tools/modgraph` scores. Nothing
in this directory is on that graph, so nothing here bears on the score.

| file | what it is | measured from |
|---|---|---|
| `counting_alloc.rs` | a `GlobalAlloc` that tallies allocations and delegates to an inner one | `src/main.rs`, `src/tests.rs`, `tests/allocation_baseline.rs` |
| `reach_audit.rs` | the reach-tightness report: the over-pinning audit at the fold chokepoint | `src/machine/core/arena/step_allocator.rs` |
| `shapes/*.koan` | the recorded programs the baselines below are measured over | — |
| `measure.sh` | builds the counted binary and prints the table below | — |

Two mechanics follow from living outside `src/`. Rust files here are `#[path]`-included
by the module that declares them, so `reach_audit.rs` is the koan-crate module
`machine::core::reach_audit` and resolves `crate::…` and `super::…` exactly as a file
under `src/` does. And a `#[path]`-loaded module's children resolve against its *own*
directory rather than one named after it, so `reach_audit.rs` spells its
`#[path = "reach_audit/tests.rs"]` out to keep the repo's `foo/tests.rs` layout.

The counting allocator has a second reason to be here: `src/` carries no `unsafe` at all,
and a counting `#[global_allocator]` is an `unsafe impl` that under `src/` would register
with `tools/observe_tests.py slate-audit` as a production site owing a Miri slate group.
Miri still exercises it — `src/tests.rs` installs it as the lib-test binary's global
allocator, so every slate test allocates through it.

## The reach-tightness report

`reach_audit.rs`, under `cfg(any(test, feature = "region-audit"))`. Every residence audit
in the engine catches under-pinning; this catches the other direction — a fold that pins
an operand's regions when its product embeds nothing of that operand. It hooks inside
`alloc_carried_with`'s brand closure, which is the one moment both the operand views and
the product are nameable, and `src/main.rs` prints its flags after a
`cargo run --features region-audit` run. Its place in the memory model is
[memory-model.md § Debug region audits](../design/memory-model.md#debug-region-audits).

## The counter

`counting_alloc.rs` wraps an allocator rather than replacing one, so a counted build and a
shipped build allocate through the same `mimalloc` and a wall-clock reading off the counted
build stays comparable. It keeps two tallies: a process-wide atomic, which is the
whole-program number a binary's `main` can report, and a thread-local, which is what a test
brackets one call with — the test harness runs tests concurrently, so a shared counter would
fold every other test's traffic into the bracket.

Three targets `#[path]`-include the one file:

- `src/main.rs`, under the `alloc-count` cargo feature. The binary wraps `mimalloc` (or the
  system allocator under Miri, which cannot call mimalloc's FFI) and prints
  `allocations: N` to stderr after the run, before the region audits so their own pin-ring
  walk stays out of the count. Off by default, so a normal build is untouched.
- `src/tests.rs`, for the library's unit-test binary. `crate::tests::allocation_count()`
  reads the thread-local tally; the relocation door's fixed-cost test
  (`src/machine/execute/lift/tests.rs`) is its caller.
- `tests/allocation_baseline.rs`, the regression test below. It needs no feature flag, which
  is what keeps `counting_alloc.rs` in the default verify slate.

## Recorded baselines

```
bash audit/measure.sh
```

Whole-program totals — interpreter startup, parse, and the run — from a debug build on this
machine. The absolute figures are not portable across machines or toolchains; what they are
for is the *margin over an empty program*, and its movement when the execute path's
allocation traffic changes. `git log -p audit/README.md` dates each figure to the change that
moved it.

| shape | what it exercises | allocations | scaling term |
|---|---|---|---|
| `shapes/tail_loop.koan` | 100 tail-recursive steps | 12 498 | 98.9 per step, linear |
| `shapes/operator_chain.koan` | 128-operand `+` chain, 127 dispatches | 5 902 | ≈26 per dispatch, mildly superlinear |
| `shapes/scope_walk_depth2_calls8.koan` | 8 dispatches down a 2-deep scope walk | 3 823 | — |
| `shapes/scope_walk_depth2_calls40.koan` | 40 dispatches down a 2-deep scope walk | 5 421 | 49.9 per dispatch |
| `shapes/scope_walk_depth10_calls8.koan` | 8 dispatches down a 10-deep scope walk | 5 284 | — |
| `shapes/scope_walk_depth10_calls40.koan` | 40 dispatches down a 10-deep scope walk | 6 879 | 49.8 per dispatch |
| *(empty program)* | interpreter startup and builtin seeding | 2 972 | — |

No shape can use comments: koan has none, and `#` is reserved for quoting. The prose
that would have headed each file is here instead.

`tail_loop.koan` is a tail-recursive countdown, so TCO holds the node table and the regions
flat and its total is a per-*step* cost rather than a per-frame one. `operator_chain.koan`
is one flat left fold, so every `+` is a dispatch — a bucket walk, a pick, and a
working-expression rebuild — and its total is a per-*dispatch* cost. The four
`scope_walk_*.koan` shapes are a depth × call-count grid over the third axis, how far a
dispatch's **scope walk** reaches: at each depth, an innermost body of *m* `PROBE y`
statements sits under *n* nested scopes that each shadow the `PROBE` bucket with a
non-admitting same-key overload, so every dispatch strict- and hard-rejects at all *n* shadow
scopes before picking at the root. Between them the three axes cover where the execute path's
allocation traffic scales.

The step term is exactly linear — 118.0 flat at 10, 50, 100 and 200 steps. The dispatch term
is not: marginal cost rises 28.9 → 29.5 → 30.4 → 31.3 across the 16→32, 32→64, 64→128 and
128→256 operand doublings, so a chain pays slightly more per operator the longer it gets.
Below 16 operands the fixed cost swamps the term, so the rise is only readable over the
larger sizes. Whatever drives it is unmeasured; the shapes are sized to the linear-enough
middle rather than to the tail.

The walk term is **flat in depth**. Differencing the two call counts at one depth cancels
parse and setup and leaves 32 dispatches' marginal cost: 1 598 allocations at depth 2 against
1 595 at depth 10 — 49.9 and 49.8 per dispatch, the two depths indistinguishable. The walk's
per-scope buffers are hosted on the drain's step scratch arena
([dag-scheduler.md § The drain protocol](../workgraph/design/dag-scheduler.md#the-drain-protocol)),
so a deeper walk bumps more scratch bytes and takes no heap allocation for them. The grid's
absolute per-dispatch figure is higher than the operator chain's because each `PROBE y` is a
whole statement — its own node, frame and working expression — where a chain operand is one
dispatch inside a single statement's fold.

## The regression test

`tests/allocation_baseline.rs` asserts the two absolute shapes' bracketed counts against a
stated bound — 12 489 for the loop, 5 893 for the chain, each carrying 41 allocations of
headroom. The bounds are tight by design: the margin is smaller than the 100 (loop) or 127
(chain) a single new allocation on the scaling path would add, so one added allocation fails
a test. Rebaselining is meant to be a deliberate edit, and the failure message says so.

Those figures sit 9 under the whole-program table above — the same gap for both shapes, and
essentially all of it process startup. The interpreter holds almost no lazy one-time state, so
there is little for a first-run bracket to absorb.

`allocations_for` still runs each shape once *outside* its bracket. The bounds are tight
enough that one lazy static added later would break them by test order alone — whichever test
reached it first would carry its whole initialisation cost. Warming with the shape's own
source rather than a stand-in is what keeps that coverage total: whatever the shape
initialises is warm by construction, including statics added later.

The scope-walk grid is held to a *shape* rather than a count. Its test differences the two
depths' per-dispatch costs and bounds the growth at one allocation per extra dispatch — far
under the ≥256 a single reintroduced per-scope allocation would add over 8 extra scopes ×
32 dispatches. A shape bound needs no rebaselining when an unrelated term moves, which is
what suits it to a claim about depth-independence rather than about a total.
