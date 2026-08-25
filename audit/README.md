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
| `shapes/*.koan` | the recorded programs the figures in [`observe/alloc/`](../observe/alloc) are measured over | — |

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

## The recorded figures

```sh
python3 tools/alloc_audit.py              # sweep and report
python3 tools/alloc_audit.py --baseline   # sweep and record
```

The figures live in [`observe/alloc/`](../observe/alloc), as two kinds of file. A **sweep**
is one experiment: every shape measured in one sitting, in a file named for the commit it was
taken on — `fb4959f6.txt` — with the date and the tree's state in its header. The **trend** is
[`terms.txt`](../observe/alloc/terms.txt): the marginal terms differenced out of each sweep,
newest first, one block per commit. A sweep is on disk exactly while its terms are in the
trend, and the last five are kept.

**Nothing here quotes one.** A figure transcribed into prose goes stale in silence, and a
reader who cannot tell whether it still holds re-measures a base revision to find out — which
is the work the record exists to save. What this file carries is what the numbers *mean*: which
term prices which path, what moves one, and what a movement is evidence of.

`tools/alloc_audit.py` is the only writer. `tools/verify.sh` runs it every slate, read-only
unless `KOAN_REBASELINE` is set, and the pre-commit hook sets it and stages the result — so the
record and the change that moved it land in one commit. The slate passes `--quiet`, which keeps
the shape and term rows that moved against the recorded sweep and the bounds that drifted, and
drops the rest under a one-line summary; run the script without it for the whole sweep.

A shape's Δ column is exact: a single allocation more than the recorded sweep prints as `+1`,
which is the movement the bounds exist to catch. A term's is not, because a term is differenced
and divided out of a pair of readings and so carries rounding noise in its last printed place
that no allocation caused — one unit there reads as `=`, and two is the smallest movement a term
can report. A `+` on an entry's SHA marks a reading
taken over a tree with uncommitted changes, which every pre-commit entry is: its stamp names the
commit the reading was taken *on top of*. Absolute figures are not portable across machines or
toolchains; what they are for is the margin over an empty program, and its movement when the
execute path's allocation traffic changes.

A sweep file carries three readings of every shape:

- **allocations** — the whole-program total, from a debug build with `--features alloc-count`:
  interpreter startup, parse, and the run.
- **symbols** — the run's `symbols_minted` total: every `Symbol::of` and `KeywordSymbol::of_run`
  that reached the BLAKE3 funnel, counted where the two meet (`machine::model::labels`). It is
  the second reading of the same run, and it exists because hashing takes no allocation — a mint
  removed from a per-call path moves nothing in the allocations column, so without this one the
  symbol-only shapes would be unmeasurable.
- **bracketed** — the same run without process startup, from the bracket
  `tests/allocation_baseline.rs` opens around one interpret call. It is the reading the bounds
  are stated against, and it is `-` for a shape no test brackets.

`terms.txt` differences those into one figure per unit of work — over the sweep of the same SHA,
so a term and the readings behind it are always the same experiment. Both columns difference the
same way, so a term has an allocations reading and a symbols reading.

### The shapes

No shape can use comments: koan has none, and `#` is reserved for quoting. The prose that would
have headed each file is here instead.

| shape | axis | what it exercises |
|---|---|---|
| `tail_loop_steps10`, `tail_loop_steps100` | step | a tail-recursive countdown. TCO holds the node table and the regions flat, so what the pair differences out is a per-*step* cost rather than a per-frame one |
| `operator_chain_operands16`, `operator_chain_operands128` | dispatch | one flat left fold, so every `+` is a dispatch — a bucket walk, a pick, and a working-expression rebuild |
| `scope_walk_depth{2,10}_calls{8,40}` | scope walk | a depth × call-count grid over how far a dispatch's scope walk reaches: an innermost body of *m* `PROBE y` statements under *n* nested scopes that each shadow the `PROBE` bucket with a non-admitting same-key overload, so every dispatch strict- and hard-rejects at all *n* shadow scopes before picking at the root |
| `builtin_call_calls{8,40}` | call arity | repetitions of one three-parameter builtin call (`MATCH … -> … WITH …`, bound as `value` / `return_type` / `branches`), an arity the binary operator chain does not reach |
| `user_fn_params{1,8}_calls{8,40}` | frame bind | a parameter-count × call-count grid over a user-defined `FN`, which is the frame bind's own axis |
| `tagged_construct_calls{8,40}` | nominal member | repetitions of one `MATCH (Maybe (Some 1)) -> :Number WITH (…)` over a two-variant `UNION` — the shape that prices a name that was *declared*, where the axes above price names that are bound, looked up or passed |
| `empty` | fixed | nothing: interpreter startup and builtin seeding, which every other shape carries |

A pair differs only in how many repetitions it runs, so differencing it cancels startup, the
declaration, and the parse of everything the two share — leaving the marginal cost of the
repetitions themselves, the parse of the extra statements included.

### What the terms say

The **step** term is exactly linear: the same per-step figure at 10, 50, 100 and 200 steps, of
which the committed pair is two. The **dispatch** term is not — marginal cost rises across the
16→32, 32→64, 64→128 and 128→256 operand doublings, so a chain pays slightly more per operator
the longer it gets. Below 16 operands the fixed cost swamps the term, so the rise is only
readable over the larger sizes, and the committed pair spans the linear-enough middle rather
than the tail. Whatever drives the rise is unmeasured.

The walk term is **flat in depth**: `scope_walk_depth2` and `scope_walk_depth10` are
indistinguishable, and `scope_walk_scope` — what one extra scope costs one dispatch, the two
differenced again — reads as zero. The walk's per-scope buffers are hosted on the drain's step
scratch arena
([dag-scheduler.md § The drain protocol](../workgraph/design/dag-scheduler.md#the-drain-protocol)),
so a deeper walk bumps more scratch bytes and takes no heap allocation for them. The grid's
absolute per-dispatch figure is higher than the operator chain's because each `PROBE y` is a
whole statement — its own node, frame and working expression — where a chain operand is one
dispatch inside a single statement's fold.

The **arity** term is `builtin_call`. Its drop when the schema-keyed argument view landed is
exactly what that change removes at this arity: the 2n parameter-name copies a bind used to
make, plus the argument map and the carrier map, both replaced by a values-only slice on the
step scratch arena ([label-interning.md](../design/label-interning.md)) — and one name copy per
call that a symbol-keyed scope binding table no longer makes.

The **frame bind** terms are `user_fn_params1`, `user_fn_params8` and the `user_fn_parameter`
slope between them. A user call binds each parameter into the fresh per-call scope under the
name the signature's parameter schema carries — the classified symbol the binding table keys
by — so the bind reaches no interner and builds no string. Nor does the call's declared-return
contract: it seals a `Copy` call site and the callable's interned type handle, and renders trace
text only on the error arm that spends it. What stands in the slope is per-*argument* cost the
bind does not own: the extra source the call site parses, and the delivery carrier each argument
travels in.

Some terms straddle a chunk boundary. bumpalo's first chunk is 496 bytes, and a frame that does
not fit it makes the region take a second one — one more allocation per repetition, and the only
allocation *size* that moves anywhere in this record. The step, walk, eight-parameter call and
tagged-cycle terms all sit within a few bytes of it, and have crossed it in both directions:
carrying a Type token as a symbol rather than as text took the tagged cycle under, and deleting
`ScopeKind::Module`'s write-only `name` put it back over — a deletion that *shrank* `Scope`.
What decides a crossing is how the whole frame packs, so its direction does not follow the
direction of the size change. The terms nowhere near the boundary are the one-parameter call,
the builtin call, and the operator chain, whose `+` opens no frame of its own.

The **fixed** term is the empty program's own reading, and it is inert to what a program *names*
— an empty program names nothing. It stays proportional to the *count* of registered overloads,
which every run pays once at seeding and every shape therefore carries. Registering a callable
renders no signature text: a registration used to summarize its own signature at seal time, a
`Vec` of per-element renderings and a `String` per element and the joined result and a bump copy
of that text into the bucket entry's region, all for a `DuplicateOverload` diagnostic a correct
program never sees. The diagnostic renders from the standing entry's stored dispatch token on
the error arm instead, and the entry stores no text at all.

The same rendering was paid at every user `FN` and `OP`, so a shape falls by its own declarations
on top of the seeding constant — the element `Vec`, the keyword's rendering, two strings per
argument slot, the join and the format that wraps it, with the per-slot pair dominating at eight
parameters. That is a *declaration* cost, so the two repetition counts of a pair fall by the same
amount and no marginal term moves with it. No term in `terms.txt` isolates it; the scope-walk
grid, whose two depths declare different numbers of shadowing overloads, is where it is visible
at all.

Registration being a per-shape constant is what keeps every differencing pair honest: it cancels
exactly. So the two things that move this record without any allocation being added to a marginal
path are a change in the *count* of registered overloads, which moves the fixed term and every
absolute figure with it, and a change to what a frame holds by *byte size*, which slides the
boundary-straddling terms across bumpalo's first chunk. A crossing is still one more call to the
allocator per repetition — what it is not is traffic a marginal path newly makes, which is why
the same terms move in both directions as layouts change.

### Symbol mints

The symbols column reads the same way: a fixed term the empty program sets, and a marginal term
each differencing pair leaves behind. Nothing in it moves with the allocations column: rendering
a signature resolved its labels through the interner's display path, which mints nothing, and a
`StaticName`'s memo is a `LazyLock` over a `Copy` digest, so forcing it heap-allocates nothing.

Seeding is where almost all of a run's mints are, because every builtin overload registers a
signature and every parameter slot, bucket key and dispatch token is declared as it goes. What
the fixed term shed was one hash per keyword element of every builtin signature: a draft
classified the spelling it was written with, and registration classified the normalized form
again to key its record. The draft door normalizes and interns once and registration copies the
classified symbol, so a keyword element is hashed where it is written and nowhere after. Each
such spelling is a `StaticName` declared beside the body that reads it
([label-interning.md § Names fixed in Rust source](../design/label-interning.md#names-fixed-in-rust-source)):
minted once for the process at first touch and compared by symbol everywhere after.

Not all of the process's one-time mints sit in that fixed figure. A keyword the machine compares
against — `AS`, `->`, `_`, the binder's key specs, the reserved operator names — is such a
`StaticName`, and it mints when the path that reads it is first walked, which is in some *program*
rather than at seeding. So a shape read against the empty program picks up a small constant for
the statics it is the first to touch. It is a per-process constant, not a per-repetition one — the
two repetition counts of every pair pick up the same amount, so it lands in the fixed part of a
reading and in no marginal term.

One marginal term is nothing at all. A **tail-loop step** mints zero: the shape's whole symbol
figure is the same at 10, 50, 100 and 200 steps, so a loop in steady state hashes no text. That is
the cleanest statement of what a parse-minted name buys — the step's two statements spell `n`
twice and its `MATCH` binds `it` once, and all three are symbols the parse and the arm binder
already hold. A **scope-walk dispatch** mints for its `PROBE y` and its value name, and the
saving is depth-independent exactly like its allocation term. A **user-function call** mints the
same at one parameter and at eight: a parameter costs nothing to name, whichever side declares it.

What the *declaration* side sheds is visible in the scope-walk grid, the only pair of shapes here
that differ in how many overloads they declare: two mints per extra overload, the same double hash
as seeding's, now taken once.

The **tagged** shapes' symbol figure fell with the field list. A list used to hand its names on as
text — the list parse built a `String` per name, `typed_field_list` interned each one to key the
schema, and the tag binder hashed the same text again to declare the member, two mints per declared
name after the parse that classified it had already minted one. The list now yields those
parse-minted symbols, so the two-variant `UNION` at the head of the shape declares its tags without
hashing anything, and the record type it keys carries each key's binding class instead of erasing it
at the intern boundary. The user-function shapes do not move with it: an FN signature never
re-hashed a parameter name, it rendered one — an allocation, not a mint — so what a parameter
declaration sheds lands entirely in the allocations column. The one mint that path did make, one per
parameter name in the return-surface scan, is on the branch where the surface arrives as an
unresolved name; `-> Number` reaches the definition already lowered to a type, so no shape here
walks it.

The **operator chain**'s mints are its `+` tokens, its `PRINT`, its probe key, and the statics it
first touches. The probe is the entry this shape records. An `OperatorChain` node keys the operator
registry by a digest over the run of operators it names, and that digest was minted twice at parse
— once when the bracket frame closed into a node, and again when the redundant-wrapper peel rebuilt
the node and refilled its cache from scratch. The peel carries the survivor's cache, so the chain
mints one probe where it minted two, and the registration side mints its powerset keys through the
same run-digest constructor ([operators.md](../design/operators.md)) — so a registered key and a
live chain's probe agree by construction, and no probe path reads text at all.

An earlier pair of changes set the marginal terms up. The single-hash **declaration** above is not
only a seeding effect: a statement declares tokens as it runs, and each of them — keyword, value
name and Type name alike — mints once where it minted twice. And a **Type token** mints at the
parse that classifies it and nowhere after, where each seam that read one used to re-classify its
text. An `Identifier` part carries the symbol its parse minted and every reader down to the lookup
ladder takes it
([label-interning.md § Where text becomes a symbol](../design/label-interning.md#where-text-becomes-a-symbol));
a `Keyword` part carries its symbol and nothing else, so the spelling a diagnostic prints is
resolved out of the run's label table rather than carried beside every token.

## The regression test

`tests/allocation_baseline.rs` is what makes a movement fail rather than go unnoticed. It brackets
each shape and asserts its count against a bound stated in the test: absolute for the tail loop and
the operator chain, and marginal — the difference between a pair — for the builtin call, the two
user-call arities, the parameter slope and the tagged construction. The scope-walk grid is held to a
*shape* rather than a count: its test differences the two depths and bounds the growth per extra
dispatch, so it needs no rebaselining when an unrelated term moves, which is what suits it to a claim
about depth-independence.

The bounds are tight by design. A bound sits a little over its measurement, and that headroom is
smaller than the number of repetitions the bounded path runs across the measurement — so one
allocation added to a per-step, per-dispatch or per-call path fails a test, and rebaselining is a
deliberate edit. `tools/alloc_audit.py` prints every bound's headroom beside the reading it is set
over and flags one that has drifted: a bound under its measurement (the test fails) or one loose
enough to miss a single added allocation.

What a bound defends is exactly that: **no allocation added to a marginal path.** It is not a claim
that the absolute figure never rises. The two movements above — a registered overload, and a frame
whose byte size crosses bumpalo's first chunk — are expected to force a rebaseline without any such
allocation existing. When a bound moves, the question the failure message asks is which of the three
it was. Only the first is a regression.

`allocations_for` runs each shape once *outside* its bracket. The bounds are tight enough that one
lazy static added later would break them by test order alone — whichever test reached it first would
carry its whole initialisation cost. Warming with the shape's own source rather than a stand-in is
what keeps that coverage total: whatever the shape initialises is warm by construction, including
statics added later. It is also what leaves the bracketed reading so close to the whole-program one,
the gap being process startup and little else.
