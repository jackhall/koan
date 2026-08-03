# Testing and linting Koan

Three layers, each with a distinct job:

1. **`cargo test`** — every unit test in the crate, run on every push and PR.
2. **`cargo clippy` / `cargo fmt`** — lints and formatting.
3. **The Miri audit slate** — targeted memory-safety coverage for every unsafe
   site in the runtime, run under tree borrows.

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
UB required for sign-off. `src/` itself carries no `unsafe`, tests included; the
slate covers the safe koan code that drives the substrate's retypes, and
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
