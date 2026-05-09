# `KType` and dispatcher concern split

[`ktype.rs`](../src/dispatch/types/ktype.rs) (694 LOC) and
[`scope.rs`](../src/dispatch/runtime/scope.rs) (601 LOC) each merge multiple
concerns into one file; split each along its concern boundaries to reduce
per-file cognitive load and clarify the module surface.

**Problem.**

- `ktype.rs` (694 LOC; ~365 LOC of impl + ~330 LOC of tests) merges six
  distinct concerns into one file: the core `KType` enum (lines 33-93), the
  display-style `name()` (130), specificity (`is_more_specific_than` at 99),
  predicate matching (`matches_value` at 237, `accepts_part` at 270,
  `function_compat` at 375), name resolution (`from_name` at 159,
  `from_type_expr` at 188), and generic substitution (`join` at 343,
  `join_iter` at 365). Adding a new `KType` variant means touching every
  one of those concerns; adding a new predicate or a new resolution rule
  means skimming the full file to find the right insertion point.
- `scope.rs` (601 LOC) mixes lexical-environment storage (`data`, `functions`,
  `out`, `arena`, `pending` — lines 44-60) with overload-resolution logic
  (`dispatch` at 230-248, `lazy_candidate` at 270-289, `pick` at 293-327).
  The two responsibilities don't share state — they share a `struct` because
  the dispatch logic happens to read `functions`. The dispatch logic threads
  through scope-chain walks at multiple sites; the storage logic threads
  through `RefCell` borrow-conflict deferral in `pending`. Two distinct
  rationales braided through one file.

**Impact.**

- *Concern boundaries are enforceable by file location.* A reader looking
  for "how does Koan resolve a type name to a `KType`" finds it in
  ~150-200 LOC rather than skimming a 694-LOC mixed file; "how does
  dispatch pick between overloads" lives in a focused dispatcher file
  rather than the middle third of `scope.rs`.
- *Adding a new variant or rule lands in one place.* New `KType` variant ⇒
  core enum file. New predicate ⇒ predicates file. New resolution rule ⇒
  resolution file. New dispatch heuristic ⇒ dispatcher file. The split
  makes the concern boundary the natural insertion point.
- *Substrate for [static-typing-and-jit](static-typing-and-jit.md).* A
  pre-execution typing pre-pass has a stable, focused dispatcher entry
  point to call into without coupling to `Scope`'s storage internals; the
  split-out resolution module gives the checker a focused surface for
  type-expression elaboration.

**Directions.**

- *`KType` split shape — decided.* Three files:
  - `ktype.rs` — core enum + `name()` Display (~200 LOC).
  - `ktype_predicates.rs` — `matches_value`, `accepts_part`,
    `is_more_specific_than`, `function_compat` (~150 LOC).
  - `ktype_resolution.rs` — `from_name`, `from_type_expr`, `join`,
    `join_iter` (~150 LOC).

  Tests stay co-located with the methods they exercise. Keep `pub use`
  re-exports at the
  [`dispatch/types.rs`](../src/dispatch/types.rs) module root so external
  callers see no API change.
- *Dispatcher extraction — decided.* New module
  `src/dispatch/runtime/dispatcher.rs` with free functions taking `&Scope`:

  ```rust
  pub(crate) fn dispatch<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> Result<KFuture<'a>, KError>
  pub(crate) fn lazy_candidate<'a>(scope: &'a Scope<'a>, expr: &KExpression<'_>) -> Option<Vec<usize>>
  ```

  `pick` and any other pure-resolution helpers move with them. `Scope::dispatch`
  and `Scope::lazy_candidate` become thin forwarders so the public surface
  on `Scope` is unchanged. After the move, `scope.rs` shrinks to
  lexical-environment storage and direct mutators only.
- *`TypeResolver` flattening — open.* The trait at
  [`resolver.rs`](../src/dispatch/types/resolver.rs) has two impls
  (`NoopResolver`, `ScopeResolver`) and one consumer
  (`KType::from_type_expr`). After the resolution-module split, decide
  whether to flatten to a closure parameter (`Fn(&str) -> Option<KType>`)
  or keep the trait for explicit naming. Recommended: revisit at split
  time — the right answer depends on how the resolution module reads.

## Dependencies

**Requires:**
- [Module system stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — stage 2 is likely to introduce new `KType` variants (functor result
  types, signature-bound slots) and new dispatch paths (functor application's
  generative type minting). Wait until those stabilize before locking in
  a file split.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md) — a
  clean dispatcher boundary makes a typing pre-pass feasible: the checker
  can call into the dispatcher's overload-resolution entry points without
  coupling to `Scope`'s storage internals, and the split-out resolution
  module gives the checker a focused surface for type-expression
  elaboration.
