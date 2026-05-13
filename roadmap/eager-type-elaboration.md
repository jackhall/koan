# Eager type elaboration with placeholder-based recursion

Phases 1–2 (one canonical runtime type representation), a meaningful slice of
phase 3 (scheduler-aware FN / STRUCT / UNION elaboration with self-recursive
STRUCT support and `LET T = T` cycle detection), parens-wrapped FN parameter
type sub-Dispatch, and the bulk of the phase-5 cleanup (deletion of
`NoopResolver`, the `TypeResolver` trait, `ScopeResolver`, the legacy
`parse_typed_field_list`, and the `&dyn TypeResolver` parameter on
`KType::from_type_expr`) have landed. What remains is the `OnceCell<KType>`
late-binding fallback for genuine functor application-time cases and the
`KType::Unresolved` deletion. Mutual STRUCT / UNION recursion (SCC
pre-registration) was originally part of this item; it now ships with
[per-declaration type identity for structs and tagged
unions](per-declaration-type-identity.md), since both touch the same
STRUCT/UNION declaration surface.

**Problem.** Two narrowed gaps remain after the parens-wrapped /
phase-5-cleanup landing:

- *`OnceCell<KType>` late binding for FN parameter types.* No concrete failing
  case has surfaced yet that the parens-wrapped sub-Dispatch + Combine path in
  [`fn_def/signature.rs`](../src/runtime/builtins/fn_def/signature.rs) and
  [`fn_def.rs`](../src/runtime/builtins/fn_def.rs) doesn't already cover.
  Functor bodies substitute outer-FN parameters to `Future(KModule)` before
  the inner FN-def runs, so parens-wrapped types like `(MODULE_TYPE_OF E Type)`
  resolve through the existing Combine path. Closing this requires either a
  concrete failing test or a richer functor-body shape that bypasses the
  substitution.
- *`KType::Unresolved` deletion.* The variant survives as a bind-time carrier
  for bare-leaf user-bound type names (`-> MyT` where `LET MyT = Number`)
  reached via [`ExpressionPart::resolve_for`](../src/ast.rs). The
  `fn_with_user_bound_return_type_works` test in
  [`fn_def/tests/return_type.rs`](../src/runtime/builtins/fn_def/tests/return_type.rs)
  pins this path. Removing the variant requires either a per-slot
  reference-vs-declaration opt-in on `KFunction::classify_for_pick` (currently
  coarse: any pre_run-bearing binder skips wrap and replay-park on all
  literal-name slots), or a new `KObject` carrier preserving the surface
  `TypeExpr` through bind. The variant's docstring in
  [`ktype.rs`](../src/runtime/model/types/ktype.rs) names what would have to
  land first.

**Impact.**

- *Genuine functor late-binding cases get a memoized fallback.* If a
  signature-typed parameter whose type comes from a SIG only in scope at
  functor application time ever surfaces (the parens-wrapped sub-Dispatch
  doesn't already cover it), the resulting `KFunction` carries the original
  `TypeExpr`; the first call re-runs resolution against the FN's captured
  scope and memoizes via one `OnceCell<KType>` per slot.
- *One canonical type carrier on the bind-time path.* Removing
  `KType::Unresolved` collapses the surface-name-string carrier so every
  `KType` flowing through dispatch is fully elaborated. The downstream
  consumers that today recover the surface name from `Unresolved(name)` —
  `extract_bare_type_name`, `dispatch_constructor`, ATTR's TypeExprRef-lhs
  lookup, FN return-type elaboration — read the elaborated `KType` directly.

**Directions.**

- *`OnceCell<KType>` late binding — deferred.* Open until a concrete failing
  case appears that the parens-wrapped sub-Dispatch + Combine path doesn't
  cover. The implementation shape (one `OnceCell<KType>` per signature slot,
  re-resolution against the captured scope on first call) is decided; only
  the trigger is missing.
- *`KType::Unresolved` deletion — deferred.* Open on one of two prerequisites
  landing first: a per-slot reference-vs-declaration opt-in on
  `classify_for_pick` so FN return-type slots can route through the
  auto-wrap rail, or a new `KObject::TypeNameRef(TypeExpr)` carrier
  preserving the surface form through bind. Either is a targeted change but
  out of scope for this item; see the variant docstring on
  [`KType::Unresolved`](../src/runtime/model/types/ktype.rs) for the gating
  detail.
- *Module-qualified type names — open.* `TypeExpr` carries a name string
  that can naturally hold a path like `MyMod.Number`; `KType` has no
  path-aware variant today. If module-qualified type references ever need
  to flow as type values, either `KType::ModuleType` covers the case
  (already path-shaped) or a new `KType::Qualified(Path)` variant is
  needed. Decision deferred until a use case forces it; the current
  module-system stages don't.
- *Forward references and partial definitions — open.* Eager elaboration
  means a type alias's RHS must resolve at bind time. Mutual STRUCT/UNION
  recursion ships with [per-declaration type identity for structs and
  tagged unions](per-declaration-type-identity.md), but a binding whose
  RHS references a name not yet introduced (e.g. a top-level `LET T = U`
  where `U` is declared later in source) still fails. Whether to extend
  `Scope::placeholders` to typed names beyond an SCC group, or to require
  source-order declaration for non-mutually-recursive aliases, is left
  open until a real use case appears.

## Dependencies

**Unblocks:**
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md) —
  higher-kinded slot elaboration (`KType::TypeConstructor`), sharing
  constraints (`<Type: E.Type>`), and the remaining stage-2 audit slate
  ride on the scheduler-driven elaborator this work lands.
