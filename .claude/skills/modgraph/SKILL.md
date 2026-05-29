---
name: modgraph
description: Use this skill when scoring module-structure changes in the koan repo against the live dep graph — measuring whether a proposed sub-module reshuffle, a rename, a file split, or an item extraction actually reduces complexity before committing to it. Pairs `cargo modules` (DOT export), `tools/modgraph.py` (recursive fractal scoring with structural + per-file size terms), and `tools/modgraph_rewrite.py` (`module` subcommand for whole-module renames, `item` subcommand for SCIP-driven item-level extractions; both produce a DOT + mirrored `src/` for what-if scoring).
---

# modgraph

Score module-structure changes before doing them. `tools/modgraph.py` always does the same thing: take a DOT graph, recursively walk a module subtree, and report a complexity index combining three terms:

- **cross edges** between groups at each level (lower is better),
- **feedback** = edges that go against the best topological order (`α`-weighted),
- **per-non-leaf charge** `β` (penalises wrapper layers) and **per-file size charge** `γ · L · log(1 + L/T)` (penalises fat files).

Defaults are calibrated on the koan tree — see `tools/modgraph.py` docstring for the defaults and rationale.

Assume `cargo modules` is on PATH.

## Recipes

### 1. Export the dep graph

```sh
cargo modules dependencies --package koan --lib \
    --no-externs --no-sysroot --no-traits --no-fns --no-types \
    > /tmp/koan.dot
```

The `--no-fns --no-types --no-traits` flags are required: the walk maps modules to files, and symbol-level nodes have no `.rs` backing, which would silently zero out LOC.

### 2. Score a subtree (or the whole crate)

```sh
python3 tools/modgraph.py --edges /tmp/koan.dot --root koan
python3 tools/modgraph.py --edges /tmp/koan.dot --root koan::runtime::machine
```

Per-module breakdown plus a single bottom-line number: the **per root-loc** score, split into `coupling` (cross/feedback edges at each wrapper, deduplicated per `(source_group, dst_module)` so splitting a file doesn't multiply its shared imports), `nesting` (β·loc charge per wrapper layer), and `size` (per-file charge over every module's own file using **raw LOC** — tests, doc comments, and inline comments all count toward the size penalty, even though structural terms ignore them). Absolute totals are intentionally not reported; only the per-loc number is calibrated to compare across runs and tree shapes.

Each row prints `nest N.N` next to `index N.N` (the rolled-up structural cost) and `own N (raw N) size N.N`, exposing both the production-LOC measure used for coupling/nesting weighting and the raw-LOC measure used for the size penalty. Tune size with `--gamma` / `--size-pivot`; tune coupling/nesting with `--alpha` / `--beta` / `--beta-children-pivot`.

For tracked baselines (used by the `verify` skill), pass `--baseline <file>` — modgraph prunes unreachable-SHA entries (branch checkout / hard reset / rebase drop), prepends today's measurement, trims to 5, and prints a delta line. Dirty-snapshot `+` entries are retained so a pre-commit hook (which always sees a staged-but-not-yet-committed tree) doesn't erase the trend log. For ad-hoc what-if scoring (refactor exploration via `modgraph_rewrite.py`), leave the flag off so the baseline file isn't touched.

### 3. Score a *refactor* (module renames)

`tools/modgraph_rewrite.py module` rewrites the DOT graph and mirrors `src/` so you can re-run modgraph against a hypothetical layout without touching real files:

```sh
python3 tools/modgraph_rewrite.py module \
    --edges /tmp/koan.dot \
    --output-edges /tmp/koan_proposed.dot \
    --output-src /tmp/koan_proposed_src \
    --rename koan::parse::kexpression=koan::ast \
    --rename koan::execute=koan::dispatch::execute

python3 tools/modgraph.py --edges /tmp/koan_proposed.dot --root koan \
                          --src-root /tmp/koan_proposed_src
```

Each `--rename OLD=NEW` rebinds module paths matching `OLD` or starting with `OLD::`; both DOT tokens and the mirrored `src/` filenames update. Renames apply against the original path only — chains (`A=B`, `B=C`) must be expressed as the final target. For long lists, use `--rename-file <path>` (one `OLD=NEW` per line, `#` for comments).

### 4. Score a *refactor* (item extraction)

When the seam is "pull these specific items out into a new module" (not a whole-module rename), use `tools/modgraph_rewrite.py item`. It reads a SCIP code-index from `rust-analyzer`, resolves item references at function/method granularity, and surgically rewrites the DOT plus mirrored `src/`:

```sh
rust-analyzer scip . --output /tmp/koan.scip
python3 tools/modgraph_rewrite.py item \
    --scip /tmp/koan.scip --edges /tmp/koan.dot --src-root src \
    --output-edges /tmp/koan_proposed.dot \
    --output-src /tmp/koan_proposed_src \
    --move koan::machine::core::scope::Scope::register_nominal=koan::machine::core::nominal
```

Item paths follow Rust-canonical spelling (`Module::Struct::method`). Requires `rust-analyzer` on PATH (`rustup component add rust-analyzer` or rely on `rust-toolchain.toml`).

The score before/after gap is what to optimize under either mode: a rewrite is worth doing only if `--root koan` drops by more than rounding noise.

## Pitfalls

- **Symbol-level DOT silently zeroes the score.** The walk maps modules to files; type/function nodes have no `.rs` backing. Always export with `--no-fns --no-types --no-traits`.
- **Score deltas, not absolutes.** The defaults are calibrated, but the absolute number depends on tree shape. Always compare current vs. proposed under the same flags.
- **The size term is intentionally orthogonal.** Splitting a 2000-line leaf into two 1000-line leaves drops the size charge from ~7168 to ~2506, but it adds one wrapper layer at structural cost `β · loc(subtree)` — the split is worth it only when the size drop exceeds the structural add.
