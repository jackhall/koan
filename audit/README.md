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
| `shapes/tail_loop.koan` | 100 tail-recursive steps | 11 957 | 641 | 92.0 per step, linear |
| `shapes/operator_chain.koan` | 128-operand `+` chain, 127 dispatches | 5 463 | 748 | ≈23 per dispatch, mildly superlinear |
| `shapes/scope_walk_depth2_calls8.koan` | 8 dispatches down a 2-deep scope walk | 3 374 | 682 | — |
| `shapes/scope_walk_depth2_calls40.koan` | 40 dispatches down a 2-deep scope walk | 4 971 | 746 | 49.9 per dispatch |
| `shapes/scope_walk_depth10_calls8.koan` | 8 dispatches down a 10-deep scope walk | 4 796 | 834 | — |
| `shapes/scope_walk_depth10_calls40.koan` | 40 dispatches down a 10-deep scope walk | 6 390 | 898 | 49.8 per dispatch |
| `shapes/builtin_call_calls8.koan` | 8 three-parameter builtin calls | 3 110 | 669 | — |
| `shapes/builtin_call_calls40.koan` | 40 three-parameter builtin calls | 5 180 | 861 | 64.7 per call |
| `shapes/user_fn_params1_calls8.koan` | 8 one-parameter user-function calls | 2 941 | 636 | — |
| `shapes/user_fn_params1_calls40.koan` | 40 one-parameter user-function calls | 4 115 | 668 | 36.7 per call |
| `shapes/user_fn_params8_calls8.koan` | 8 eight-parameter user-function calls | 3 238 | 650 | — |
| `shapes/user_fn_params8_calls40.koan` | 40 eight-parameter user-function calls | 5 244 | 682 | 62.7 per call, 3.71 per parameter |
| `shapes/tagged_construct_calls8.koan` | 8 construct-and-match cycles over a two-variant `UNION` | 3 425 | 720 | — |
| `shapes/tagged_construct_calls40.koan` | 40 construct-and-match cycles over a two-variant `UNION` | 6 391 | 1 072 | 92.7 per cycle |
| *(empty program)* | interpreter startup and builtin seeding | 2 530 | 618 | — |

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
**92.7 per cycle**, from 2 966 allocations for the 32 cycles the shapes differ by. One of those 32 is
an arena chunk rather than heap traffic — bumpalo's 496-byte first chunk, and the only allocation
size that moves anywhere in this table. The cycle's frame sits within a few bytes of that boundary
and has crossed it twice: carrying a Type token as a symbol rather than as text took it under, to
91.7, and deleting `ScopeKind::Module`'s write-only `name` put it back over. What decides a crossing
is how the whole frame packs, so its direction does not follow the direction of the size change —
that deletion *shrank* `Scope`, 464 bytes to 448.

The **fixed** figure the four axes are read against is the empty program's own: interpreter
startup and builtin seeding, at **2 530**. Seeding registers every builtin overload, and each
registration writes a bucket key and a dispatch token; both hold a keyword's symbol rather than
its text, so the seeding pass copies no keyword bytes and mints no per-element string. The figure
is nearly inert to the symbol work itself — an empty program names nothing, so parse-interned
tokens move it by one — but it is not inert to the *count* of overloads: registering the two
dynamic `ATTR` reads costs 38, and since every run pays seeding once, that 38 lands on every shape
in the table alike.

The marginal terms move the other way, and it is the same four that moved last time: the step term
91.0 to 92.0, the walk term 48.9/48.8 to 49.9/49.8, the eight-parameter call term 61.7 to 62.7, the
cycle term 91.7 to 92.7. Each is one 496-byte arena chunk per repetition — the same unit and the
same four frames, which straddle the boundary and so move together whichever way a layout change
pushes them. The terms that do not move are the ones whose frames are nowhere near it: a
one-parameter call at 36.7, a builtin call at 64.7, and the operator chain, whose `+` opens no frame
of its own.

No shape gains heap traffic. Every movement in this column is either the seeding constant or a chunk
crossing; the value-side symbol work takes allocations off the execute path and adds none.

### Symbol mints

The symbols column reads the same way: a fixed term the empty program sets, and a marginal term
each differencing pair leaves behind.

The **fixed** symbol figure is **618** — 614, plus 4 for the two dynamic `ATTR` overloads. The 614
was itself down from 905, before a declaration recorded its spelling under the digest it had
already taken. Seeding is where almost all of a run's mints are, because
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

Every marginal term falls, and one of them falls to nothing. A **tail-loop step** mints **0.0**:
the shape's whole figure is 641 at 10 steps, at 50, at 100 and at 200, so a loop in steady state
hashes no text at all. That is the cleanest statement of what a parse-minted name buys — the step's
two statements spell `n` twice and its `MATCH` binds `it` once, and all three are symbols the parse
and the arm binder already hold. A **scope-walk dispatch** is 2, down from 3, at both depths — one
`PROBE y`, one value name, one mint gone, and the saving is depth-independent exactly like its
allocation term. A **user-function call** is 1.0 per call, down from 2.0, still flat across the
arities: the same at one parameter and at eight. A **builtin call** is 6, down from 7, and a
**tagged construct-and-match** cycle 11, down from 12; both shapes are `MATCH` statements, and the
1 each sheds is the arm binder that used to classify `it` afresh on every arm taken.

One term does not move: the **operator chain**, read against the empty program rather than a
differencing pair, is 1.0 per dispatch over its 127. A `+` chain binds nothing and names nothing —
what it still mints is its own keyword.

An earlier pair of changes set those terms up. The single-hash **declaration**
above is not only a seeding effect: a statement declares tokens as it runs, and each of them —
keyword, value name and Type name alike — now mints once where it minted twice. And a **Type
token** mints at the parse that classifies it and nowhere after, where each seam that read one
used to re-classify its text. The tagged cycle splits its drop of 10 between them, 7 and 3:
7 across the tokens it declares per cycle, and 3 on its `-> :Number` leaf alone, which cost four
mints a cycle and now costs the one the parse takes. The value side is now the same story:
[parse-interned identifiers](../roadmap/reduce_allocs/parse-interned-identifiers.md) is what the
drops above measure. What is left on the marginal terms is spelled by the program's
**keywords** — a statement's leading token, an operator's — and carrying those from the parse
boundary too is
[symbol-only keyword tokens](../roadmap/reduce_allocs/symbol-only-keyword-tokens.md). The flat
line across arities is unchanged in meaning: a parameter costs nothing to name, whichever side
declares it.

The symbol work costs nothing in the allocations column. A `StaticName`'s memo is a `LazyLock` over
a `Copy` digest, so forcing it heap-allocates nothing, and registration writes the interner entry it
always wrote; what moves that column here is frame layout and overload count, above.

## The regression test

`tests/allocation_baseline.rs` asserts the two absolute shapes' bracketed counts against a
stated bound — 11 985 for the loop, 5 490 for the chain, carrying 37 and 36 allocations of
headroom — and each differencing pair's marginal count against its measurement plus 31: 2 101
for the builtin call, 1 205 for the one-parameter user call, 863 for the seven-parameter slope,
2 997 for the tagged construction.
The bounds are tight by design: the margin is smaller than the 100 (loop), 127 (chain) or 32
(every differencing pair, which is how many repetitions they differ by) a single new allocation
on the scaling path would add, so one added allocation fails a test. Rebaselining is meant to be
a deliberate edit, and the failure message says so.

What a bound defends is exactly that: **no allocation added to a per-step, per-dispatch or
per-call path.** It is not a claim that the absolute figure never rises. Two things move these
numbers without any such allocation existing, and both are expected to force a rebaseline:
registering a builtin overload, which every run pays once at seeding and every shape here
therefore carries; and a change to what a frame holds by *byte size*, which slides the four
boundary-straddling terms across bumpalo's 496-byte first chunk in whichever direction it packs.
When a bound moves, the question the failure message asks is which of the three it was. Only the
first — a new allocation on a marginal path — is a regression.

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
