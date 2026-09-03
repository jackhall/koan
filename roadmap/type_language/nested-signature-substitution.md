# Substitution through nested signatures

A signature nested inside another signature's slot type is opaque to
abstract-member substitution, so a slot declared over the outer signature's
members can never be satisfied.

**Problem.** A SIG value slot may name a signature *inside* a compound type —
`VAL subs :(LIST OF (Inner WITH {Item = Elt}))` elaborates, because
`LIST OF` takes a slot of any kind. Pinning the inner signature's `Item` to the
outer signature's abstract member `Elt` is the shape that makes such a slot
worth writing: it says "a list of `Inner`s over *my* element type". No module
can satisfy it. Ascribing one reports

```
member `subs` has type `:(LIST OF SIG (Item: Carrier, one: Carrier))`
but the signature declares `:(LIST OF SIG (Item: Elt, one: Elt))`
```

— the declared `Elt` is never substituted with the module's `Carrier`, so the
structural compare fails on a pair that should have matched.

Three walks in [`sig_schema.rs`](../../src/machine/model/types/sig_schema.rs)
recurse through `List` / `Dict` / `Record` / `KFunction` / `Union` /
`ConstructorApply` and bottom out on everything else, `Signature` included:

- [`substitute_sig_members`](../../src/machine/model/types/sig_schema.rs)
  returns a nested signature unchanged, so the reference to `Elt` inside it
  survives substitution.
- `references_sig_member` reports `false` for it, so `slot_satisfied_by` takes
  its no-substitution fast path and compares the declared and supplied types
  structurally — the compare the error above shows failing.
- `canonicalize_binder` leaves the nested reference sourced at the *declaring*
  scope instead of rewriting it to `ScopeId::SENTINEL`, so two textually
  identical `SIG` declarations carrying one do not project to the same schema
  and do not intern to one type.

The barrier is not unsound today — the coercion walk's own
`Signature` arm is the same pass-through, so
[ascription](../../src/builtins/ascribe.rs) never builds a view whose nested
member reads at the wrong types; it fails the satisfaction check first. The
whole shape is simply unusable.

The prior art is ML's. OCaml admits a submodule specification as an ordinary
signature component (`module Sub : sig … end`), and makes **both** relations
structurally recursive through it: a `with type elt = int` constraint rewrites
every occurrence of the path, nested components included, and signature
matching descends into each `module Sub : T` component and re-runs itself on
the submodule. Nothing there is special-cased for nesting — the nested
signature is just another term the same two walks traverse.

**Acceptance criteria.**

- `substitute_sig_members` rewrites references to the enclosing signature's
  abstract members inside a nested `Signature` node — in its manifest members
  and its value slots alike.
- Substitution is capture-avoiding: a nested signature that declares its own
  member of the same name shadows the outer one, and that member's references
  are left alone.
- `references_sig_member` reports a nested reference, so `slot_satisfied_by`
  takes the substituting path rather than the structural fast path.
- A module whose member is a nested-signature-typed slot satisfies the SIG that
  declares it: the `VAL subs :(LIST OF (Inner WITH {Item = Elt}))` shape above
  ascribes, and a module supplying an `Inner` over some *other* type is
  rejected.
- `canonicalize_binder` re-sources a nested reference to `ScopeId::SENTINEL`,
  so two textually identical SIG declarations carrying a nested signature
  project to one schema and intern to one type.
- A member filling a nested-signature-typed slot reads through an opaque view
  at the view's types, on the same terms every other slot shape does
  ([design/typing/modules.md](../../design/typing/modules.md)) — the
  [`coerce_object_into`](../../src/machine/model/values/coerce.rs) walk grows
  the matching arm.

**Directions.**

- *Shadowing rule — decided.* A nested signature's own abstract members shadow
  the enclosing signature's by name. Koan canonicalizes every SIG's members to
  `ScopeId::SENTINEL`, so `source` cannot tell an inner binder from an outer
  one and the recursion must subtract the nested schema's own
  `abstract_members` keys from the substitution before descending. OCaml gets
  this from scoping; koan has to spell it.
- *Coercion arm — open.* A nested-signature slot's member is a `KObject::Module`
  whose own members would need coercing, which is a module-level rewrite the
  value walk has no arm for. Options: (a) rebuild the nested module as a view
  of itself under the outer view's mints — reusing
  [`alloc_module_view`](../../src/machine/core/scope/registry.rs), which is
  already the "born holding coerced members" door; (b) leave nested modules
  representation-transparent and say so, as the keyworded surface does
  ([sig-keyworded-surface.md](sig-keyworded-surface.md)). Recommended: (a) —
  the door exists, and (b) reopens the leak this slot shape is for.
- *Top-level module-typed slots — open.* `VAL sub :Inner` does not elaborate at
  all: `VAL`'s type slot is `KKind::ProperType` and a signature is
  `KKind::Signature`, so a module-typed member is reachable only nested inside a
  container. Whether to admit a signature directly in a `VAL` slot is a separate
  surface decision this item does not settle.

## Dependencies

**Requires:** none — the substitution and satisfaction walks this extends have
shipped.

**Unblocks:** none tracked yet.
