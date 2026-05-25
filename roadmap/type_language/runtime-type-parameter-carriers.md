# Generic value-slot binding via the destructuring unifier

The runtime type-parameter carriers are in place; the remaining slice is
per-call binding of free type-parameter names that appear in an FN's
*value-slot* signature.

**Problem.** Parameterized values now carry their type arguments
([ktype.md § Runtime type-parameter carriers](../../design/typing/ktype.md#runtime-type-parameter-carriers)),
and a `:(List T)` / `:(Result T E)` slot admits a value structurally via the
[`matches_value`](../../src/machine/model/types/ktype_predicates.rs)
`ConstructorApply` / container arms. But a free type-parameter *name* in a
value-slot signature has nowhere to bind: `FN head (xs :(List T)) -> :T = ...`
can't be defined, because `T` in the deferred return resolves through scope and
nothing populates that scope from the argument the call actually carried. The
generic-destructuring unifier
([`unify_slot`](../../src/machine/model/types/unify.rs)) — which walks a slot's
surface `TypeExpr` against a value's carried `KType` and collects
`(param_name, concrete)` bindings — exists and is unit-tested, but is not wired
into [`invoke.rs`](../../src/machine/core/kfunction/invoke.rs): the re-export is
`#[allow(unused_imports)]` at the crate level.

**Impact.**

- *Generic value-slot functions become definable.* `FN head (xs :(List T)) ->
  :T` and `FN unwrap (r :(Result T E)) -> :T` type-check per call against the
  argument the caller actually passed.
- *Type-parameter dispatch on value slots.* An overload whose value slot binds
  a parameter resolves its deferred return against the bound concrete type, so
  the per-call return matches the argument's instantiation.

**Directions.**

- *Deferral trigger — open.* Defer an FN parameter slot when it carries a free
  type-parameter name (a leaf that's a signature type-parameter, not a
  scope-resolved type). Alternatives: scan the surface `TypeExpr` for
  parameter-set membership at definition time, vs. defer every parameterized
  slot and let `unify_slot` no-op on the concrete-leaf case. Recommended: the
  definition-time scan, so concrete-leaf slots keep their dispatch-time check.
- *Per-call binding site — decided.* Call `unify_slot` at the invoke
  dual-write site, registering each returned `(param_name, concrete)` into the
  per-call child scope via `Scope::register_type` before the deferred return
  elaborates — the seam the unifier was built against.
- *Type-parameter representation — decided.* No `KType::TypeParam` variant; type
  parameters stay ordinary scope-resolved names, and the unifier identifies a
  leaf as a parameter by membership in the caller-supplied `params` set. (Shipped
  in `unify.rs`; restated here because the deferral trigger above leans on it.)

## Dependencies

**Requires:**

None — the runtime carriers, the `matches_value` admission arms, ascription
stamping, and the `unify_slot` core are all shipped; this item wires the
existing unifier into the invoke path.

**Unblocks:**

None as a hard prerequisite. `List` / `Dict` element-type dispatch and the
builtin `Result` parameterized type
([error-handling](../../design/error-handling.md)) already function on the
shipped carriers; this item extends them to generic value-slot signatures.
