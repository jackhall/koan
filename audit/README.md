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
| `shapes/tail_loop.koan` | 100 tail-recursive steps | 11 919 | 1 749 | 92.0 per step, linear |
| `shapes/operator_chain.koan` | 128-operand `+` chain, 127 dispatches | 5 425 | 1 163 | ≈23 per dispatch, mildly superlinear |
| `shapes/scope_walk_depth2_calls8.koan` | 8 dispatches down a 2-deep scope walk | 3 336 | 1 026 | — |
| `shapes/scope_walk_depth2_calls40.koan` | 40 dispatches down a 2-deep scope walk | 4 933 | 1 154 | 49.9 per dispatch |
| `shapes/scope_walk_depth10_calls8.koan` | 8 dispatches down a 10-deep scope walk | 4 758 | 1 306 | — |
| `shapes/scope_walk_depth10_calls40.koan` | 40 dispatches down a 10-deep scope walk | 6 352 | 1 434 | 49.8 per dispatch |
| `shapes/builtin_call_calls8.koan` | 8 three-parameter builtin calls | 3 072 | 1 036 | — |
| `shapes/builtin_call_calls40.koan` | 40 three-parameter builtin calls | 5 142 | 1 548 | 64.7 per call |
| `shapes/user_fn_params1_calls8.koan` | 8 one-parameter user-function calls | 2 903 | 947 | — |
| `shapes/user_fn_params1_calls40.koan` | 40 one-parameter user-function calls | 4 077 | 1 043 | 36.7 per call |
| `shapes/user_fn_params8_calls8.koan` | 8 eight-parameter user-function calls | 3 200 | 975 | — |
| `shapes/user_fn_params8_calls40.koan` | 40 eight-parameter user-function calls | 5 206 | 1 071 | 62.7 per call, 3.71 per parameter |
| `shapes/tagged_construct_calls8.koan` | 8 construct-and-match cycles over a two-variant `UNION` | 3 387 | 1 108 | — |
| `shapes/tagged_construct_calls40.koan` | 40 construct-and-match cycles over a two-variant `UNION` | 6 353 | 1 812 | 92.7 per cycle |
| *(empty program)* | interpreter startup and builtin seeding | 2 492 | 905 | — |

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

The step term is exactly linear — 92.0 flat at 10, 50, 100 and 200 steps. The dispatch term
is not: marginal cost rises across the 16→32, 32→64, 64→128 and 128→256 operand doublings,
so a chain pays slightly more per operator the longer it gets. Below 16 operands the fixed
cost swamps the term, so the rise is only readable over the larger sizes. Whatever drives it
is unmeasured; the shapes are sized to the linear-enough middle rather than to the tail.

The walk term is **flat in depth**. Differencing the two call counts at one depth cancels
parse and setup and leaves 32 dispatches' marginal cost: 1 597 allocations at depth 2 against
1 594 at depth 10 — 49.9 and 49.8 per dispatch, the two depths indistinguishable. The walk's
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
**92.7 per cycle**, from 2 966 allocations for the 32 cycles the shapes differ by, down from 98.7
when the variant schema was keyed by the tag's rendered text. The 6-per-cycle drop is the tag
`String` a construction used to bump into the tagged value plus the schema-key text clones a
variant selection made on the way in and out of the registry, both replaced by the classified
symbol the declaration already interned.

The **fixed** figure the four axes are read against is the empty program's own: interpreter
startup and builtin seeding, at **2 492**, down from 2 973 when a bucket key spelled its
keywords out. Seeding registers every builtin overload, and each registration writes a bucket
key and a dispatch token; both now hold a keyword's symbol where they held its text, so the
seeding pass copies no keyword bytes and mints no per-element string. Every absolute figure in
the table carries that drop, and a shape that declares overloads of its own carries a little more
of the same saving on top — the 10-deep scope-walk grid's shadowing definitions are the widest,
at 531. None of it reaches a differencing pair, which cancels registration along with the rest of
setup; the one marginal term that did move is the step term, 93.0 to 92.0, and that one is the
splice door's doing — a splice inherits the bucket key its node was constructed with instead of
bumping a second identical run.

### Symbol mints

The symbols column reads the same way: a fixed term the empty program sets, and a marginal term
each differencing pair leaves behind.

The **fixed** symbol figure is **905**, down from 1 322 before the Rust-side names were
declared. Seeding is where almost all of a run's mints are, because every builtin overload
registers a signature and each parameter slot classified its spelling from text at that moment
— once per registration, once per run, over spellings fixed in Rust source. Each such spelling
is now a `StaticName` declared beside the body that reads it
([label-interning.md § Names fixed in Rust source](../design/label-interning.md#names-fixed-in-rust-source)):
minted once for the process at first touch, recorded into the run's interner without hashing at
registration, and compared by symbol everywhere after. The 417 the empty program sheds is that
per-run re-classification.

The marginal terms are what a call itself mints, seeding cancelled. A **builtin call** is 16,
down from 20 — four per call, one for each slot read on the `MATCH` path, which now compares a
memoized symbol rather than hashing the parameter's spelling. A **tagged construct-and-match**
cycle is 22, down from 27. A **scope-walk dispatch** is 4, down from 6, at both depths: the
reads the walk sheds are depth-independent, exactly like its allocation term. The **operator
chain**, read against the empty program rather than a differencing pair, is 2.0 per dispatch
over its 127, down from 4.0.

A **user-function call** is flat at 3.0 per call — at one parameter and at eight, before and
after. A user function's parameter names are spelled by the koan program, so no Rust-side static
reaches them and this item cannot touch the term; moving it takes carrying the classified symbol
from the parse boundary
([parse-interned identifiers](../roadmap/reduce_allocs/parse-interned-identifiers.md)). That
flat line is the bound on what declaring names in Rust source can buy: it prices the machine's
own vocabulary, not the program's.

The allocations column does not move on any shape. A `StaticName`'s memo is a `LazyLock` over a
`Copy` digest, so forcing it heap-allocates nothing, and registration writes the interner entry
it always wrote.

## The regression test

`tests/allocation_baseline.rs` asserts the two absolute shapes' bracketed counts against a
stated bound — 11 947 for the loop, 5 452 for the chain, carrying 37 and 36 allocations of
headroom — and each differencing pair's marginal count against its measurement plus 31: 2 101
for the builtin call, 1 205 for the one-parameter user call, 2 997 for the tagged construction.
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
