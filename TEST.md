# Testing and linting Koan

Four layers, each with a distinct job:

1. **`cargo test`** — every unit test in the crate, run on every push and PR.
2. **`cargo clippy` / `cargo fmt`** — lints and formatting.
3. **The Miri audit slate** — targeted memory-safety coverage for every unsafe
   site in the runtime, run under tree borrows.
4. **The region debug audits** — debug-only over-pinning diagnostics over a real
   program run, reported and never enforced.

## Unit tests

```sh
cargo test                  # all unit tests
cargo test parse::          # one module
cargo test -- --nocapture   # show stdout
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the
code (parser, scheduler, dispatch, interpreter all have suites). After smoke-
testing a feature or bug fix, capture the smoke test as a unit test in the
nearest module's `tests` block.

CI runs `cargo build --verbose && cargo test --verbose` on push and PR against
`master` (see [.github/workflows/rust.yml](.github/workflows/rust.yml)).

## Tutorial snippets

Every runnable code block in [`tutorial/`](tutorial/README.md) is checked against
the interpreter by [`tools/verify_snippets.py`](tools/verify_snippets.py): it runs
each `koan` block that is immediately followed by a `text` expected-output block
and diffs the result. This runs as a step in the verify slate (`tools/verify.sh`),
so tutorial drift fails the same gate as tests and lints. Run it standalone after
editing the tutorial:

```sh
cargo build && python3 tools/verify_snippets.py
```

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

`tools/modgraph regen --baseline observe/complexity.txt` manages the file end-to-end:
it prunes entries whose commit isn't reachable from HEAD (covers `git
checkout`, `git reset --hard`, rebase drops) and every prior dirty-snapshot
(`+`-suffixed) entry, then prepends today's measurement and prints a one-
line delta against the prior top entry.

Captured at `--root koan` with default `α=2, β=5, γ=10, T=400`. Scoring
details and tuning lives in
[.claude/skills/modgraph/SKILL.md](.claude/skills/modgraph/SKILL.md).

## Miri audit slate

The audit slate is the load-bearing memory-safety check. It runs every unsafe
site the runtime reaches — the lifetime-erasure transmutes in `workgraph`'s
witnessed substrate, the safe-code disciplines routing them (brand-confined
construction doors, interior mutation under live shared borrows, region drop
order), and the cycle gate that prevents self-referential `Rc<FrameStorage>`
storage — under Miri's tree-borrows mode, with zero process-exit leaks and zero
UB required for sign-off. `src/` carries no `unsafe` at all — koan's only
`unsafe` is the counting global allocator in
[`audit/counting_alloc.rs`](audit/counting_alloc.rs), measurement scaffolding
outside the tree the slate audit censuses (`tools/observe_tests.py` walks `src/`
only). It is still exercised under Miri: [`src/tests.rs`](src/tests.rs) installs
it as the lib-test binary's global allocator, so every slate test allocates
through it. The slate covers the safe koan code that drives the substrate's
retypes, and
`workgraph`'s own slate ([workgraph/observe/miri_slate.md](workgraph/observe/miri_slate.md))
covers the library in isolation.

The model the slate signs off on is documented in
[design/memory-model.md](design/memory-model.md#verification).

### Command of record

```sh
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- <test-names>
```

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

The canonical slate — test names grouped by the unsafe site each pins down,
the policy for adding tests, and the runtime baseline (five most-recent full-
slate runs) all live in [`observe/miri_slate.md`](observe/miri_slate.md).

## Region debug audits

Two diagnostics report **over-pinning** — a region kept alive longer than the
values reaching it need, which every other check passes silently because it
breaks no invariant. Both are compiled out of a release build, and both only
record: neither panics, and neither changes what is retained
([design/memory-model.md § Debug region audits](design/memory-model.md#debug-region-audits)).

```sh
cargo run -- program.koan                       # debug build: pin rings reported
cargo run --features region-audit -- program.koan   # also reports over-folds
```

Findings print to stderr after the run. The pin-ring detector needs no feature —
any debug build has it — while the reach-tightness report is `region-audit`'s.
Silence means the run detected nothing, which is the expected result; a report is
a real finding worth chasing.

## Allocation counts

The counting global allocator ([`audit/`](audit/README.md)) reports allocation
traffic two ways.

```sh
bash audit/measure.sh                                   # whole-program totals per shape
cargo run --features alloc-count -- program.koan        # one program's totals, on stderr
cargo test --test allocation_baseline                   # the bounded regression test
```

`audit/measure.sh` reproduces the committed baseline table in
[audit/README.md](audit/README.md); the table is transcribed by hand, so a
figure that moves is a deliberate edit. The regression test brackets each
recorded shape and asserts its count against a bound tight enough to see one
added allocation — 41 allocations of headroom against the 100 (loop) or 127
(chain) a single new one on the scaling path costs. A failure means either an
allocation was added to the execute path or an unrelated fixed cost moved;
re-measure with `audit/measure.sh` before rebaselining, and rebaseline the table
and the bound together.

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
process's `Symbol` mint count, printed as `symbols_minted: N` and captured by
`audit/measure.sh` as the third column of its report. Hashing takes no allocation, so
this is the only instrument that sees a mint leave a per-call path
([audit/README.md § Symbol mints](audit/README.md#symbol-mints)).

The figure is **recorded, not bounded**. It has no entry in
`tests/allocation_baseline.rs`: the counter is a lib-side `cfg` an integration test
cannot reach. A figure that moves is caught by re-measuring and rebaselining the table,
the same way the allocation column's absolute rows are.

The names fixed in Rust source — builtin parameter slots and the `Result` / `KError`
tags ([design/label-interning.md § Names fixed in Rust source](design/label-interning.md#names-fixed-in-rust-source))
— are pinned by four unit tests in
[`labels/tests.rs`](src/machine/model/labels/tests.rs), over a static and a slot group of
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
