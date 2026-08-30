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
| `shapes/*.koan` | the recorded programs the figures in [`observe/alloc.txt`](../observe/alloc.txt) are measured over | — |

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

## The attribution profiler

The counter prices a term; the `dhat` cargo feature names the sites a term is made of. It
installs [dhat-rs](https://docs.rs/dhat)'s allocator as the binary's global allocator, which
records an untrimmed backtrace per allocation and writes `dhat-heap.json` after the run. Run
one shape at two sizes and difference the block counts per site with
`tools/dhat_diff.py` — a site whose count scales with the size difference is on the per-unit
path, and everything constant (startup, seeding) cancels:

```sh
cargo run --features dhat -- audit/shapes/wide_n10.koan && mv dhat-heap.json small.json
cargo run --features dhat -- audit/shapes/wide_n100.koan && mv dhat-heap.json big.json
python3 tools/dhat_diff.py small.json big.json 90                      # site table, per step
python3 tools/dhat_diff.py small.json big.json 90 --detail 'arm_tail'  # full stacks
```

The two instruments are deliberately separate features. The counter *wraps* mimalloc, so a
counted build allocates through the shipped allocator and its wall-clock stays comparable; the
dhat allocator must *own* the global slot (it delegates to the system allocator) and pays a
backtrace per allocation, so its runs are neither count- nor time-comparable with the sweep's.
A `compile_error!` in `src/main.rs` guards the pair. The counter's figures are the currency of
`observe/alloc.txt`; a dhat profile is the instrument to reach for when a term moves and the
question is *where*.

## The recorded figures

```sh
python3 tools/alloc_audit.py              # sweep and report
python3 tools/alloc_audit.py --baseline   # sweep and record
```

A reading is a **debug** build's, so anything a debug build does lands in it — an assertion that
allocates is counted per step exactly as the work it guards is. The scheduler's acyclicity guard
([`Scheduler::would_create_cycle`](../workgraph/src/scheduler.rs)) is the one such guard on the
per-step path, and it holds its walk stack and visit stamps on the scheduler across calls, so it
adds nothing to any term. What is left between a debug reading and a release one is a few
allocations per step, and both move together.

The figures live in [`observe/alloc.txt`](../observe/alloc.txt), one row per commit swept,
newest first, the last five kept — the same trend log
[`observe/complexity.txt`](../observe/complexity.txt) and
[`observe/coverage.txt`](../observe/coverage.txt) are, rendered by the same
`tools/trendlog.py`, which sits each column's name directly over its own column. A row is one
experiment: every shape measured in one sitting, under one stamp. Rows whose SHA has left
HEAD's history are dropped on the next recording sweep.

**Nothing here quotes one.** A figure transcribed into prose goes stale in silence, and a
reader who cannot tell whether it still holds re-measures a base revision to find out — which
is the work the record exists to save. What this file carries is what the numbers *mean*: which
term prices which path, what moves one, and what a movement is evidence of.

`tools/alloc_audit.py` is the only writer. `tools/verify.sh` runs it every slate, read-only
unless `KOAN_REBASELINE` is set, and the pre-commit hook sets it and stages the result — so the
record and the change that moved it land in one commit. The slate passes `--quiet`, which keeps
the shape and term rows that moved against the recorded sweep and the bounds that drifted, and
drops the rest under a one-line summary; run the script without it for the whole sweep.

A shape's Δ column is exact: a single allocation more than the recorded row prints as `+1`,
which is the movement the bounds exist to catch, and keeping the row's columns per-shape rather
than per-term is what preserves it. A term's is not, because a term is differenced
and divided out of a pair of readings and so carries rounding noise in its last printed place
that no allocation caused — one unit there reads as `=`, and two is the smallest movement a term
can report. A `+` on an entry's SHA marks a reading
taken over a tree with uncommitted changes, which every pre-commit entry is: its stamp names the
commit the reading was taken *on top of*. Absolute figures are not portable across machines or
toolchains; what they are for is the margin over an empty program, and its movement when the
execute path's allocation traffic changes.

A row carries two readings of every shape, in the column order its header names:

- **allocations** — the whole-program total, from a debug build with `--features alloc-count`:
  interpreter startup, parse, and the run.
- **symbols** — the run's `symbols_minted` total: every `Symbol::of` and `KeywordSymbol::of_run`
  that reached the BLAKE3 funnel, counted where the two meet (`machine::model::labels`). It is
  the second reading of the same run, and it exists because hashing takes no allocation — a mint
  removed from a per-call path moves nothing in the allocations column, so without this one the
  declaration shape's saving would be unmeasurable.

The **terms are derived from a row on read, not stored.** A term is a difference of two of the
row's columns over the gap in `n`, so recording it would round away the exact per-shape figure
the row exists to keep — and a stored term could disagree with the readings beside it. Deriving
means the sweep's terms and the recorded sweep's are always computed the same way, out of the
same experiment, and that the whole record is one file with one writer.

A third reading, **bracketed** — the same run without process startup, from the bracket
`tests/allocation_baseline.rs` opens around one interpret call — is measured fresh on every run
and never recorded. It is what the bounds are stated against, so it is read to check a bound's
headroom and never differenced against an earlier sweep.

### The shapes

No shape can use comments: koan has none, and `#` is reserved for quoting. The prose that would
have headed each file is here instead.

Four shapes, each with **one scaling parameter `n`**. Three of them are two committed files that
differ in `n` and in nothing else, so differencing a pair cancels interpreter startup, the shape's
declarations and its parse *exactly* — leaving the marginal cost of one unit of `n` and nothing
around it. The fourth is the empty program, which is the fixed cost the other three carry.

| shape | `n` scales | what it exercises |
|---|---|---|
| `wide_n10`, `wide_n100` | tail-recursive steps | one driver whose body reaches across the runtime per iteration: a record construct and field read, a module opened with `USING`, a tagged-union construct and `MATCH`, an operator chain, an overloaded user call, a record projection through `FROM`, a `TRY` and a `CATCH`, a quote and its evaluation, a `NEWTYPE`, `TYPE OF`, list and map literals, and an anonymous function applied by named argument. TCO holds the node table and the regions flat, so the term is per-*step* churn |
| `deep_n10`, `deep_n100` | live frames | the same body with the recursive call out of tail position, so every iteration's frame is still standing when the next runs. The axis `wide` structurally cannot see |
| `declare_n10`, `declare_n100` | declared names | a `UNION`, a record `NEWTYPE`, a `SIG`, a `MODULE` and an `FN` signature, each carrying `n` names. Nothing runs, so this is registration alone — the axis the two recursion-driven shapes hold constant |
| `empty` | — | nothing: interpreter startup and builtin seeding, which every other shape carries |

The two recursion-driven shapes are **deliberately broad rather than isolating one path**. Each
prices a composite, so a regression anywhere on the execute path lands on one of them; what they
report is *that* something moved, not where. Naming the site is the attribution profiler's job —
run the pair under the `dhat` feature and difference it with `tools/dhat_diff.py`. That division
is the reason there are four shapes and not twenty: a per-axis shape answers "where" only for the
axis someone thought to carve out in advance, and dhat answers it for every axis at once.

`declare` is the one shape whose two sizes are mechanical rather than hand-written. Regenerate it
by changing `n` in the loop that emits it — five declaration forms, each with names `Tag1…Tagn`,
`f1…fn`, `v1…vn`, `m1…mn`, `p1…pn`.

### What the terms say

The **wide_step** term is exactly linear: the same per-step figure at 10, 20, 50 and 100 steps, of
which the committed pair is two. That is what makes an absolute bound on `wide_n100` a statement
about the per-step path — a movement in it is `n` times whatever was added to one step.

The **deep_frame** term is linear in the same way, and sits a small constant above `wide_step`:
the same per-frame figure at 10, 20, 50 and 100 depth, a few allocations more than the wide body's
per-step one. That gap is what a standing frame costs over a retired one, and it is the whole of
what depth adds — no path that runs once per step walks or hashes anything whose size is the number
of live frames. A `deep_frame` that pulls away from `wide_step` is therefore this shape's own
signal, and the only reading either recursion shape gives that the other cannot.

The **declare_name** term prices the declaration side across five forms at once, so it moves when
any of them changes what it builds per name. Registering a callable renders no signature text: a
registration used to summarize its own signature at seal time, a `Vec` of per-element renderings
and a `String` per element and the joined result and a bump copy of that text into the bucket
entry's region, all for a `DuplicateOverload` diagnostic a correct program never sees. The
diagnostic renders on the error arm instead — the colliding registration's own untyped bucket key
beside the standing entry's stored dispatch token — and neither the entry nor the write op that
installs it holds any text.

The **fixed** term is the empty program's own reading, and it is inert to what a program *names* —
an empty program names nothing. It stays proportional to the *count* of registered overloads, which
every run pays once at seeding and every other shape therefore carries. Registration being a
per-shape constant is what keeps every differencing pair honest: it cancels exactly. So the one
thing that moves this record without any allocation being added to a marginal path is a change in
that count, which moves the fixed term and every absolute figure with it.

No term straddles a chunk boundary. A region asks its bump for a first chunk sized to hold a whole
frame's residency (`FIRST_CHUNK_BYTES` in
[workgraph/src/witnessed/region.rs](../workgraph/src/witnessed/region.rs)), and the measured
per-call high-water across these shapes sits under that chunk's usable bytes. So a region takes
exactly one chunk, and what a frame holds moves this record only by changing how many objects a
step allocates, not how large they are. A layout change big enough to push a frame's residency past
the chunk would put the boundary back, which is what the headroom is for.

### Symbol mints

The symbols column reads the same way: a fixed term the empty program sets, and a marginal term
each differencing pair leaves behind. Nothing in it moves with the allocations column — a
`StaticName`'s memo is a `LazyLock` over a `Copy` digest, so forcing it heap-allocates nothing.

Seeding is where almost all of a run's mints are, because every builtin overload registers a
signature and every parameter slot, bucket key and dispatch token is declared as it goes. What the
fixed term shed was one hash per keyword element of every builtin signature: a draft classified the
spelling it was written with, and registration classified the normalized form again to key its
record. The draft door normalizes and interns once and registration copies the classified symbol,
so a keyword element is hashed where it is written and nowhere after. Each such spelling is a
`StaticName` declared beside the body that reads it
([label-interning.md § Names fixed in Rust source](../design/label-interning.md#names-fixed-in-rust-source)):
minted once for the process at first touch and compared by symbol everywhere after.

Not all of the process's one-time mints sit in that fixed figure. A keyword the machine compares
against — `AS`, `->`, `_`, the binder's key specs, the reserved operator names — is such a
`StaticName`, and it mints when the path that reads it is first walked, which is in some *program*
rather than at seeding. So a shape read against the empty program picks up a small constant for the
statics it is the first to touch. It is a per-process constant, not a per-unit one — both sizes of
every pair pick up the same amount, so it lands in the fixed part of a reading and in no marginal
term.

`declare_name` is where the mint column has a marginal term worth reading, since it is the only
shape whose `n` scales names rather than work. A **declaration** mints once where it minted twice:
a statement declares tokens as it runs, and each of them — keyword, value name and Type name alike
— is hashed where it is written. A **Type token** mints at the parse that classifies it and nowhere
after, where each seam that read one used to re-classify its text. An `Identifier` part carries the
symbol its parse minted and every reader down to the lookup ladder takes it
([label-interning.md § Where text becomes a symbol](../design/label-interning.md#where-text-becomes-a-symbol));
a `Keyword` part carries its symbol and nothing else, so the spelling a diagnostic prints is
resolved out of the run's label table rather than carried beside every token. A record's field list
yields its parse-minted symbols rather than handing names on as text, so a declared field is not
re-hashed to key the schema.

`wide_step` and `deep_frame` mint a small constant per unit rather than zero, which is what a
steady-state loop should reach: a tail loop's own two statements spell `n` twice and its `MATCH`
binds `it` once, and all three are symbols the parse and the arm binder already hold. No single
construct in the body accounts for the residue in isolation — each was measured alone in a bare
loop and minted nothing per step — so what remains is a combination effect, unattributed and worth
a look.

## The regression test

`tests/allocation_baseline.rs` is what makes a movement fail rather than go unnoticed. One test per
shape, four in all: it brackets the shape and asserts its count against a bound stated in the test —
absolute for `empty`, `wide_n100` and `deep_n100`, and marginal for `declare`, whose bound is on the
difference between its two sizes.

The bounds are tight by design. A bound sits a little over its measurement, and that headroom is
smaller than the number of units of `n` the bounded path runs across the measurement — so one
allocation added to a per-step, per-frame or per-name path fails a test, and rebaselining is a
deliberate edit. `empty` is the exception to the arithmetic, not the discipline: nothing repeats in
it, so one added allocation adds exactly one, and its headroom is set to what a seeding change of
any real size would exceed. `tools/alloc_audit.py` prints every bound's headroom beside the reading
it is set over and flags one that has drifted: a bound under its measurement (the test fails) or one
loose enough to miss a single added allocation.

What a bound defends is exactly that: **no allocation added to a marginal path.** It is not a claim
that the absolute figure never rises. The movement above — a registered overload — is expected to
force a rebaseline without any such allocation existing. When a bound moves, the question the
failure message asks is which of the two it was. Only the first is a regression.

`allocations_for` runs each shape once *outside* its bracket. The bounds are tight enough that one
lazy static added later would break them by test order alone — whichever test reached it first would
carry its whole initialisation cost. Warming with the shape's own source rather than a stand-in is
what keeps that coverage total: whatever the shape initialises is warm by construction, including
statics added later. It is also what leaves the bracketed reading so close to the whole-program one,
the gap being process startup and little else.
