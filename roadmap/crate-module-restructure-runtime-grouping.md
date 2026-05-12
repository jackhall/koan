# Crate module restructure — runtime grouping with hoisted AST

**Problem.** The Rust crate's top-level module split
(`parse / dispatch / execute / builtins`) is dominated by one fan-in
(`builtins → dispatch`, 172 edges) and one fan-out
(`* → parse::kexpression`, the AST type that everything consumes). The
`tools/modgraph.py` fractal score (LOC-weighted across every parent
module) is 126.43; the koan top level alone contributes ~85% of the
sum (index 272 × 6118 LOC). Of the 256 top-level cross edges, 218
flow into dispatch and ~32 flow into parse — almost all of the latter
into a single file (`parse::kexpression`).

This is a Rust-side module-layout problem only. No `.koan` source,
no behavior, no public Koan-language surface changes.

**Impact.**

- *Lower fractal complexity.* LOC-normalized fractal score drops from
  126.43 to 89.54 (−29.2%); top-level coupling index drops from 272.0
  to 62.0. Verified by simulating the rename in `/tmp/koan_partitions/`
  with `tools/modgraph.py --fractal koan`.
- *Cleaner top-level layering.* Top-level becomes
  `parse → ast → runtime`: `parse` produces AST values, `runtime`
  consumes them. The `builtins → dispatch::*` fan-in becomes
  `runtime::builtins → runtime::{model, machine}` — same shape,
  internal to `runtime`.
- *Internal partition that matches existing comments.* `dispatch.rs`'s
  module comment already calls out "types and values are the bottom
  of the dependency stack; runtime + kfunction build on them." This
  promotes that distinction to the file tree as `runtime::model` vs
  `runtime::machine`.

**Directions.**

- *Final Rust-module shape — decided.*

  ```
  src/
    lib.rs                       (declares: ast, parse, runtime)
    main.rs
    ast.rs                       (was src/parse/kexpression.rs)
    parse.rs
    parse/                       (unchanged except for the `mod kexpression` removal)
    runtime.rs                   (declares: builtins, model, machine; no re-exports)
    runtime/
      builtins.rs                (was src/builtins.rs)
      builtins/                  (was src/builtins/)
      model.rs                   (NEW; declares `types`, `values`; holds the
                                  re-exports currently in dispatch.rs that come
                                  from `types::*` and `values::*`)
      model/
        types.rs                 (was src/dispatch/types.rs)
        types/                   (was src/dispatch/types/)
        values.rs                (was src/dispatch/values.rs)
        values/                  (was src/dispatch/values/)
      machine.rs                 (was src/dispatch.rs, with the type/value
                                  re-exports removed — they move to model.rs)
      machine/
        kfunction.rs             (was src/dispatch/kfunction.rs)
        kfunction/               (was src/dispatch/kfunction/)
        core.rs                  (was src/dispatch/runtime.rs — RENAMED to avoid
                                  shadowing the new top-level `runtime`)
        core/                    (was src/dispatch/runtime/)
        execute.rs               (was src/execute.rs)
        execute/                 (was src/execute/)
  ```

  No file is split, no new file is created beyond `runtime.rs` and
  `runtime/model.rs` (parent files). Every existing `.rs` file moves
  to exactly one new location.

- *Module path renames — decided.* Apply across the entire crate:

  | Old path                        | New path                            |
  |---------------------------------|-------------------------------------|
  | `crate::parse::kexpression`     | `crate::ast`                        |
  | `crate::parse::ExpressionPart`  | `crate::ast::ExpressionPart`        |
  | `crate::parse::KExpression`     | `crate::ast::KExpression`           |
  | `crate::parse::KLiteral`        | `crate::ast::KLiteral`              |
  | `crate::parse::TypeExpr`        | `crate::ast::TypeExpr`              |
  | `crate::parse::TypeParams`      | `crate::ast::TypeParams`            |
  | `crate::dispatch`               | `crate::runtime::machine`           |
  | `crate::dispatch::kfunction`    | `crate::runtime::machine::kfunction`|
  | `crate::dispatch::runtime`      | `crate::runtime::machine::core`     |
  | `crate::dispatch::types`        | `crate::runtime::model::types`      |
  | `crate::dispatch::values`       | `crate::runtime::model::values`     |
  | `crate::execute`                | `crate::runtime::machine::execute`  |
  | `crate::builtins`               | `crate::runtime::builtins`          |

  Names imported via the re-exports listed below also rebind from
  `crate::dispatch::*` to either `crate::runtime::machine::*` (kfunction
  / core re-exports) or `crate::runtime::model::*` (types / values
  re-exports). The four bullets below specify the split.

- *`parse.rs` edits — decided.* Delete `mod kexpression;` and the
  `pub use kexpression::{ExpressionPart, KExpression, KLiteral,
  TypeExpr, TypeParams};` line. Inside `parse/`, every file that uses
  one of those types updates its `use` to `use crate::ast::...`.
  No `pub use crate::ast::...;` re-export is added in `parse.rs` — the
  new path is the only path.

- *`ast.rs` content — decided.* Identical contents to the current
  `src/parse/kexpression.rs`. Public names (`ExpressionPart`,
  `KExpression`, `KLiteral`, `TypeExpr`, `TypeParams`) become
  `pub`-at-`crate::ast` directly (the `pub use` from `parse.rs` is
  not reproduced; the items themselves are already `pub` in
  `kexpression.rs`).

- *`runtime/model.rs` content — decided.* Declares `types` and
  `values` as `pub(crate) mod`. Holds the subset of the old
  `dispatch.rs` `pub use` block that originates in `types` or
  `values`:

  ```rust
  pub use types::{
      Argument, ExpressionSignature, KType, Parseable, Serializable,
      SignatureElement, UntypedElement, UntypedKey, is_keyword_token,
  };
  pub use values::{KKey, KObject};
  ```

  No other content.

- *`runtime/machine.rs` content — decided.* Identical to today's
  `src/dispatch.rs` minus the `pub use types::*` and `pub use
  values::*` lines (those moved to `model.rs`). Module declarations
  become:

  ```rust
  pub(crate) mod kfunction;
  pub(crate) mod core;       // was: mod runtime;
  pub(crate) mod execute;    // NEW — was top-level
  ```

  The `pub use kfunction::*` and `pub use runtime::*` re-exports
  stay, with `runtime` renamed to `core`:

  ```rust
  pub use kfunction::{
      ArgumentBundle, Body, BodyResult, CombineFinish, KFunction, NodeId, SchedulerHandle,
  };
  pub(crate) use kfunction::substitute_params;
  pub use core::{CallArena, Frame, KError, KErrorKind, KFuture, Resolution, RuntimeArena, Scope};
  ```

  The module-doc comment at the top updates its enumeration of
  submodules to match (`runtime` → `core`; add `execute`).

- *`runtime.rs` content — decided.* Three `pub mod` declarations
  and nothing else:

  ```rust
  //! Runtime — everything that consumes a parsed `KExpression` to produce a value.
  //! `model` is the value/type vocabulary; `machine` is the dispatcher, scheduler,
  //! and executor; `builtins` is the K-language standard library implemented on top.

  pub mod builtins;
  pub mod model;
  pub mod machine;
  ```

  No re-exports — callers reach into the named subtree.

- *`lib.rs` edits — decided.* Replace the four `pub mod` lines with:

  ```rust
  pub mod ast;
  pub mod parse;
  pub mod runtime;
  ```

  Update the doc comment's `[`execute::interpret`]` reference to
  `[`runtime::machine::execute::interpret`]`.

- *`runtime/machine/core.rs` (renamed from `runtime.rs`) — decided.*
  File contents unchanged. The submodule files in `runtime/machine/core/`
  (`arena.rs`, `dispatcher.rs`, `kerror.rs`, `scope.rs`) are byte-for-byte
  identical to their old `dispatch/runtime/` originals; their
  `use crate::dispatch::...` paths rebind per the rename table above.

- *Naming choices — decided.*
    - Top-level holder for dispatch + execute + builtins is `runtime`
      (not `core`, not `engine`).
    - `dispatch::runtime` becomes `runtime::machine::core` (not
      `runtime::machine::runtime`). The shadow would be legal but
      harmful to grep.
    - `kexpression` becomes `ast` (not `ir`, not `expr`).
    - `dispatch` becomes `machine` (not `dispatcher`, not `engine`).
    - Internal split inside `machine` keeps `kfunction` and `execute`
      as siblings of `core`; do not nest them.

- *Visibility — decided.* All `pub(crate) mod` stays `pub(crate) mod`;
  all `pub mod` stays `pub mod`. The new `runtime/model.rs` and
  `runtime/machine.rs` both use `pub(crate) mod` for their children
  (matching the current `dispatch.rs` discipline). `runtime.rs` uses
  `pub mod` for its three children.

- *Tests — decided.* Test files live alongside their parents and
  move with them (e.g. `src/builtins/fn_def/tests/*.rs` moves to
  `src/runtime/builtins/fn_def/tests/*.rs`). Inline `#[cfg(test)]
  mod` blocks travel with the file. No test is renamed or rewritten;
  imports inside tests update per the rename table.

- *Doc-comment paths — decided.* Doc comments referencing
  `dispatch/`, `execute/`, or `parse/kexpression.rs` by file path
  update to the new path. Mentions of `KType` / `Scope` / `KError` /
  etc. by Rust-name are unaffected (the names don't change).

- *Verification — decided.* The refactor is complete when:
    1. `cargo build` succeeds.
    2. `cargo test` passes (no test logic changes; only `use` paths).
    3. `cargo clippy` has no new warnings.
    4. `cargo modules dependencies --package koan --lib --no-externs
       --no-sysroot --no-traits --no-fns --no-types > /tmp/koan.dot`
       followed by `python3 tools/modgraph.py --edges /tmp/koan.dot
       --fractal koan` reports a LOC-normalized fractal score within
       ±0.5 of **89.54** (the simulated target).
    5. No symbol is renamed; no item changes visibility; no module
       has a non-mechanical edit.

- *Out of scope — decided.* No `.koan` source files change. No
  signature, struct field, or trait method changes. No reduction of
  `pub use` re-exports beyond the redistribution between `model.rs`
  and `machine.rs` mandated above. No introduction of a re-export
  shim from `crate::parse` to `crate::ast` (callers update directly).
  No additions to `runtime.rs` or `runtime/model.rs` beyond what is
  spelled out above.

## Dependencies

**Requires:** none. This is a pure Rust-side rename; the crate has
no external consumers and no `pub` API contract to preserve.

**Unblocks:** none on the language roadmap. Implicit benefit: future
cross-module work has a smaller dep graph to reason about.
