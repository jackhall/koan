# Per-call type-parameter binding in parameter signatures

Free type-parameter names in an FN's parameter signatures get bound per
call. The binding source is one of two: extraction from an argument's
carried type structure, or the value of an earlier parameter in the same
signature. Both sub-cases share the same `Deferred`-carrier widening, the
same per-call scope installation seam, and the same deferred-return
re-elaboration path.

**Problem.** A parameter's declared type can't reference a free name
today — neither one that's *inferred* from an argument's type structure
nor one *bound by* an earlier parameter in the same signature. Parameter
types resolve against the FN's outer scope at definition time, so any
free name has nowhere to bind:

- *Type-parameter inference from value-slot arguments.* Parameterized
  values carry their type arguments
  ([ktype.md § Runtime type-parameter carriers](../../design/typing/ktype.md#runtime-type-parameter-carriers)),
  and a `:(LIST OF T)` / `:(Result T E)` slot admits a value structurally
  via the [`matches_value`](../../src/machine/model/types/ktype_predicates.rs)
  `ConstructorApply` / container arms. But a free type-parameter name in
  the slot has nowhere to bind: `FN head (xs :(LIST OF T)) -> :T = ...`
  can't be defined, because `T` in the deferred return resolves through
  scope and nothing populates that scope from the argument the call
  actually carried. A generic-destructuring unifier — walking a slot's
  elaborated `KType` against a value's carried `KType` and collecting
  `(param_name, concrete)` bindings — is the missing piece, and nothing
  in [`invoke.rs`](../../src/machine/core/kfunction/invoke.rs) populates
  the per-call scope from the argument. (An earlier `TypeExpr`-walking
  `unify_slot` scaffold was removed when positional type syntax was
  retired; with parameterized surface `TypeExpr`s gone, the unifier must
  walk `KType` ↔ `KType` — a value's carried `KType` against a slot's
  elaborated `KType` — not surface `TypeExpr` ↔ `KType`.)

- *Cross-parameter references in the same signature.* `(FN (MAKE T:
  Type elt: T) -> T = ...)` errors at FN-definition because `T` isn't
  bound at signature-elaboration time. OCaml's multi-parameter functor
  signatures carry this exact shape — `module Make (E : ORDERED) (S :
  SET with type elt = E.t) = ...` — the second parameter's signature
  mentions the first. Without it, multi-parameter functors can't express
  cross-parameter sharing constraints at the parameter list; the
  workaround is encoding the constraint in the body or routing through
  a paired tuple-module.

Both forms hit the same wall: the parameter type slot is a `KType`
resolved once against the FN's outer scope. There's no carrier for "this
slot's type is determined per call against the per-dispatch frame."

A third gap is **name discrimination**. Without an explicit declarator
for free type-parameter names, a slot leaf that doesn't resolve in scope
would have to be inferred as a free param — making a typo
(`:(LIST OF U)` for the intended `:(LIST OF T)`) silently register `U` as a
fresh free parameter rather than erroring. The signature would accept
more values than the author intended, and the misspelled return would
unify trivially with anything.

**Impact.**

- *Generic value-slot functions become definable.* `FN head (xs :(LIST
  OF T)) -> :T` and `FN unwrap (r :(Result T E)) -> :T` type-check per call
  against the argument the caller actually passed. Type-parameter
  dispatch on value slots resolves the deferred return against the
  bound concrete type, so the per-call return matches the argument's
  instantiation.
- *Multi-parameter OCaml-style functors with sharing constraints become
  writable.* Generalizes the single-parameter functor surface
  ([design/typing/functors.md](../../design/typing/functors.md)) so the
  second parameter's signature can pin a slot to the first parameter's
  abstract type.
- *Dependent value-typed parameters become writable.* Constructions like
  `(BUILD T: Type x: T)` — accept a type, then accept a value of that
  type — are first-class.
- *One per-call scope-installation seam serves both sources.* Both
  binding paths (argument-extracted, earlier-parameter-bound) install
  `(param_name, concrete)` into the per-call scope via
  `Scope::register_type` before the deferred return elaborates — same
  call site, same scope-write semantics, same downstream elaboration.
- *Typo errors fail loud.* A misspelled type-parameter name that doesn't
  match a declared TypeVar and doesn't resolve to an in-scope type errors
  at FN-definition rather than silently registering as a fresh free
  parameter.
- *Bounded TypeVars become writable.* `LET T = (Union Number Str)` or
  `LET T = (SIG_WITH Foo ((Type: Number)))` constrains the per-call
  binding to a sub-admissibility region of the type lattice, paralleling
  Java/Rust/Scala bounded generics on koan's existing
  specificity-respecting machinery.

**Directions.**

- *Carrier widening — decided.* Parameter type slots widen to the same
  `ReturnType { Resolved(KType), Deferred(DeferredReturn) }` carrier
  shipped at
  [`ExpressionSignature::return_type`](../../src/machine/model/types/signature.rs).
  Selection at FN-definition: scan each parameter type's `TypeExpr` for
  leaves matching either an earlier parameter name *or* a slot-local
  free type-parameter name (e.g. `T` in `:(LIST OF T)`).
- *Binding-source unification — decided.* Both sources install
  `(param_name, concrete)` into the per-call scope via
  `Scope::register_type` before deferred elaboration runs. A
  `KType`-walking unifier handles the argument-extraction case; the
  earlier-parameter case routes through the existing per-call type-side
  install path for type-denoting parameter values.
- *Per-call binding site — decided.* The invoke per-call type-side install
  site, before deferred-return elaboration — the seam the unifier targets.
- *Type-parameter declaration — decided.* Free type-params are declared
  explicitly via `LET TypeName = <bound>` at FN/module scope — the same
  arity-0 declarator that SIG bodies already use for abstract type slots
  ([design/typing/modules.md](../../design/typing/modules.md)), extended
  to outer scopes. `LET T = AnyType` is the unconstrained form; any type
  expression on the RHS imposes an upper admissibility bound
  (`LET T = Number`, `LET T = (Union Number Str)`,
  `LET T = (SIG_WITH Foo ((Type: Number)))`). The unifier populates the
  per-call `params` set from declared TypeVar names in the FN's
  enclosing scope, not from scope-failure on slot leaves; a slot leaf
  that doesn't match a declared name *and* doesn't resolve in scope is
  an error, closing the typo hole.
- *Type-parameter carrier — decided.* TypeVar identity is by declaration
  name, matching the existing
  [`KType::UserType { kind, scope_id, name }`](../../src/machine/model/types/ktype.rs)
  pattern: `(scope_id, name)` is the identity discriminator, with the
  declared bound carried as a separate field. Two `LET T1 = AnyType` /
  `LET T2 = AnyType` declarations produce distinct TypeVars whose slots
  unify independently; multiple uses of the same TypeVar name couple per
  call, so `(FN combine (a :T, b :T) -> :T = ...)` requires both
  arguments to land on the same concrete type and threads it to the
  return.
- *Bound-and-specificity — decided.* The RHS bound is an upper
  admissibility limit, respected with koan's existing specificity rule:
  a slot `:T` with `LET T = Number` admits Number and anything strictly
  more specific (a NEWTYPE-over-Number, for example). Reuses the
  dispatcher's `is_more_specific` logic rather than introducing a
  parallel exact-type comparison.
- *Open-RHS aliases — decided.* `LET TypeName = <expr>` outside SIG
  bodies always carries TypeVar semantics; for fully-concrete RHS this
  is observably equivalent to the alias reading it replaces (the
  coupling is trivial when every slot must already be the same concrete
  type). Users who want per-slot independent Union admission write
  `:(Union ...)` inline rather than going through a name.
- *Bound-chaining for cross-TypeVar references — decided.* A TypeVar
  RHS that references another declared TypeVar preserves the reference
  structurally — `LET T1 = T2` gives T1 a distinct
  `(scope_id, "T1")` identity with bound `T2`, and the unifier resolves
  T1's admissibility by chasing through T2's per-call binding rather
  than collapsing to T2's bound at declaration time. Compound RHS
  expressions follow the same rule: `LET T1 = (Union T2 Number)` admits
  values whose type matches T2's per-call binding *or* Number,
  preserving both the cross-TypeVar coupling and any concrete
  constraints alongside it. Matches OCaml's `with type t = X.t` sharing
  semantics; collapse-at-declaration is rejected because it cascades
  constraint loss through compound RHS expressions (any
  `(Union X TypeVar)` would silently widen to `AnyType` and erase X).
- *Dispatch staging — open.* Slot N's admissibility depends on bindings
  from slot M < N. Two paths:
  - *(a) Staged left-to-right dispatch.* At dispatch time, resolve
    parameters in order. After binding slot M, install M into a
    per-dispatch scope; re-elaborate slot N's `Deferred` type against
    that scope; admissibility-check slot N. Touches
    `signature_admits_strict`, `Scope::resolve_dispatch`, and the
    dispatch index's lookup keys.
  - *(b) Index-side projection.* Compute admissibility partially at
    definition (against everything that *can* be resolved) and complete
    the check at dispatch time. Lighter on dispatch, heavier on the
    index.
- *Overload conflict rules — open.* Two FNs with the same fixed-token
  shape but different dependent-annotation patterns (one `(MAKE T:
  Type elt: T)`, the other `(MAKE T: Type elt: Number)`) need a
  comparison rule for "more specific." Today's overload resolution is
  concrete-type-keyed; dependent annotations need a partial-order
  extension.
- *Tripwire extension — decided.* The existing
  [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  `debug_assert!` that guards deferred-return Any-coarsening
  extends over argument-slot deferreds — same invariant ("no consumer
  compares a deferred-carrying slot against a non-`Any` structural slot
  yet"), same forcing condition. The full precision fix stays parked in
  [kfunction-deferred-ret-precision.md](kfunction-deferred-ret-precision.md),
  whose scope grows to cover *parameter and return* slots in one pass
  once the tripwire fires.

## Dependencies

**Requires:**

None — the substrate (runtime carriers, `matches_value` admission arms,
ascription stamping, `ReturnType` carrier, per-call type-side install for
type-denoting parameters) is shipped; this item builds the
`KType`-walking slot unifier, wires it into the invoke path, and widens
parameter slots to admit the deferred-carrier shape.

**Unblocks:**

- [Structural KFunction admission across deferred parameter and return slots](kfunction-deferred-ret-precision.md)
  — widening parameter slots to the `Deferred(_)` carrier and extending
  the tripwire over them grows the precision item's scope to cover both
  surfaces in one pass.

`List` / `Dict` element-type dispatch and the builtin `Result`
parameterized type
([error-handling](../../design/error-handling.md)) already function on
the shipped carriers; this item extends them to generic value-slot
signatures and to cross-parameter-referencing signatures.
