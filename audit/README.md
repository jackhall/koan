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
| `counting_alloc.rs` | a `GlobalAlloc` that tallies allocations and bytes and delegates to an inner one | `src/tests.rs`, `cellgraph/perf/main.rs` |
| `seek_cost.rs` | the per-call cost of a seek read off a request, one keyed from the values, and a miss | `src/dispatch/tests.rs` |
| `parametric_layers.rs` | what a resumption re-runs, decides and lays per hop, walked closed and witnessed | `src/dispatch/tests.rs` |

A Rust file here is `#[path]`-included by the module that declares it, so it resolves
`crate::…` and `super::…` exactly as a file under `src/` does.

The counting allocator has a second reason to be here: `src/` carries no `unsafe` at all,
and a counting `#[global_allocator]` is an `unsafe impl` that under `src/` would register
with `tools/observe_tests.py slate-audit` as a production site owing a Miri slate group.
Miri still exercises it — `src/tests.rs` installs it as the lib-test binary's global
allocator, so every slate test allocates through it.

## The counter

`counting_alloc.rs` wraps an allocator rather than replacing one, so it counts whatever
allocator its target would otherwise run on. Its tallies are thread-locals, since a test
brackets one call with them and the test harness runs tests concurrently: a shared counter would
fold every other test's traffic into the bracket. A count sits beside a byte total — the count
says how many times the program asked, the bytes how much it asked for, which is what separates a
growing buffer from a new one — and a live balance gives what the thread still holds.

Two targets `#[path]`-include the one file:

- `src/tests.rs`, for the library's unit-test binary. `crate::tests::allocation_count()`
  reads the thread-local tally, which the type lattice's heap contract and the scheduler's
  tail tests bracket. It needs no feature flag, which is what keeps `counting_alloc.rs` in the
  default verify slate.
- `cellgraph/perf/main.rs`, the cell substrate's per-verb measurement harness, under that
  crate's `perf` feature. It brackets each door call with the counts, bytes and live balance
  ([cellgraph/README.md § Measuring](../cellgraph/README.md#measuring)).

## Parametric layers

`parametric_layers.rs` runs four tail loops at depths 4 and 16 and prints the per-hop
difference, with every resumption walked closed (before) and witnessed (after):

```sh
cargo test --lib -p koan -- --ignored --nocapture parametric_layers
```

| loop | column | units re-run per hop in closing walks, by open mark | walked | kept | laid | closed types | region bytes | bytes |
|---|---|---|---|---|---|---|---|---|
| one type per hop | before | none | 0 | 0 | 0 | 0 | 2580 | 22494 |
| | after | none | 0 | 0 | 0 | 0 | 2580 | 22630 |
| a novel type per hop | before | Verdict 2, Retype 3, Unequal 2, Unclosed 1, Kind 1, ModuleBinder 3 | 1 | 1 | 1 | 1 | 8498 | 136967 |
| | after | Verdict 1, Retype 1, Unequal 1, Unclosed 1, ModuleBinder 1 | 1 | 1 | 1 | 2 | 6930 | 137226 |
| nothing held | before | Verdict 2, Retype 3, Unequal 1, Kind 1, ModuleBinder 3 | 1 | 1 | 1 | 0 | 7272 | 87741 |
| | after | none | 0 | 1 | 1 | 2 | 3807 | 43984 |
| a deepening entry under a statement | before | Verdict 2, Retype 3, Unequal 1, Kind 1, ModuleBinder 3, ModuleContent 2 | 2 | 2 | 2 | 0 | 35728 | 488878 |
| | after | none | 0 | 2 | 2 | 4 | 28073 | 355694 |

No loop witnesses past its first hops, so no witnessing walk re-runs a unit and no question is
recorded per hop. A unit is counted once per kind of open mark it reaches. A loop whose hops bind
novel types that keep every question walks nothing past its first hops: each hop lays its binding
layer as a table of closed types alone. Where a unit holds (the novel loop's list over `x`), each
hop still walks, re-running fewer units. A memo hit reads the witnessed layer in place where the
memo keeps it over the frame's layers beneath, and otherwise lays only its header over them, so a
hit lays one layer in the frame's region, as walked closed; a frame whose event witnesses lays
both its witnessed and its binding layer there.

## The attribution profiler

The `dhat` cargo feature names the sites a program's allocations are made at. It installs
[dhat-rs](https://docs.rs/dhat)'s allocator as the binary's global allocator, which records an
untrimmed backtrace per allocation and writes `dhat-heap.json` after the run. Run one program at
two sizes and difference the block counts per site with `tools/dhat_diff.py` — a site whose count
scales with the size difference is on the per-unit path, and everything constant (startup,
seeding) cancels:

```sh
cargo run --features dhat -- small.koan && mv dhat-heap.json small.json
cargo run --features dhat -- big.koan && mv dhat-heap.json big.json
python3 tools/dhat_diff.py small.json big.json 90                      # site table, per unit
python3 tools/dhat_diff.py small.json big.json 90 --detail 'run'       # full stacks
```

The dhat allocator owns the global slot (it delegates to the system allocator) and pays a
backtrace per allocation, so a profiled run is neither count- nor time-comparable with an
unprofiled one.
