# Eager type elaboration with placeholder-based recursion

Phases 1–2 (one canonical runtime type representation), a meaningful slice
of phase 3 (scheduler-aware FN / STRUCT / UNION elaboration with
self-recursive STRUCT support and `LET Ty = Ty` cycle detection),
parens-wrapped FN parameter type sub-Dispatch, the bulk of the phase-5
cleanup (deletion of `NoopResolver`, the `TypeResolver` trait,
`ScopeResolver`, the legacy `parse_typed_field_list`, and the `&dyn
TypeResolver` parameter on `KType::from_type_expr`), and the
`KType::Unresolved` deletion plus `OnceCell<KType>` late-binding mechanism
(via the `KObject::TypeNameRef(TypeExpr, OnceCell<&'a KType>)` carrier
whose cell *is* the late-binding slot — see
[design/type-system.md § Open work](../design/type-system.md#open-work))
have landed. Mutual STRUCT / named-UNION recursion ships through the
`Bindings.pending_types` SCC pre-registration sweep (see
[design/type-system.md § Open work](../design/type-system.md#open-work)).
This item now narrows to the two genuinely deferred questions the shipped
work does not address: module-qualified type-name paths and non-SCC
forward references.

**Problem.** Two narrow questions remain after the type-identity stages
land:

- *Module-qualified type names don't resolve through the type-side
  lookup.*
  [`Scope::resolve_type(&str)`](../src/runtime/machine/core/scope.rs)
  keys `bindings.types` by flat name, so a TypeExpr like `MyMod.Number`
  (or chained `Outer.Inner.T`) misses. Value-side ATTR already chains
  module-member access via `KObject::KModule` walking —
  [`attr.rs::body_type_lhs`](../src/runtime/builtins/attr.rs) routes
  Type-Type ATTR through `access_module_member` so `Outer.Inner.x`
  resolves left-to-right — but type-position TypeExprs have no
  equivalent walker. No `KType` shape change follows: the resolved type
  is the leaf's existing per-declaration `KType::UserType { kind,
  scope_id, name }` (see
  [design/type-system.md § Open work](../design/type-system.md#open-work)).
- *Non-SCC forward references in type aliases fail at bind time.* Eager
  elaboration means a type alias's RHS must resolve at bind time. Mutual
  STRUCT / named-UNION recursion is covered by the `pending_types` SCC
  sweep, but a top-level `LET Ty = Un` where `Un` is declared later in
  source (and not in a mutual SCC with `Ty`) still fails.

**Impact.**

- *Module-qualified type names resolve in type position.* `LET MyT =
  MyMod.Number` (and chained `Outer.Inner.T`) binds without rejection,
  matching the value-side ATTR chain that already ships.
- *Source order stops being load-bearing for type aliases.* A top-level
  `LET Ty = Un; LET Un = Number` binds without rejection.

**Directions.**

- *Module-qualified type names — deferred.* Teach the type-side
  resolver to walk dotted TypeExpr paths: resolve the head segment to a
  `KObject::KModule` value, then descend through each inner module's
  `bindings.types` for subsequent segments (the same chain
  `access_module_member` already walks on the value side). The resolved
  `KType` is the leaf's existing per-declaration `UserType` — no new
  variant, no path field. Deferred until a use case forces it; current
  module-system stages do not.
- *Forward references and partial definitions — deferred.* Whether to
  extend the
  [`Bindings::types` map](../src/runtime/machine/core/bindings.rs)
  with a placeholder lane for type names beyond an SCC group (mirroring
  `Bindings::placeholders` for values), or to require source-order
  declaration for non-mutually-recursive aliases, is left open until a
  real use case appears.

## Dependencies

**Requires:** none.

**Unblocks:**
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md) —
  higher-kinded slot elaboration (`KType::TypeConstructor`), sharing
  constraints (`<Type: Er.Type>`), and the remaining stage-2 audit slate
  ride on the scheduler-driven elaborator this work lands.
