# `KType` concern split

[`ktype.rs`](../src/dispatch/types/ktype.rs) (694 LOC) merges multiple
concerns into one file; split it along its concern boundaries to reduce
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

**Impact.**

- *Concern boundaries are enforceable by file location.* A reader looking
  for "how does Koan resolve a type name to a `KType`" finds it in
  ~150-200 LOC rather than skimming a 694-LOC mixed file.
- *Adding a new variant or rule lands in one place.* New `KType` variant ⇒
  core enum file. New predicate ⇒ predicates file. New resolution rule ⇒
  resolution file. The split makes the concern boundary the natural
  insertion point.
- *Substrate for [static-typing-and-jit](static-typing-and-jit.md).* The
  split-out resolution module gives a typing pre-pass a focused surface
  for type-expression elaboration.

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
- *`TypeResolver` flattening — deferred.* The trait at
  [`resolver.rs`](../src/dispatch/types/resolver.rs) has two impls
  (`NoopResolver`, `ScopeResolver`) and one consumer
  (`KType::from_type_expr`). After the resolution-module split, decide
  whether to flatten to a closure parameter (`Fn(&str) -> Option<KType>`)
  or keep the trait for explicit naming. Punted to the resolution-module
  split — the right answer depends on how that module reads.

## Dependencies

**Requires:**
- [Module system stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — stage 2 is likely to introduce new `KType` variants (functor result
  types, signature-bound slots) and new dispatch paths (functor application's
  generative type minting). Wait until those stabilize before locking in
  a file split.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md) — the
  split-out resolution module gives the checker a focused surface for
  type-expression elaboration without coupling to the rest of `KType`'s
  predicate and substitution logic.
