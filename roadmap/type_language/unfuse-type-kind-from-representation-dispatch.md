# Unfuse type-kind classification from representation dispatch

One `kind_of` lattice classifies types; runtime-representation dispatch becomes its
own predicate. `AnyUserType` — which today does both — dissolves into the two.

**Problem.** koan has two enums answering "what kind of type is this," consulted on two
paths that never meet:

- [`KKind`](../../src/machine/model/types/kkind.rs) `{Proper, Module, Signature, Any}`
  classifies a *type value* via [`kind_of`](../../src/machine/model/types/ktype.rs), matched
  by the `OfKind(KKind)` slot. `kind_of` bottoms out at `Proper` for every nominal — its
  `_ => Proper` arm cannot see the nominal family.
- [`NominalKind`](../../src/machine/model/types/recursive_set.rs) `{Tagged, Newtype,
  TypeConstructor}` recovers the family `kind_of` discarded — read off
  `set.member(index).kind` and matched only by the `AnyUserType{NominalKind}` slot.

`NominalKind` exists solely to un-truncate that `_ => Proper` arm: one classification job,
split across two enums and two `KType` wildcards.

`AnyUserType` then does two *unrelated* jobs under one name:

- *Type-kind classification.* Naming a nominal family as a type kind — e.g. the return
  contract `AnyUserType{Tagged}` on [`CATCH`](../../src/builtins/catch.rs) ("produces some
  tagged union").
- *Runtime-representation dispatch.* Selecting a runtime value by its `KObject` shape —
  [`ATTR <s:Newtype>`](../../src/builtins/attr.rs) matches every `KObject::Wrapped` so
  `access_field` can reach through the wrapper. The slot's comment claims it keys on
  `NominalKind` "never the repr," but catching every `Wrapped` *is* a representation match.

Both jobs run by hand-enumerating `KObject` variants — `(Tagged → KObject::Tagged) |
(Newtype → KObject::Wrapped)` in both
[`matches_value` and `accepts_part`](../../src/machine/model/types/ktype_predicates.rs).
`TypeConstructor` has no arm, so the third family drifts out of every match.

**Acceptance criteria.**

- Type kinds form one subsumption lattice — `Any > {Module, Signature, Proper > {Tagged,
  Newtype, TypeConstructor}}`. The separate `NominalKind` enum is gone.
- `kind_of` is the sole type→kind classifier; it descends `Variant` / `SetRef` /
  `ConstructorApply` to report the nominal family, never collapsing a nominal to `Proper`.
- A type-accepting slot is one variant, `OfKind(KKind)`; `AnyUserType` is gone. Specificity
  follows the subsumption tree — `OfKind(Tagged)` out-specifies `OfKind(Proper)`.
- Type-kind classification reads only a `KType` — a value's `ktype()`, or a type value
  itself — never a `KObject` representation.
- A builtin that dispatches on runtime representation (`ATTR`'s newtype field access) selects
  its argument through a representation predicate distinct from the type-kind lattice.
- `CATCH`'s return type is determined without a nominal-family wildcard.
- `Carried::Type` remains the type channel; no reified type rides a `KObject`.

**Directions.**

- *Carrier — decided.* `Carried::Type` stays; a type value rides the type channel, not a
  `KObject`. A type value and an instance are different things, and the channel is what
  selects whether a kind question means "the meta-kind of this type value" (type channel) or
  "the family of this value's type" (object channel) — the same `KType` answers each role
  differently, so the channels must stay distinct.
- *Kind-lattice shape — decided.* `NominalKind` folds in *under* `Proper` as a subsumption
  tree, not a flat enum; `kind_of` descends nominals to report the family it currently
  discards.
- *Representation predicate — open.* How `ATTR`'s newtype field access selects "any
  `Wrapped` value" once the type-kind lattice no longer serves that role. Options: (a) a
  dedicated representation-shape predicate matched against `KObject`; (b) dispatch the field
  access through the value's concrete type. Recommended: (a) — keeps representation dispatch
  honest and out of the type lattice.
- *`CATCH` return determination — open.* How `CATCH` names its result once `AnyUserType{Tagged}`
  is unavailable. Options: explicit return-type syntax on the call; inference from the caught
  expression's type.

## Dependencies

Shares the "what is a type when used as a value" seam with
[constructors as first-class function values](constructor-as-first-class-function.md) — both
reason about a type used as a value — but neither blocks the other.

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
