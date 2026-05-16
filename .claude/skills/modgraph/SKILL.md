---
name: modgraph
description: Use this skill when scoring module-structure changes in the koan repo against the live dep graph — measuring whether a proposed sub-module reshuffle, a rename, or a file split actually reduces complexity before committing to it. Pairs `cargo modules` (DOT export), `tools/modgraph.py` (recursive fractal scoring with structural + per-file size terms), and `tools/modgraph_rewrite.py` (apply renames to DOT + mirror `src/` for what-if scoring).
---

# modgraph

Score module-structure changes before doing them. `tools/modgraph.py` always does the same thing: take a DOT graph, recursively walk a module subtree, and report a complexity index combining three terms:

- **cross edges** between groups at each level (lower is better),
- **feedback** = edges that go against the best topological order (`α`-weighted),
- **per-non-leaf charge** `β` (penalises wrapper layers) and **per-file size charge** `γ · L · log(1 + L/T)` (penalises fat files).

Defaults (`α=2, β=5, γ=10, T=400`) are calibrated on the koan tree — see `tools/modgraph.py` docstring for the rationale.

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

Per-module breakdown plus a single bottom-line number: the **per root-loc** score, split into `structure` (coupling/nesting) and `size` (per-file size charge over every module's own file — including fat `mod.rs` files above small children). Absolute totals are intentionally not reported; only the per-loc number is calibrated to compare across runs and tree shapes.

Each row prints `own N size N.N` so the biggest offenders are visible in-line. Tune the size term with `--gamma` / `--size-pivot`; tune structure with `--alpha` / `--beta`.

For tracked baselines (used by the `verify` skill), pass `--baseline <file>` — modgraph prunes stale entries (unreachable SHAs, prior dirty snapshots), prepends today's measurement, trims to 5, and prints a delta line. For ad-hoc what-if scoring (refactor exploration via `modgraph_rewrite.py`), leave the flag off so the baseline file isn't touched.

### 3. Score a *refactor* (renames, not moves)

`tools/modgraph_rewrite.py` applies `OLD=NEW` renames to the DOT graph and mirrors `src/` to a parallel tree, so you can re-run modgraph against a hypothetical layout without touching real files:

```sh
python3 tools/modgraph_rewrite.py \
    --edges /tmp/koan.dot \
    --output-edges /tmp/koan_proposed.dot \
    --output-src /tmp/koan_proposed_src \
    --rename koan::parse::kexpression=koan::ast \
    --rename koan::execute=koan::dispatch::execute

python3 tools/modgraph.py --edges /tmp/koan_proposed.dot --root koan \
                          --src-root /tmp/koan_proposed_src
```

Each `--rename OLD=NEW` rebinds module paths matching `OLD` or starting with `OLD::`; both DOT tokens and the mirrored `src/` filenames update. Renames apply against the original path only — chains (`A=B`, `B=C`) must be expressed as the final target. For long lists, use `--rename-file <path>` (one `OLD=NEW` per line, `#` for comments).

The score before/after gap is what to optimize: a rename is worth doing only if `--root koan` drops by more than rounding noise.

## Pitfalls

- **Symbol-level DOT silently zeroes the score.** The walk maps modules to files; type/function nodes have no `.rs` backing. Always export with `--no-fns --no-types --no-traits`.
- **Score deltas, not absolutes.** The defaults are calibrated, but the absolute number depends on tree shape. Always compare current vs. proposed under the same flags.
- **The size term is intentionally orthogonal.** Splitting a 2000-line leaf into two 1000-line leaves drops the size charge from ~7168 to ~2506, but it adds one wrapper layer at structural cost `β · loc(subtree)` — the split is worth it only when the size drop exceeds the structural add.
