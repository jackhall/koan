# Type identity stage 2 — `KObject::TypeNameRef` carrier and `KType::Unresolved` deletion

Stage 2 of the four-stage type-identity arc. Replaces the surface-name
fallback in the elaborated type language with a `KObject`-side carrier that
preserves the parsed `TypeExpr` and memoizes its resolution. Subsumes the
`KType::Unresolved` deletion and `OnceCell<KType>` late-binding gates left
open by [eager type elaboration](eager-type-elaboration.md).

**Problem.** `ExpressionPart::resolve_for` at
[`ast.rs:142-152`](../src/ast.rs) falls back to `KType::Unresolved(name)`
when `KType::from_type_expr` can't bind a bare-leaf type name at parse
time. `KType::Unresolved` is a surface-name carrier living inside the
elaborated type language. Four downstream consumers — `extract_bare_type_name`,
`dispatch_constructor`, ATTR's `TypeExprRef`-lhs path, and FN return-type
elaboration — recover the surface name out of `KType`, not out of `KObject`.
The variant's
[docstring](../src/runtime/model/types/ktype.rs) records this as transitional.

A related gap: signature-typed parameters whose type resolves only at
functor application time (rather than at FN-definition time) have no
memoization slot — eager elaboration would force a re-walk of the captured
scope on every call. The "no concrete failing case yet" framing in eager's
deferred bullet has been on the books since
[eager type elaboration](eager-type-elaboration.md) shipped its phase-5
cleanup.

**Impact.**

- *One canonical type carrier on the bind-time path.* Removing
  `KType::Unresolved` collapses the surface-name-string carrier so every
  `KType` flowing through dispatch is fully elaborated. The four downstream
  consumers read the elaborated `KType` directly off the `KObject::TypeNameRef`
  cell.
- *Memoized late-binding for genuine functor application-time cases.* The
  `OnceCell<&'a KType>` on `TypeNameRef` is the same mechanism the deferred
  bullet in [eager type elaboration](eager-type-elaboration.md) calls for.
  First resolution against the captured scope memoizes; subsequent reads
  are an `OnceCell::get`.
- *Surface form survives bind for diagnostics.* The captured `TypeExpr`
  carries the user's spelling for error rendering even when the resolved
  `KType` has a different name (e.g. a `LET Ty = Number` alias whose use
  site reads `Ty` — diagnostics can show either form).

**Directions.**

- *`KObject::TypeNameRef` shape — decided.* `KObject::TypeNameRef(TypeExpr,
  OnceCell<&'a KType>)`. The `TypeExpr` is the parsed surface form; the
  cell memoizes the resolved `&'a KType` from
  [`Scope::resolve_type`](../src/runtime/machine/core/scope.rs) (added in
  [stage 1.4](type-identity-1.4-scope-resolve-type-and-rewire.md)).
  `KObject::ktype()`
  reports `KType::TypeExprRef` (same slot kind as `KTypeValue`) so dispatch
  routing is unchanged.

- *`KType::Unresolved` deletion — decided.* The variant deletes in this
  stage. `ExpressionPart::resolve_for`'s fallback at `ast.rs:142-152` emits
  `KObject::TypeNameRef(t.clone(), OnceCell::new())` instead. The four
  consumers gain a `TypeNameRef` arm that calls `tnr.resolve(scope)` and
  reads the cell.

- *Consumer migration — decided.* Each of the four consumers gets an
  explicit `KObject::TypeNameRef` arm:
  - `extract_bare_type_name`
    ([`argument_bundle.rs:82-124`](../src/runtime/machine/kfunction/argument_bundle.rs))
    returns the carrier's surface name directly (declaration slots want
    the user-written name, not the resolved type's name).
  - `dispatch_constructor`, ATTR's `TypeExprRef`-lhs path, and FN
    return-type elaboration each call `tnr.resolve(scope)` and read the
    elaborated `&'a KType`.

- *`tnr.resolve(scope)` API — open.* The `OnceCell` can't capture a scope
  (would tangle lifetimes). Two shapes: (a) every consumer threads its
  body's scope into a `tnr.resolve_in(&scope)` call; (b) `TypeNameRef`
  carries a `&'a Scope<'a>` (impacts size and `deep_clone` shape).
  Recommend (a) — explicit threading, no carrier weight.

- *`Clone` semantics of `TypeNameRef` — open.* `KObject` implements
  `deep_clone` ([`kobject.rs:95-123`](../src/runtime/model/values/kobject.rs)).
  `OnceCell<&'a T>` is `Copy`-able for `Copy` references, so the cell's
  resolved state could be preserved across clones. Recommend preserving
  the cached state when the clone stays within the same scope (the common
  case); resetting on cross-scope clones is the implementer's call.

- *`OnceCell<KType>` for FN parameter slots — covered.* The umbrella
  late-binding case ([eager type elaboration](eager-type-elaboration.md)
  Problem bullet 1) is structurally the same mechanism: the `TypeNameRef`
  cell *is* the per-slot memoization slot. If a genuine functor
  application-time case surfaces post-ship, no new mechanism is needed —
  just route the FN parameter slot through the carrier.

## Dependencies

**Requires:**

- [Type identity stage 1.5 — consumer migration and fallback removal](type-identity-1.5-consumer-migration.md)
  — the `TypeNameRef` carrier resolves through `Scope::resolve_type` with
  no `Scope::resolve` fallback for type names.
- [Type identity stage 1.7 — `LET Ty = Number` routes through `register_type`](type-identity-1.7-let-type-value-writes-types.md)
  — Type-class LET aliases live in `types`, so the carrier's resolution
  picks them up uniformly with builtin types.

**Unblocks:**

- [Type identity stage 3 — `KType::UserType` and per-declaration identity](type-identity-3-user-type-and-per-decl.md)
  — the `KType` variant collapse is cleaner once `KType::Unresolved` is
  already gone.
- [Eager type elaboration with placeholder-based recursion](eager-type-elaboration.md)
  — closes the residual `KType::Unresolved` deletion and `OnceCell<KType>`
  late-binding gates.
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — HKT slot elaboration relies on the canonical-`KType`-only invariant
  this stage establishes.
