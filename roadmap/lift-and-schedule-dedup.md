# Lift-walk and aggregate-scheduler dedup

Several `KObject`-tree walks and per-variant scheduler paths in `execute/`
duplicate the same shape with only leaf decisions differing — collapse to
parametric helpers.

**Problem.** Three near-duplicate patterns are visible in the runtime:

- `needs_lift` ([`lift.rs:118-133`](../src/execute/lift.rs#L118-L133)) and
  `kobject_borrows_arena` ([`lift.rs:180-194`](../src/execute/lift.rs#L180-L194))
  walk the same `KObject` composite shape (`List`, `Dict`, `Tagged`, `Struct`,
  `KFuture`, `KFunction`) with the same recursion strategy; they differ only
  in the leaf decision and the bottom-out condition. Two parallel trees of
  pattern matches that have to stay in sync as new composite variants land.
- `schedule_list_literal`
  ([`run.rs:179-203`](../src/execute/run.rs#L179-L203)) and
  `schedule_dict_literal`
  ([`run.rs:210-256`](../src/execute/run.rs#L210-L256)) iterate their parts,
  pattern-match on `ExpressionPart` types identically, and call
  `self.add(NodeWork::Dispatch(...))` the same way. Only the result container
  differs. The same shape repeats in `run_aggregate` and `run_aggregate_dict`
  ([`run.rs:148-291`](../src/execute/run.rs#L148-L291)) — same iteration over
  resolved deps, same reclamation, same allocation, different container
  builder.
- Module / signature resolution is duplicated between `ascribe`'s
  `resolve_module` / `resolve_signature`
  ([`ascribe.rs:190-238`](../src/dispatch/builtins/ascribe.rs#L190-L238))
  and `type_ops`'s `resolve_module_arg`
  ([`type_ops.rs:184-206`](../src/dispatch/builtins/type_ops.rs#L184-L206)).
  The same dual-shape pattern (`KObject::KModule(_) | KObject::TypeExprValue(t)`
  with scope-chain lookup) appears in both, with identical error wrapping.

**Impact.**

- *One walker per concern in `lift.rs`.* Adding a new descendant predicate
  (e.g., for a future audit-pass walker) gets a closure rather than a third
  copy of the variant tree. Only the walker knows the composite shape;
  consumers express their question as a closure.
- *One literal scheduler.* Adding a third literal type (tuple literal,
  set literal) becomes a one-call-site addition rather than a third copy of
  the per-element scheduling and `ExpressionPart` matching.
- *One module-resolution helper.* ATTR's RHS, the ascription operators' LHS,
  and `MODULE_TYPE_OF`'s subject all share a single source of "given a value
  or a `TypeExprRef`, produce a `&'a Module<'a>`" logic, with one error
  shape and one place to extend when a new "module-like" carrier lands.

**Directions.**

- *Walker shape — open.* Two viable forms: (a)
  `any_descendant<F: Fn(&KObject) -> Option<bool>>(v, predicate)` where
  the predicate returns `Some(true|false)` for a leaf decision and `None`
  to recurse, or (b) two methods `is_some_descendant<F>(v, leaf_check: F)`
  with recurse-implicit. Recommended: (a) — explicit recurse signal makes
  the bottom-out condition match the existing structure 1:1.
- *Aggregate-and-scheduler generic — decided.* A parametric scheduler taking
  the per-element type and a "build container from `Vec<KObject>`" closure.
  Mirrors the `Bind` / `Aggregate` distinction the scheduler already
  exposes — the helper carries the iteration and reclamation; the closure
  carries the container shape.
- *Module-resolution helper placement — decided.* Helper lives next to
  `Module` / `Signature` in
  [`dispatch/values/module.rs`](../src/dispatch/values/module.rs) since
  both consumers cross builtin boundaries. Signature:
  `pub(crate) fn resolve_module<'a>(scope: &'a Scope<'a>, obj: &KObject<'a>) -> Result<&'a Module<'a>, KError>`
  plus the symmetric `resolve_signature`.
- *`ValueConstructor` trait for struct / tagged-union construction —
  deferred.* `apply`'s shape (validate args → build value → return as
  `BodyResult::Tail`) is shared between
  [`struct_value.rs:43-80`](../src/dispatch/values/struct_value.rs#L43-L80)
  and
  [`tagged_union.rs:38-67`](../src/dispatch/values/tagged_union.rs#L38-L67),
  but the actual code overlap is ~20-30 LOC. The trait may add more glue
  than it saves. Revisit if a third constructor lands or if a downstream
  feature needs a uniform construction protocol.

## Dependencies

**Requires:**
- [Module system stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — stage 2 is actively churning the lift and schedule code paths (new
  module-and-functor lift sites, new audit-slate Miri tests targeting
  these areas, generative type minting in functor application).
  Consolidating now would force a re-do once stage 2's reshape lands.

**Unblocks:**

No specific roadmap items downstream — this is internal cleanup.
