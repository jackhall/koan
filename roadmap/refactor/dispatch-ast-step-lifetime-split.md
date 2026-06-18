# Split dispatch's `'run` into AST `'ast` and cart `'step`

Dispatch decide functions carry one lifetime parameter, `'run`, that stands for two
genuinely different lifetimes at once; separating them removes a cart-to-run
over-approximation.

**Problem.** A dispatch decide function names a single lifetime `'run` for both the
long-lived immutable AST it walks (`KExpression<'run>` / `ExpressionPart<'run>`,
across ~12 functions) and the cart-scale things it reads and produces (`Scope`,
`Carried`, `NameOutcome<'run>`, `Outcome<'run>`). These are not the same lifetime:
the AST outlives the run, while the scope and values die with the per-call cart.
The refined functions already keep them apart —
[`extract_binder_install<'run, 'step>`](../../src/machine/execute/dispatch/submit.rs)
takes `expr: &KExpression<'run>` alongside `scope: &'step Scope<'step>` — but most
decide functions collapse both into one `'run`, so
[`current_scope`](../../src/machine/execute/dispatch/ctx.rs) returns `&Scope<'run>`,
widening a frame-bounded (cart-scale) scope up to the AST/run lifetime. That
over-approximation lets a decide handler type a cart-scale scope or value as though
it outlived the run.
[`SchedulerView`](../../src/machine/execute/dispatch/ctx.rs)'s slot 1 is already
named `'step` (the cart content) and `read_result` ties its terminal to the
transient view borrow `'view`, but dispatch call sites still instantiate the view's
slot 1 with their conflated `'run`.

**Acceptance criteria.**

- No dispatch decide function names `'run` for cart-scale content: the pristine-AST
  lifetime is spelled `'ast` and the cart lifetime `'step`, with an `'ast: 'step`
  bound wherever both appear in one signature.
- Every dispatch call site instantiates the read view as `SchedulerView<'step, 'view>`
  — slot 1 is the cart lifetime, never the AST.
- `current_scope`, `build_bare_outcomes`, and the working-expression splice path
  type their scope and values at the cart lifetime `'step`, not widened to the AST
  lifetime.
- No `'ast`-typed `Scope` or `Carried` is produced from a cart-scale read, so a
  decide handler cannot hold a cart value past the cart that backs it.
- `cargo test` and the Miri audit slate stay green.

**Directions.**

- *AST lifetime name — decided.* The pristine-AST lifetime is `'ast`, matching the
  existing `'ast` in [`exec.rs`](../../src/machine/core/kfunction/exec.rs); it is
  assumed to outlive the scheduler.
- *Conflated functions gain a second parameter — decided.* A function using `'run`
  for both `KExpression` and `Outcome` becomes `<'ast, 'step>` with `'ast: 'step`,
  mirroring `extract_binder_install`.
- *View shape — decided.* `SchedulerView` stays two-lifetime `<'step, 'view>` (cart
  content, transient borrow); the AST never lives on the view — it flows through
  method arguments, so splitting it out is per-function work, not a struct change.
- *Working-expression invariance — decided.* The working expression is `'step`.
  `KExpression` / `ExpressionPart` are invariant in their lifetime, but the working
  expression never holds a live `'ast` sub-node: `stage_all_eager_parts` and
  `part_walk` already stage every `Box<KExpression>` / composite-literal part out
  into an `'ast`-born sub-Dispatch (so `PendingSub` / `DepRequest` carry the `'ast`
  half), leaving only lifetime-free owned parts (`Keyword` / `Identifier` /
  `Type(TypeName)` / `Literal`), spliced `Future(Carried<'step>)`, **and** the
  inline `Box<KExpression>` of a *lazy* slot (a `:KExpression` / `:SigiledTypeExpr` /
  `:RecordType` param, left in place for the receiving builtin). The `'ast`→`'step`
  transition happens once, at the birth decide, where no `Future` exists yet: owned
  parts are rebranded into fresh `'step` (destructure-and-rewrap of the owned
  `String` / `TypeName` / `KLiteral` payloads — moves, no clone); a lazy slot's
  `Box<KExpression>` is **structurally cloned into the cart** (`KExpression::clone`).
  The lazy clone is sound because a captured lazy expr is consumed as a
  `KObject::KExpression` *value* (`action.rs` extracts it by `e.clone()`) that the
  consumer-pull lift already deep-clones on every frame crossing — it is value
  semantics, never a borrow into the program AST, so it needs no `'ast` lifetime.
  Target: `stage_all_eager_parts<'ast, 'step>(Vec<ExpressionPart<'ast>>)
  -> (Vec<ExpressionPart<'step>>, Vec<(usize, PendingSub<'ast>)>)`, `'ast: 'step`. No
  two-lifetime `KExpression` type; the only deep clone is the lazy `Box<KExpression>`.

## Dependencies

This is the follow-up to the `'s`→`'step` / `'v`→`'view` / `'frame`→`'step` lifetime
rename and the `SchedulerView<'step, 'view>` definition rename, which shipped
together; this item finishes the job on the dispatch call sites. It may also surface
as a finding of the broader [naming and responsibility audit](naming-and-responsibility-audit.md).

**Requires:** none — a local dispatch refactor on shipped substrate.

**Unblocks:** none tracked yet.
