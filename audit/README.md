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
  `allocations: N` to stderr after the run — beside `symbols_minted: N`, the same feature's
  other reading — before the region audits so their own pin-ring walk stays out of the count.
  Off by default, so a normal build is untouched.
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

The **symbols** column is the run's `symbols_minted` total: every `Symbol::of` and
`Symbol::of_parts` that reached the BLAKE3 funnel, counted where the two meet
(`machine::model::labels`). It is the second reading of the same run, and it exists because
hashing takes no allocation — a mint removed from a per-call path moves nothing in the
allocations column, so without this one the symbol-only program would be unmeasurable. The
figure is **recorded, not bounded**: it has no entry in `tests/allocation_baseline.rs`, since
the counter is a lib-side `cfg` an integration test cannot reach. The *scaling term* column
reads the allocations column; the symbol terms are quoted in the prose below.

| shape | what it exercises | allocations | symbols | scaling term |
|---|---|---|---|---|
| `shapes/tail_loop.koan` | 100 tail-recursive steps | 11 818 | 937 | 91.0 per step, linear |
| `shapes/operator_chain.koan` | 128-operand `+` chain, 127 dispatches | 5 424 | 744 | ≈23 per dispatch, mildly superlinear |
| `shapes/scope_walk_depth2_calls8.koan` | 8 dispatches down a 2-deep scope walk | 3 325 | 688 | — |
| `shapes/scope_walk_depth2_calls40.koan` | 40 dispatches down a 2-deep scope walk | 4 890 | 784 | 48.9 per dispatch |
| `shapes/scope_walk_depth10_calls8.koan` | 8 dispatches down a 10-deep scope walk | 4 739 | 848 | — |
| `shapes/scope_walk_depth10_calls40.koan` | 40 dispatches down a 10-deep scope walk | 6 301 | 944 | 48.8 per dispatch |
| `shapes/builtin_call_calls8.koan` | 8 three-parameter builtin calls | 3 071 | 672 | — |
| `shapes/builtin_call_calls40.koan` | 40 three-parameter builtin calls | 5 141 | 896 | 64.7 per call |
| `shapes/user_fn_params1_calls8.koan` | 8 one-parameter user-function calls | 2 902 | 640 | — |
| `shapes/user_fn_params1_calls40.koan` | 40 one-parameter user-function calls | 4 076 | 704 | 36.7 per call |
| `shapes/user_fn_params8_calls8.koan` | 8 eight-parameter user-function calls | 3 191 | 661 | — |
| `shapes/user_fn_params8_calls40.koan` | 40 eight-parameter user-function calls | 5 165 | 725 | 61.7 per call, 3.57 per parameter |
| `shapes/tagged_construct_calls8.koan` | 8 construct-and-match cycles over a two-variant `UNION` | 3 378 | 723 | — |
| `shapes/tagged_construct_calls40.koan` | 40 construct-and-match cycles over a two-variant `UNION` | 6 312 | 1 107 | 91.7 per cycle |
| *(empty program)* | interpreter startup and builtin seeding | 2 491 | 614 | — |

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
scopes before picking at the root. The two `builtin_call_calls*.koan` shapes are the same
differencing trick over a fourth axis, **call arity**: 8 and 40 repetitions of one
three-parameter builtin call (`MATCH … -> … WITH …`, bound as `value` / `return_type` /
`branches`), an arity the binary operator chain does not reach. Between them the four axes
cover where the execute path's allocation traffic scales.

The step term is exactly linear — 91.0 flat at 10, 50, 100 and 200 steps. The dispatch term
is not: marginal cost rises across the 16→32, 32→64, 64→128 and 128→256 operand doublings,
so a chain pays slightly more per operator the longer it gets. Below 16 operands the fixed
cost swamps the term, so the rise is only readable over the larger sizes. Whatever drives it
is unmeasured; the shapes are sized to the linear-enough middle rather than to the tail.

The walk term is **flat in depth**. Differencing the two call counts at one depth cancels
parse and setup and leaves 32 dispatches' marginal cost: 1 565 allocations at depth 2 against
1 562 at depth 10 — 48.9 and 48.8 per dispatch, the two depths indistinguishable. The walk's
per-scope buffers are hosted on the drain's step scratch arena
([dag-scheduler.md § The drain protocol](../workgraph/design/dag-scheduler.md#the-drain-protocol)),
so a deeper walk bumps more scratch bytes and takes no heap allocation for them. The grid's
absolute per-dispatch figure is higher than the operator chain's because each `PROBE y` is a
whole statement — its own node, frame and working expression — where a chain operand is one
dispatch inside a single statement's fold.

The arity term is **64.7 per call**, from 2 070 allocations for the 32 calls the two
`builtin_call` shapes differ by. It was 73.7 while a call re-keyed its arguments onto
parameter names; 8 per call of the drop is exactly what the schema-keyed argument view removes
at this arity — the 2n = 6 parameter-name copies, plus the argument map and the carrier map,
both replaced by a values-only slice on the step scratch arena
([label-interning.md](../design/label-interning.md)). The last 1 per call is the name copy a
symbol-keyed scope binding table no longer makes.

The **nominal-member** axis is the two `tagged_construct` shapes, 8 and 40 repetitions of one
`MATCH (Maybe (Some 1)) -> :Number WITH (…)` over a two-variant `UNION`. Each cycle reads the
union's `SetMember` node out of the registry, selects a variant out of the constructor's schema,
builds the tagged value and matches on its tag — so it is the shape that prices a *name that was
declared*, where the four axes above price names that are bound, looked up or passed. It costs
**91.7 per cycle**, from 2 934 allocations for the 32 cycles the shapes differ by, down from 92.7
when a Type token was carried as text. The 1-per-cycle drop is one 496-byte arena chunk — bumpalo's
first chunk, and the only allocation size that moves. A cycle's statement names five Type tokens
(the union binder, three variant-tag mentions and the return-type leaf) whose spellings no longer
reach the region the cycle opens, and the frame falls back under one chunk.

The **fixed** figure the four axes are read against is the empty program's own: interpreter
startup and builtin seeding, at **2 491**. Seeding registers every builtin overload, and each
registration writes a bucket key and a dispatch token; both hold a keyword's symbol rather than
its text, so the seeding pass copies no keyword bytes and mints no per-element string. The figure
is nearly inert to the symbol work: an empty program names no type, so parse-interned Type tokens
move it by one.

The marginal terms are where that work reads. Three of them fall by one — the step term 92.0 to
91.0, the walk term 49.9/49.8 to 48.9/48.8, the cycle term 92.7 to 91.7 — and the eight-parameter
call term falls by one too, 62.7 to 61.7. Every one of those is the same 496-byte arena chunk, and
it is the only allocation size that moves anywhere: a Type token's spelling is no longer bumped
into the region the call opens, so a frame that sat just over one chunk now sits under it. The
terms that do not move are the ones whose shapes were not near the boundary — a one-parameter call
at 36.7, a builtin call at 64.7 — and the operator chain, whose `+` names no type at all.

### Symbol mints

The symbols column reads the same way: a fixed term the empty program sets, and a marginal term
each differencing pair leaves behind.

The **fixed** symbol figure is **614**, down from 905 before a declaration recorded its spelling
under the digest it had already taken. Seeding is where almost all of a run's mints are, because
every builtin overload registers a signature and every parameter slot, bucket key and dispatch
token is declared as it goes. Each such spelling is a `StaticName` declared beside the body that
reads it
([label-interning.md § Names fixed in Rust source](../design/label-interning.md#names-fixed-in-rust-source)):
minted once for the process at first touch and compared by symbol everywhere after. What the 291
sheds is the second hash each *declaration* used to take — classifying the text and then handing
the same text to the interner, which hashed it again to key the record. The classified symbol now
carries into the recording, so a declaration mints once. That single change is 279 of the 291;
the other 12 are the eleven builtin type names, declared as statics rather than classified from
text wherever a seam needed one.

The marginal terms are what a call itself mints, seeding cancelled. A **builtin call** is 7, down
from 16. A **tagged construct-and-match** cycle is 12, down from 22 — the largest marginal drop
here. A **scope-walk dispatch** is 3, down from 4, at both depths: the reads the walk sheds are
depth-independent, exactly like its allocation term. A **tail-loop step** is 3.0, down from 8.0.
The **operator chain**, read against the empty program rather than a differencing pair, is 1.0 per
dispatch over its 127, down from 2.0. A **user-function call** is 2.0 per call, down from 3.0, and
still flat across the arities — the same at one parameter and at eight.

Two changes drive every one of those, in much the same proportion. The single-hash **declaration**
above is not only a seeding effect: a statement declares tokens as it runs, and each of them —
keyword, value name and Type name alike — now mints once where it minted twice. And a **Type
token** mints at the parse that classifies it and nowhere after, where each seam that read one
used to re-classify its text. The tagged cycle splits its drop of 10 between them, 7 and 3:
7 across the tokens it declares per cycle, and 3 on its `-> :Number` leaf alone, which cost four
mints a cycle and now costs the one the parse takes. What is left on the marginal terms is spelled
by the koan program on the value side — parameter names, a call's own keyword — and carrying those
from the parse boundary too is
[parse-interned identifiers](../roadmap/reduce_allocs/parse-interned-identifiers.md) and
[symbol-only keyword tokens](../roadmap/reduce_allocs/symbol-only-keyword-tokens.md). The flat
line across arities is unchanged in meaning: a parameter costs nothing to name, whichever side
declares it.

The allocations column does not move on any shape. A `StaticName`'s memo is a `LazyLock` over a
`Copy` digest, so forcing it heap-allocates nothing, and registration writes the interner entry
it always wrote.

## The regression test

`tests/allocation_baseline.rs` asserts the two absolute shapes' bracketed counts against a
stated bound — 11 846 for the loop, 5 451 for the chain, carrying 37 and 36 allocations of
headroom — and each differencing pair's marginal count against its measurement plus 31: 2 101
for the builtin call, 1 205 for the one-parameter user call, 831 for the seven-parameter slope,
2 965 for the tagged construction.
The bounds are tight by design: the margin is smaller than the 100 (loop), 127 (chain) or 32
(every differencing pair, which is how many repetitions they differ by) a single new allocation
on the scaling path would add, so one added allocation fails a test. Rebaselining is meant to be
a deliberate edit, and the failure message says so.

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
