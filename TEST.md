# Testing and linting Koan

Three layers, each with a distinct job:

1. **`cargo test`** — every unit test in the workspace, run on every push and PR.
2. **`cargo clippy` / `cargo fmt`** — lints and formatting.
3. **The Miri audit slate** — targeted memory-safety coverage for every unsafe
   site in the runtime, run under tree borrows.

## Two verification tiers

[`tools/verify.sh`](tools/verify.sh) runs one of two tiers, and the tier is the argument:

```sh
tools/verify.sh            # routine — what the pre-commit hook runs
tools/verify.sh --total    # total — what CI runs, and what you run before a merge
```

The **routine** tier answers "did this change break anything": the unsafe-site drift check, tests
and doctests, the cellgraph surface checks, clippy, doc links. It measures nothing, so it costs
seconds. It also reads the changed paths and narrows itself — a Markdown-only change runs the link
audit alone, and a change confined to `cellgraph/` runs that crate's slate and reports whether koan
still compiles rather than gating on it.

The **total** tier adds everything that costs minutes: coverage instrumentation, the
[Miri audit slate](#miri-audit-slate), the modgraph complexity score, and a property sweep 32×
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

## Unit tests

```sh
cargo test --workspace         # every module's unit tests, across the workspace
cargo test parse::             # one module
cargo test -p sexlex           # the layout crate alone
cargo test -- --nocapture      # show stdout
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the
code. After smoke-
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
beside its sibling unit tests. Five files hold the twenty-one properties:

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
  shape as a function of the key and the head class, and the summary rendering.
- [`src/symbols/tests.rs`](src/symbols/tests.rs) — interning laws, including
  how a [name fixed in Rust source](src/symbols/README.md#names-fixed-in-rust-source)
  records.
- [`src/parse/builtin_shapes/tests/`](src/parse/builtin_shapes/tests.rs) — the builtin shape
  table, split two ways: static table-shape walks including the
  `BuiltinShapeId`-equals-index pin and the pin holding every slot's reading to a
  column written out by hand (`table.rs`), and the caching and binder-plan laws
  (`binder.rs`). The two table laws — every slot typed once per overload,
  and each role's slot typed by its reading's code type — are build-time `const`
  assertions in `builtin_shapes.rs`, not tests.
- [`src/parse/builtin_shapes/layout/tests.rs`](src/parse/builtin_shapes/layout/tests.rs) —
  slot-layout laws: symbol order, the lexical position beside each entry, and redundant
  wrappers.

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
list — one the interpreter does not run yet — is skipped and counted apart; the
roadmap items that ship those shapes shrink the list.

## Linting and formatting

```sh
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run these locally before pushing. Clippy is configured per-crate in
[Cargo.toml](Cargo.toml); per-site `#[allow(...)]` is fine when the lint is
wrong (e.g., the `clippy::large_enum_variant` allow on `Pairing` in
[`walk/binary.rs`](src/type_lattice/walk/binary.rs), whose structural arm is the
common one, where boxing it would put an allocation on the hot path of every
relation in the lattice).

## Modgraph complexity baseline

The verify skill records the koan crate's modgraph fractal-complexity score
to [`observe/complexity.txt`](observe/complexity.txt) on every run, newest
first, capped to five entries. A refactor should either reduce the score
by more than rounding noise, reduce code duplication, or enforce some
invariant using the type system.

It shares its layout with [`observe/coverage.txt`](observe/coverage.txt): both are
rendered by
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
cellgraph's own slate covers that library in isolation:
[cellgraph/observe/miri_slate.md](cellgraph/observe/miri_slate.md).

### Command of record

```sh
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- <test-names>
```

[`observe/miri_slate.md`](observe/miri_slate.md) is the slate, and
`python3 tools/miri.py` drives it.

The first run under a fresh Miri target dir takes several minutes to compile;
subsequent runs are 1–3 min per test. Triage workflow (per-test re-runs,
pinned-id allocation tracking) lives in
[.claude/skills/miri/SKILL.md](.claude/skills/miri/SKILL.md).

Read the **whole** output — never `tail` it. The slate tests live in the lib
unit-test binary; any other test binary matches none of the slate filter and
reports `0 passed; N filtered out`, which reads identically to "Miri ran
nothing." Confirm the lib `test result:`
line shows `passed` ≈ the slate size (`python3 tools/observe_tests.py slate | wc -w`)
before trusting a clean result — exit code 0 alone is not sufficient, since
`cargo test` exits 0 when zero tests run.

### The slate

The canonical slate — test names grouped by the substrate discipline each pins
down, the policy for adding tests, and the runtime baseline (five most-recent
full-slate runs) all live in [`observe/miri_slate.md`](observe/miri_slate.md).
