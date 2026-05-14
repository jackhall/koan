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

- *Module-qualified type names have no path-aware `KType` shape.*
  `TypeExpr` carries a name string that can hold a path like
  `MyMod.Number`, but `KType` has no variant that preserves a
  multi-segment path. The `KType::UserType { kind: Module, scope_id,
  name }` shape that shipped with the type-identity stage 3 carrier (see
  [design/type-system.md § Open work](../design/type-system.md#open-work))
  gives per-module abstract-type identity, but does not let a
  `MyMod.Number` surface-name flow as a typed value end-to-end.
- *Non-SCC forward references in type aliases fail at bind time.* Eager
  elaboration means a type alias's RHS must resolve at bind time. Mutual
  STRUCT / named-UNION recursion is covered by the `pending_types` SCC
  sweep, but a top-level `LET Ty = Un` where `Un` is declared later in
  source (and not in a mutual SCC with `Ty`) still fails.

**Impact.**

- *Module-qualified type names flow through dispatch.* If module-qualified
  type references ever need to be passed as type values (e.g. a `LET MyT
  = MyMod.Number` binding), the carrier shape lands.
- *Source order stops being load-bearing for type aliases.* A top-level
  `LET Ty = Un; LET Un = Number` binds without rejection.

**Directions.**

- *Module-qualified type names — deferred.* Either `KType::UserType {
  kind: Module, ... }` is extended to carry a multi-segment path, or a
  new `KType::Qualified(Path)` variant lands. Decision deferred until a
  use case forces it; current module-system stages do not.
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
