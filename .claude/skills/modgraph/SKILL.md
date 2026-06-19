---
name: modgraph
description: Use this skill when scoring module-structure changes in the koan repo against the live dep graph — measuring whether a proposed sub-module reshuffle, a rename, a file split, or an item extraction actually reduces complexity before committing to it. The `tools/modgraph` package exposes three verbs — `regen` (refresh `observe/modules.dot` via `cargo modules` + re-export correction and `observe/doc_graph.dot` via `tools/doclinks.py signals`, then score), `score` (score a DOT subtree with structural + per-file size terms), and `rewrite` (`module` for whole-module renames, `item` for SCIP-driven item-level extractions; both produce a DOT + mirrored `src/` for what-if scoring).
---

# modgraph

Score module-structure changes before doing them. `python3 tools/modgraph <verb>` (a directory-as-script package) takes a DOT graph, recursively walks a module subtree, and reports a complexity index combining:

- **cross edges** between groups at each level (lower is better),
- **feedback** = edges that go against the best topological order (`α`-weighted),
- **fan-out** = distinct external entry points per dst_group past the first (`λ`-weighted, provider-side facade reward — a group with one external entry pays nothing),
- **per-non-leaf charge** `β` (penalises wrapper layers) and **per-file size charge** `γ · L · log(1 + L/T)` (penalises fat files).

Defaults are calibrated on the koan tree — see the package docstring (`tools/modgraph/__main__.py` / `score.py` / `loc.py`) for the rationale.

**Re-export correction (default).** `cargo modules` resolves every `use` edge to the item's *definition* module, discarding `pub use` facades — so a consumer writing `use crate::machine::Scope` is charged a deep edge to `core::scope` even though moving `scope` anywhere under machine wouldn't break it. `regen` corrects this: it re-attributes each `uses` edge to the module path the author actually wrote (`reexport.py`). The corrected graph is the canonical `observe/modules.dot`; it is what every score reflects. This is core graph construction, not an option (`regen --no-reexport` exists for raw-vs-corrected diagnostics only).

## Recipes

### 1. Score against the tracked baseline (regenerate + score in one shot)

```sh
python3 tools/modgraph regen --root koan --baseline observe/complexity.txt
```

`regen` rebuilds the two source-data files before scoring: `observe/modules.dot` (via `cargo modules dependencies …` with the symbol-filtering flags baked in, then re-export correction) and `observe/doc_graph.dot` (via `tools/doclinks.py signals` — kept in sync so source and doc graphs match the same working tree). This is the canonical command for "rescore after I changed something." Always prefer it over hand-running `cargo modules` — that path is fragile (missing `--no-fns --no-types --no-traits` silently zeros LOC, and it skips re-export correction).

### 2. Score a subtree directly

```sh
python3 tools/modgraph score --root koan
python3 tools/modgraph score --root koan::machine
```

`--edges` defaults to `observe/modules.dot`. Run `regen` (Recipe 1) first after a code change so the DOT reflects the current tree; subsequent subtree scores in the same session can be plain `score`.

Per-module breakdown plus a single bottom-line **score** — the total structural+size cost over a fixed denominator D (`--denominator`, default 1000), split into `coupling` (cross/feedback edges plus fan-out at each wrapper, with cross-edges deduplicated per `(source_group, dst_module)` so splitting a file doesn't multiply its shared imports), `nesting` (β·loc charge per wrapper layer), and `size` (per-file charge over every module's own file using effective LOC — raw code/comment lines plus attributed doc prose; tests and comments all count toward the size penalty, even though structural terms ignore them). D is a constant scale, not the tree's LOC, so deleting code always lowers the score and growing the tree raises it — compare scores across runs directly.

Each row prints `cross N   fb N   fan N   nest N.N` and `index N.N` (the rolled-up structural cost). `fan` is the provider-side facade signal: for each dst_group at this partition, count distinct dst_modules referenced from outside, subtract one (a single entry — the facade ideal — pays nothing). Tune size with `--gamma` / `--size-pivot`; tune coupling/nesting with `--alpha` / `--beta` / `--beta-children-pivot` / `--lambda-facade`.

For tracked baselines (used by the `verify-koan` skill), pass `--baseline <file>` to either `score` or `regen` — it prunes unreachable-SHA entries (branch checkout / hard reset / rebase drop), prepends today's measurement, trims to 5, and prints a delta line. Dirty-snapshot `+` entries are retained so a pre-commit hook (which always sees a staged-but-not-yet-committed tree) doesn't erase the trend log. For ad-hoc what-if scoring (refactor exploration via `rewrite`), leave the flag off so the baseline file isn't touched.

### 3. Score a *refactor* (module renames)

`tools/modgraph rewrite module` rewrites the DOT graph and mirrors `src/` so you can re-run `score` against a hypothetical layout without touching real files. Make sure `observe/modules.dot` is fresh first (`python3 tools/modgraph regen --root koan` if in doubt), then:

```sh
python3 tools/modgraph rewrite module \
    --edges observe/modules.dot \
    --output-edges /tmp/koan_proposed.dot \
    --output-src /tmp/koan_proposed_src \
    --rename koan::parse::kexpression=koan::ast \
    --rename koan::execute=koan::dispatch::execute

python3 tools/modgraph score --edges /tmp/koan_proposed.dot --root koan \
                             --src-root /tmp/koan_proposed_src
```

Each `--rename OLD=NEW` rebinds module paths matching `OLD` or starting with `OLD::`; both DOT tokens and the mirrored `src/` filenames update (colliding merges concatenate, so the merged file's size is counted honestly). Renames apply against the original path only — chains (`A=B`, `B=C`) must be expressed as the final target. For long lists, use `--rename-file <path>` (one `OLD=NEW` per line, `#` for comments).

### 4. Score a *refactor* (item extraction)

When the seam is "pull these specific items out into a new module" (not a whole-module rename), use `tools/modgraph rewrite item`. It reads a SCIP code-index from `rust-analyzer`, resolves item references at function/method granularity, and surgically rewrites the DOT plus mirrored `src/`:

```sh
rust-analyzer scip . --output /tmp/koan.scip
python3 tools/modgraph rewrite item \
    --scip /tmp/koan.scip --edges observe/modules.dot --src-root src \
    --output-edges /tmp/koan_proposed.dot \
    --output-src /tmp/koan_proposed_src \
    --move koan::machine::core::scope::Scope::register_nominal=koan::machine::core::nominal
```

Item paths follow Rust-canonical spelling (`Module::Struct::method`). Requires `rust-analyzer` on PATH (`rustup component add rust-analyzer` or rely on `rust-toolchain.toml`).

The score before/after gap is what to optimize under either mode: a rewrite is worth doing only if `--root koan` drops by more than rounding noise.

## Pitfalls

- **Score deltas, not absolutes.** The defaults are calibrated, but the absolute number depends on tree shape. Always compare current vs. proposed under the same flags.
- **Stale `observe/modules.dot`.** If you edited code since the last regen, scoring against the tracked DOT compares the old structure to itself. Run `regen` (Recipe 1) before trusting a delta.
- **Re-export correction means "where it's written, not where it's defined."** A facade-routed import (`use crate::machine::Scope`) attributes to the facade; only a deep import (`use crate::machine::core::scope::…`) is charged as deep coupling. Merging files to lower `fan` games nothing if consumers already import through a facade — fix the layout or the import, not the file boundary.
- **The size term is intentionally orthogonal.** Splitting a 2000-line leaf into two 1000-line leaves drops the size charge sharply, but it adds one wrapper layer at structural cost `β · loc(subtree)` — the split is worth it only when the size drop exceeds the structural add.
