# Testing and linting Koan

Four layers, each with a distinct job:

1. **`cargo test`** — every unit test in the modules the rewrite keeps, run on every push and PR.
2. **`cargo clippy` / `cargo fmt`** — lints and formatting.
3. **The Miri audit slate** — targeted memory-safety coverage for every unsafe
   site in the runtime, run under tree borrows.
4. **The region debug audits** — debug-only over-pinning diagnostics over a real
   program run, reported and never enforced.

## Two verification tiers

[`tools/verify.sh`](tools/verify.sh) runs one of two tiers, and the tier is the argument:

```sh
tools/verify.sh            # routine — what the pre-commit hook runs
tools/verify.sh --total    # total — what CI runs, and what you run before a merge
```

The **routine** tier answers "did this change break anything": the unsafe-site drift check, tests
and doctests, the cellgraph surface checks, clippy, doc links. It measures nothing, so it costs
seconds. It also reads the changed paths and narrows itself — a Markdown-only change runs the link
audit alone, a change confined to `workgraph/` or `cellgraph/` runs that crate's slate and reports
the crates above it rather than gating on them.

The **total** tier adds everything that costs minutes: coverage instrumentation, the
[Miri audit slate](#the-miri-audit-slate), the modgraph complexity score, and a property sweep 32×
deeper than the routine one. It reads no change scope — it always runs the whole workspace — and it
is the only tier that rebaselines the trend logs under `observe/`, and then only with
`KOAN_REBASELINE=1`. CI runs it on every push and PR against `master`.

Property depth is the one variable `PROPTEST_CASES`, which `ProptestConfig::default()` reads: the
routine tier sets 64, the total tier 2048, and every property module states its share of that
default (`crate::tests::case_share`) rather than a literal, so the modules' relative depths hold at
whatever the tier — or you — ask for. No module goes below 64 cases.

```sh
PROPTEST_CASES=16384 tools/verify.sh --total   # an overnight sweep of the lattice laws
```

## The pending rewrite

The runtime is being rewritten from the ground up. The modules the rewrite keeps —
`memory`, `parse`, `scope`, `source`, `type_lattice`, `values`, `elaborate`,
`knot`, `scheduler`, `program`, `dispatch` and the embedded crates `cellgraph`
and `sexlex` — and the interpreter binary over them are
what a default koan build compiles and a default `cargo test` runs. `workgraph`
is no longer a koan dependency; it still builds and tests as a workspace member.
The old runtime — `machine`, `builtins`, the guard fixtures and every
`tests/*.rs` integration binary —
sits behind the `pending_rewrite` cargo feature, and **that build no longer
compiles**: `memory` is narrowed onto `cellgraph`, the old runtime names the
items it deleted, and the `pub use workgraph::scheduler` re-export
[src/lib.rs](src/lib.rs) keeps behind that feature collides with koan's own
`scheduler` module. It is re-implemented layer by layer rather than kept building;
[old_design/](old_design/) and
[observe/miri_slate_pending_rewrite.md](observe/miri_slate_pending_rewrite.md) are
its requirements record. `region-audit` and the two seam-force features are
knobs on the old runtime and turn it on; `alloc-count` and `dhat` instrument
the interpreter binary.

```sh
cargo test --workspace --features workgraph/test-hooks   # every kept module and embedded crate
```

`workgraph` is a workspace member but not a default one, so a bare `cargo test`
or `cargo clippy` skips it. Its doctests read a fixture compiled only under its
`test-hooks` feature, and no other crate turns it on, so a `--workspace` test or
clippy run names the feature; `tools/verify.sh` passes it on every workspace
step.

The verify slate (`tools/verify.sh`), the pre-commit hook and CI run the default
build, and the routine tier runs the [tutorial snippets](#tutorial-snippets)
through the interpreter. The tools that only mean something over the old
runtime — `tools/seam_equivalence.sh`, `tools/miri.py --pending-rewrite` and
the bounded regression test `tools/alloc_audit.py` runs — have nothing to run
until the rewrite re-points them. A test in a kept module that reaches the old runtime — the
form-table⟺registration law, the AST cache laws that ride a `WorkingExpression` —
is gated with it and comes back as the rewrite replaces what it reached.

## Unit tests

```sh
cargo test --workspace --features workgraph/test-hooks   # the kept modules' unit tests, across the workspace
cargo test parse::                                       # one module
cargo test -p sexlex                                     # the layout crate alone
cargo test -- --nocapture                                # show stdout
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the
code (parser, scheduler, dispatch, interpreter all have suites). After smoke-
testing a feature or bug fix, capture the smoke test as a unit test in the
nearest module's `tests` block.

CI runs the total tier on push and PR against `master` (see
[.github/workflows/rust.yml](.github/workflows/rust.yml)); the pre-commit hook runs the routine
tier.

### Parser property suites

The parser's laws are stated as [proptest](https://docs.rs/proptest) properties
rather than pinned one input at a time, at a quarter of the tier's depth — 64 cases under the
routine tier, 512 under the total one. A pin that fixes a
diagnostic message or a surface rule is *not* rewritten as a property and stays
beside its sibling unit tests. Seven files hold the twenty-nine properties:

- [`src/parse/tests/properties.rs`](src/parse/tests/properties.rs) — the ten
  end-to-end laws, over a generated expression tree, a tape of random bytes that
  chooses the layout it renders under (spacing, indented child lines, ignorable
  commas, redundant `(…)` wrappers), and an oracle that writes the same tree in
  the harness's `describe` notation: round-trip, the redundant-wrapper peel, token
  classification, spans, container arity, separator insensitivity, symbol minting,
  the operator-chain classification, type-sigil idempotence and compound-atom
  desugaring. Its keyword pool is deliberately disjoint from the keywords
  [`BUILTIN_SHAPES`](src/parse/builtin_shapes.rs) spells, so no generated run matches a builtin
  shape.
- [`src/parse/ast/tests.rs`](src/parse/ast/tests.rs) — node laws: the dispatch
  shape as a function of the key and the head class, the cache agreeing with a
  recompute and riding a copy and a resplice, a node key matching the signature key
  of the same pattern, the summary rendering, and structural equality.
- [`src/symbols/tests.rs`](src/symbols/tests.rs) — interning laws, beside
  the four fixed-name pins described under [Symbol mints](#symbol-mints).
- [`src/parse/builtin_shapes/tests/`](src/parse/builtin_shapes/tests.rs) — the builtin shape
  table, split three ways: static table-shape walks including the
  `BuiltinShapeId`-equals-index pin and the pin holding every slot's reading to a
  column written out by hand (`table.rs`), the caching and binder-plan laws
  (`binder.rs`), and the table⟺registration law pinning every `BUILTIN_SHAPES` key
  against the live builtin registration set, which is derived once, here
  (`registration.rs`). The two table laws — every slot typed once per overload,
  and each role's slot typed by its reading's code type — are build-time `const`
  assertions in `builtin_shapes.rs`, not tests.
- [`src/parse/builtin_shapes/layout/tests.rs`](src/parse/builtin_shapes/layout/tests.rs) —
  slot-layout laws: symbol order, the lexical position beside each entry, parameter merge.
- [`src/machine/model/close_inference/tests.rs`](src/machine/model/close_inference/tests.rs)
  — the capture-inference laws, per close rule.

### Scope property laws

The [scope](src/scope/README.md) laws are round trips at the tier's full depth, with nothing
filtered. [`src/scope/tests/plan.rs`](src/scope/tests/plan.rs) generates a *shape plan* from a
stream of choices. A plan says what each scope should come out as: its binders in the value and
type channels, how they partition into components, each read's class, and the binder each read
lands on. A plan also writes quote values, whose code holds holes, `$` and `\` names,
registrations and keyworded uses at planned keys, each use beside the candidates its mark says it
lists. The plan is rendered to koan source, which is parsed and built. Every plan is valid by
construction and names are unique across the program, so the expected shape is the plan itself
and nothing is re-derived from the source. `MODULE` and operator bodies are not generated, and
neither is `USING … SCOPE`: a `USING` block's parameters are *derived* — from the declaration its
operand names — so a plan could only state them by carrying modules end to end, which is what
nothing being re-derived from the source forbids. [`src/scope/tests/examples.rs`](src/scope/tests/examples.rs)
covers `MODULE` and every `USING` reading and refusal by example.
[`src/scope/tests/groups.rs`](src/scope/tests/groups.rs) covers the
[operator-group](src/scope/README.md#operator-groups) model before any rewrite —
what the builtin groups cover, what a `GROUP` body's member scan reads, and each
refusal the position-blind claims pre-scan makes — and
[`src/scope/tests/rewrite.rs`](src/scope/tests/rewrite.rs) covers the rewrite
itself, asserting on the shape a body owns: the four rewrites, where an operator
run is reached, equality and the `!=` negation, the groups a `USING` body and a
quote's code see, and each refusal. [`src/scope/tests/quotes.rs`](src/scope/tests/quotes.rs)
covers a quote value's code shape by example: how a hole, a `$` name and a `\` name each resolve,
its carried type, a malformed quote kept as a refusal, a refused code handing nothing to the next
body drafted, a mark no quote value holds, and the names an `EVAL` offers.
[`src/scope/tests/properties.rs`](src/scope/tests/properties.rs) holds four laws:

- a planned program shapes back into its plan: kinds, layouts, components and whether each is
  cyclic, one mention per planned read with its class, statement and landing, nested scopes,
  which captures are knot edges, and a unit order in which a unit runs after every unit it reads
  and every statement is in exactly one unit; and every name and keyworded use in a quote's code
  lands where its mark says — a hole, a binder of the code, where a read written at the quote lands
  through `$`, or offered through `\`;
- a plan with exactly one refusal injected (an eager cycle, an eager read ahead, an undeclared
  name, a rebind, a shadowed builtin) is refused with that refusal;
- a name re-declared inside a nested scope takes the reads nearest it;
- every activation of a planned program reads by name what its coordinates name, and closure
  bindings copy the enclosing words or hold knot edges. A quote's code is not activated, since
  only an `EVAL` runs one.

The knot suite renders the same plans with no quote value, since its law does not read how a
quote ties.

### Dispatch property laws

The [dispatch](src/dispatch/README.md#testing) laws draw types, values and whole programs from
[`src/dispatch/tests/generate.rs`](src/dispatch/tests/generate.rs), every program valid by
construction save a registration holding a union whose members tie, which the load refuses where
it is declared and the narrowing law discards. The narrowing law runs each program as loaded and
again under the test-only `unnarrowed` switch in [`statics.rs`](src/dispatch/statics.rs), which
leaves every keyworded use whole and refuses none: a program the load accepts runs as it does
unnarrowed, and a use the load refuses faults when run unnarrowed.
The spelling laws ([`spellings.rs`](src/dispatch/tests/spellings.rs)) call one registration over
one class by keyword, by name, and by name with its record's fields reversed: each spelling loads,
faults or prints alike.
The lexical-variable law runs a site naming a quantified variable by a type with and without the
contexts drawn around it, the bare program its oracle.

## Tutorial snippets

Every runnable code block in [`tutorial/`](tutorial/README.md) is checked against
the interpreter by [`tools/verify_snippets.py`](tools/verify_snippets.py): it runs
each `koan` block that is immediately followed by a `text` expected-output block
and diffs the result, through the interpreter binary, which the routine tier
builds first. A snippet using an expression shape on the script's `PENDING`
list — one the rewrite does not run yet — is skipped and counted apart; the
roadmap items that ship those shapes shrink the list.

## Linting and formatting

```sh
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run these locally before pushing. Clippy is configured per-crate in
[Cargo.toml](Cargo.toml); per-site `#[allow(...)]` is fine when the lint is
wrong (e.g., the `clippy::large_enum_variant` allows on
[`NodeScope`](src/machine/execute/nodes.rs), [`Outcome`](src/machine/execute/outcome.rs)
and [`ScopeKind`](src/machine/core/scope.rs), where the wide arm is the common one and
boxing it would cost an allocation on the hot path).

## Modgraph complexity baseline

The verify skill records the koan crate's modgraph fractal-complexity score
to [`observe/complexity.txt`](observe/complexity.txt) on every run, newest
first, capped to five entries. A refactor should either reduce the score
by more than rounding noise, reduce code duplication, or enforce some
invariant using the type system.

It shares its layout with [`observe/coverage.txt`](observe/coverage.txt) and
[`observe/alloc.txt`](observe/alloc.txt): all three are rendered by
`tools/trendlog.py`, which puts each column's name directly over its own column
rather than listing the names in a header a reader has to count against the row.

`tools/modgraph regen --baseline observe/complexity.txt` manages the file end-to-end:
it prunes entries whose commit isn't reachable from HEAD (covers `git
checkout`, `git reset --hard`, rebase drops) and every prior dirty-snapshot
(`+`-suffixed) entry, then prepends today's measurement and prints a one-
line delta against the prior top entry.

Captured at `--root koan` with default `α=2, β=5, γ=10, T=400`. Scoring
details and tuning lives in
[.claude/skills/modgraph/SKILL.md](.claude/skills/modgraph/SKILL.md).

## Miri audit slate

The audit slate is the load-bearing memory-safety check. It runs the safe
koan code that drives every unsafe site the kept modules reach — `cellgraph`'s
`Writer::fill`, `Writer::thin_run` and reattach seam, and `bumpalo`'s allocator
under the bump tier — under Miri's tree-borrows mode, with zero process-exit
leaks and zero UB required for sign-off. `memory`'s knot adds no layout or
retype over `thin_run` (cellgraph's slate runs it at every edge its arithmetic
has, including a fill writing into the same region); what koan's slate pins is
`knot`'s copy of a knot, whose node run is filled while closure runs and data
node residents are written into the same region. `src/` carries no `unsafe` at all — koan's only
`unsafe` is the counting global allocator in
[`audit/counting_alloc.rs`](audit/counting_alloc.rs), measurement scaffolding
outside the tree the slate audit censuses (`tools/observe_tests.py` walks `src/`
only). It is still exercised under Miri: [`src/tests.rs`](src/tests.rs) installs
it as the lib-test binary's global allocator, so every slate test allocates
through it. The slate covers the safe koan code that drives the substrate's
retypes, and
each embedded crate's own slate covers that library in isolation:
[workgraph/observe/miri_slate.md](workgraph/observe/miri_slate.md) and
[cellgraph/observe/miri_slate.md](cellgraph/observe/miri_slate.md).

The model the slate signs off on is documented in
[old_design/memory-model.md](old_design/memory-model.md#verification).

### Command of record

```sh
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- <test-names>
```

Two slates. [`observe/miri_slate.md`](observe/miri_slate.md) covers the modules
the rewrite keeps and runs on the default build; `python3 tools/miri.py` drives
it. [`observe/miri_slate_pending_rewrite.md`](observe/miri_slate_pending_rewrite.md)
is the old runtime's slate, frozen with the code it audits: `python3 tools/miri.py
--pending-rewrite` selects it and adds `--features pending_rewrite`.

The first run under a fresh Miri target dir takes several minutes to compile;
subsequent runs are 1–3 min per test. Triage workflow (per-test re-runs,
pinned-id allocation tracking) lives in
[.claude/skills/miri/SKILL.md](.claude/skills/miri/SKILL.md).

Read the **whole** output — never `tail` it. The slate tests live in the lib
unit-test binary, which runs *first*; the trailing `tests/*.rs` binaries match
none of the slate filter and each report `0 passed; N filtered out`, so the tail
of a run looks identical to "Miri ran nothing." Confirm the lib `test result:`
line shows `passed` ≈ the slate size (`python3 tools/observe_tests.py slate | wc -w`)
before trusting a clean result — exit code 0 alone is not sufficient, since
`cargo test` exits 0 when zero tests run.

### The slate

The canonical slate — test names grouped by the substrate discipline each pins
down, the policy for adding tests, and the runtime baseline (five most-recent
full-slate runs) all live in [`observe/miri_slate.md`](observe/miri_slate.md);
the old runtime's keeps the same shape in
[`observe/miri_slate_pending_rewrite.md`](observe/miri_slate_pending_rewrite.md).

## Region debug audits

Two diagnostics of the old runtime report **over-pinning** — a region kept
alive longer than the values reaching it need, which every other check passes
silently because it breaks no invariant. The interpreter binary is the
rewrite's and reports neither, so the commands below wait on the old runtime
building again. Both are compiled out of a release build, and both only
record: neither panics, and neither changes what is retained
([old_design/memory-model.md § Debug region audits](old_design/memory-model.md#debug-region-audits)).

```sh
cargo run --features pending_rewrite -- program.koan   # debug build: pin rings reported
cargo run --features region-audit -- program.koan      # also reports over-folds
```

Findings print to stderr after the run. The pin-ring detector needs no feature
beyond the old runtime — any debug build of it has it — while the
reach-tightness report is `region-audit`'s.
Silence means the run detected nothing, which is the expected result; a report is
a real finding worth chasing.

## Allocation counts

The counting global allocator ([`audit/`](audit/README.md)) reports allocation
traffic two ways.

```sh
python3 tools/alloc_audit.py                            # sweep every shape and report
python3 tools/alloc_audit.py --baseline                 # sweep and record
cargo run --features alloc-count -- program.koan        # one program's totals, on stderr
cargo test --features pending_rewrite --test allocation_baseline   # the bounded regression test
```

The sweep is the recorder. It runs every shape through the counted binary and writes
the readings to [`observe/alloc.txt`](observe/alloc.txt) — one row per commit swept,
newest first, capped to the last five, with the marginal terms derived from each row
on read rather than stored beside it.
The sweep reads the interpreter binary, so it
runs on demand rather than in either verification tier: run it with `--baseline` in the
commit that moves a reading, and the record and the change that moved it land together —
nothing has to be re-measured against a base revision to be trusted. What a figure *means*
is in [audit/README.md](audit/README.md), which quotes none of them.

The regression test is the gate. It brackets each recorded shape and asserts its count
against a bound whose headroom is smaller than the repetition count a single new allocation
on the scaling path would cost, so one added allocation fails it. The sweep prints every
bound's headroom beside the reading it is set over and flags one that has drifted. A failure
means either an allocation was added to the execute path or an unrelated fixed cost moved;
re-measure with `tools/alloc_audit.py` before rebaselining.

`workgraph` counts on its own, with its own copy of the scaffolding
([`workgraph/src/tests.rs`](workgraph/src/tests.rs), a delegating counter over
`System` installed for that crate's lib-test binary and tallying per thread, so
a bracket is not polluted by the tests running beside it). It carries the
scheduler's steady-state claim — a slot that parks and wakes on a fixed shape
allocates nothing per wake once its rows have grown — as an *exact* equality
rather than a bound
([`scheduler/tests/recycling.rs`](workgraph/src/scheduler/tests/recycling.rs)),
which is why a reintroduced per-wake allocation surfaces there as a `+1` before
it reaches koan's bounds at all. The one allocation the window still admits is
the install door's debug-only acyclicity check, named at the constant that
expects it.

## Symbol mints

The `alloc-count` feature carries a second reading beside the allocation tally: the
process's `Symbol` mint count, printed as `symbols_minted: N` and recorded beside the
allocation figure in `observe/alloc.txt`. Hashing takes no allocation, so
this is the only instrument that sees a mint leave a per-call path
([audit/README.md § Symbol mints](audit/README.md#symbol-mints)).

The figure is **recorded, not bounded**. It has no entry in
`tests/allocation_baseline.rs`: the counter is a lib-side `cfg` an integration test
cannot reach. A figure that moves shows up as a delta in the sweep's report, the same way
the allocation column's absolute rows do.

The names fixed in Rust source — builtin parameter slots and the `Result` / `KError`
tags ([old_design/label-interning.md § Names fixed in Rust source](old_design/label-interning.md#names-fixed-in-rust-source))
— are pinned by four unit tests in
[`src/symbols/tests.rs`](src/symbols/tests.rs), over a static and a slot group of
that test module's own so they pin the mechanism rather than whatever spelling a builtin
happens to declare. Two cover a lone declaration: `a_static_name_mints_what_of_mints` (the
memo is exactly what the class's `of` would mint, and `text()` is the spelling as written)
and `record_interns_the_spelling_under_the_memoized_symbol` (`record` hands back the
memoized symbol, interns the spelling under it, and a second call adds nothing). Two more
cover a group, which is where grouping could quietly merge what it only means to co-locate:
`a_slot_group_declares_each_field_independently` (each field carries its own spelling and
its own symbol, and two fields do not collide) and
`record_interns_each_grouped_slot_separately` (recording a group of two leaves two interner
entries, both resolvable).

There is deliberately no exhaustive "every declared static classifies" test, because
every declaration is already forced by every test that runs a program: each slot in a
builtin's group reaches `arg` at that builtin's registration and each tag static reaches the registration
that builds its type, so building a prelude forces the whole set of memos. A spelling
that will not classify panics in *every* such test rather than only in the one
exercising its builtin.
