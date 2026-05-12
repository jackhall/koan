---
name: rust-refactor
description: Use this skill when refactoring Rust code in the koan repo — renaming symbols across the crate, moving items between files/modules, extracting a function or sub-module, applying batch structural search-and-replace, pruning unused code, auto-fixing compiler/clippy diagnostics, or evaluating module-structure changes against the dep graph. Pairs `ast-grep` for structural rewrites, `cargo clippy --fix` / `cargo fix` for lint-driven cleanup, `cargo build` as the verification step, and `tools/modgraph.py` for partition-complexity scoring.
---

# rust-refactor

Command-line workflow for Rust refactors in the koan repo. Four tools, in order of trust:

1. **`cargo build` / `cargo clippy`** — the compiler is the source of truth.
2. **`cargo fix` / `cargo clippy --fix`** — auto-apply suggestions the compiler already knows about.
3. **`ast-grep`** — pattern-based structural rewrites for things the compiler can't do alone (renames, moves, signature reshapes).
4. **`cargo modules` + `tools/modgraph.py`** — score a proposed top-level module partition against the live dep graph before committing to a reshuffle.

Assumes `ast-grep`, `cargo clippy`, and `cargo modules` are on PATH.

## When to reach for which tool

| Situation | Tool |
| --- | --- |
| Compiler or clippy emitted a fixable suggestion | `cargo fix` / `cargo clippy --fix` |
| Same syntactic pattern appears in many places | `ast-grep` (with `--rewrite` for SSR) |
| Renaming a symbol across the crate | `ast-grep` + `cargo build` |
| Moving an item between files/modules | move by hand, then `cargo build` + `cargo fix` |
| Anything touching types, lifetimes, or trait bounds | finish with `cargo build` to verify |
| Considering a module split / merge / rename, or moving several items between files/modules | `cargo modules` + `tools/modgraph.py` |

## Recipes

### 1. Apply compiler and lint suggestions

```sh
cargo fix --allow-dirty --allow-staged           # rustc suggestions
cargo clippy --fix --allow-dirty --allow-staged  # clippy suggestions
cargo build                                       # verify
```

Commit between the two `--fix` passes so each diff is reviewable on its own.

### 2. Find every call site of a symbol

```sh
ast-grep --lang rust --pattern 'foo($$$ARGS)'    # function calls
ast-grep --lang rust --pattern '$X.foo($$$ARGS)' # method calls
ast-grep --lang rust --pattern 'use $$$::foo'    # imports
```

`$$$NAME` matches a list of nodes (zero or more); `$NAME` matches a single node. Scope with a path (e.g. `ast-grep ... src/parse/`) when the same name might mean different things in different modules.

### 3. Rename a symbol across the crate

```sh
# Preview first — no --update-all means no writes.
ast-grep --lang rust --pattern 'old_name' --rewrite 'new_name'

# Apply.
ast-grep --lang rust --pattern 'old_name' --rewrite 'new_name' --update-all
cargo build
```

For renames that also reshape args, use metavariables: `--pattern 'old_name($A, $B)' --rewrite 'new_name($B)'`.

### 4. Move an item from one file to another

1. Confirm the definition: `ast-grep --lang rust --pattern '<item-signature>'`.
2. Find references using the patterns from recipe 2.
3. Cut the item, paste it into the new file. Add `pub` if it now crosses a module boundary.
4. `cargo build`. The "unresolved import" / "cannot find … in this scope" errors are the worklist.
5. `cargo fix --allow-dirty` resolves many of those automatically.
6. Re-run `cargo build` and `cargo clippy` to confirm clean.

### 5. Structural pattern rewrite

Example: convert free-function `foo(x, y)` into method-call `x.foo(y)` across the crate.

```sh
ast-grep --lang rust \
  --pattern 'foo($X, $Y)' \
  --rewrite '$X.foo($Y)' \
  --update-all
cargo build
```

Always preview without `--update-all` first — `ast-grep` doesn't know types, so a rewrite that's correct for one `foo` may be wrong for another.

### 6. Extract a function or sub-module

**Function.** Copy the block into a new `fn` with explicit params for every captured local. `cargo build`; the compiler reports any captured names you missed. Replace the original block with a call to the new `fn`.

**Sub-module.** Create the new file (e.g. `src/parse/foo.rs`), add `mod foo;` to the parent (`src/parse.rs` or `src/parse/mod.rs`). Move items, mark `pub` whatever crosses the boundary. `cargo build`; the compiler lists every now-unresolved import; `cargo fix --allow-dirty` auto-adds many of them. Commit before `cargo fix` so the auto-edit diff is reviewable.

### 7. Prune unused code

```sh
cargo build 2>&1 | grep -E 'warning: (unused|never used|dead_code)'
cargo clippy -- -W dead_code -W unused
```

Then delete what's truly dead, or mark `#[allow(dead_code)]` deliberately for things kept on purpose. If `cargo machete` is on PATH, run it for unused-dependency detection.

### 8. Score a proposed module partition

Before reshuffling code between top-level modules, measure whether the new shape actually
reduces cross-module coupling. `tools/modgraph.py` reads a [cargo-modules](https://github.com/regexident/cargo-modules)
DOT graph and a TOML partition spec, then reports a complexity index
(`cross_edges + alpha * feedback_weight`, where feedback is the weight of
edges that go against the best topological order).

```sh
cargo modules dependencies --package koan --lib \
    --no-externs --no-sysroot --no-traits --no-fns --no-types \
    > /tmp/koan.dot

cat > /tmp/partition.toml <<'EOF'
[groups]
parse    = ["koan::parse"]
model    = ["koan::dispatch::types", "koan::dispatch::values"]
machine  = ["koan::dispatch::kfunction", "koan::dispatch::runtime",
            "koan::dispatch", "koan::execute"]
builtins = ["koan::builtins"]
EOF

python3 tools/modgraph.py --edges /tmp/koan.dot --partition /tmp/partition.toml
```

Output reports the index, the best topological order, the back-edges (which
moves would most reduce the index), and the full src→dst matrix. Score the
current partition the same way — a proposed reshuffle is only worth doing if
its index is lower.

To score the internal coupling of a single subtree (one group per direct
child), skip the TOML and use `--children-of`:

```sh
python3 tools/modgraph.py --edges /tmp/koan.dot --children-of koan::dispatch
```

To score every parent module recursively and aggregate (weighted by lines
of code per subtree):

```sh
python3 tools/modgraph.py --edges /tmp/koan.dot --fractal koan
```

Reports per-module index plus `Σ index·loc` and the LOC-normalized average.
Each level's coupling index is weighted by the LOC under that node, so
internal tangles in large modules count for more than the same tangle in a
small one.

## Pitfalls

- `ast-grep` is syntactic, not type-aware. It cannot disambiguate two `foo`s in different modules — scope by path or do the rewrite in stages.
- Don't stack `cargo clippy --fix` on top of uncommitted `ast-grep` rewrites in the same pass. Commit between phases so a bad rewrite is easy to revert.
