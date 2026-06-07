# Type values as data carriers

A user type used as a value rides a `KObject` data carrier, classified by one kind
lattice — collapsing the type-channel/object-channel fork in value matching.

**Problem.** A user type used as a value flows through the value channel's dedicated
`Carried::Type(&KType)` arm
([carried.rs:17](../../src/machine/model/values/carried.rs)), not a `KObject` data
carrier — a type-value and an instance-value of that type travel different channels.
This forks every value-matching predicate into a type-vs-object split, and forces two
parallel "match a value by its kind" mechanisms:

- [`KKind`](../../src/machine/model/types/kkind.rs) `{Proper, Module, Signature, Any}`
  classifies a *type-value* (`kind_of`), matched by the `OfKind(KKind)` wildcard.
- [`NominalKind`](../../src/machine/model/types/recursive_set.rs) `{Tagged, Newtype,
  TypeConstructor}` classifies a user *carrier*, matched by the `AnyUserType{NominalKind}`
  wildcard.

`OfKind` and `AnyUserType` sit side by side in both
[`matches_value`](../../src/machine/model/types/ktype_predicates.rs) (the `OfKind` arm at
the type-value check, the `AnyUserType` arm at the carrier check) and
[`accepts_part`](../../src/machine/model/types/ktype_predicates.rs) — two arms of one
predicate keyed on a value's kind, split only by which channel the value arrived through.
The two enums and their two `KType` wildcards duplicate one classification job. The fork
is already drifting: `AnyUserType` matches only `Tagged → KObject::Tagged` and
`Newtype → KObject::Wrapped` in both predicates; `NominalKind::TypeConstructor` is
unhandled, so the third family falls through the hand-enumerated match list.

**Acceptance criteria.**

- A user type used as a value is carried as a `KObject` data carrier; its kind is read
  from `KObject::ktype()`, the same path an instance value uses.
- A single kind-matched slot admits a value by its `ktype()`; `OfKind` and `AnyUserType`
  are one `KType` variant, not two.
- `KKind` and `NominalKind` form one kind lattice; every nominal family — including
  `TypeConstructor` — is matched by construction rather than a hand-enumerated family list.
- `matches_value`, `accepts_part`, and `matches_type` carry no type-channel-vs-object-channel
  branch for user type values.

**Directions.**

- *Carrier representation — open.* How a reified type rides a `KObject`. Options: (a) reuse
  `KObject::Wrapped` with a sentinel `type_id` marking "this carrier is a type"; (b) a
  dedicated `KObject` variant for a reified type. Recommended: prototype (b) — a distinct
  variant keeps `ktype()` total without overloading `Wrapped`'s repr semantics.
- *`Carried::Type` scope — open.* Whether module / signature / primitive type tokens also
  leave the `Carried::Type` arm in this item, or only user nominals move first and the arm
  retires in a follow-up. Recommended: move user nominals first; the arm's other tenants are
  separable.
- *Kind-lattice merge shape — open.* Whether `NominalKind` folds into an enlarged `KKind`,
  or the two become axes of one kind value (`(meta, family)`). Decision waits on the carrier
  representation, since the matched discriminant is whatever `ktype()` reports.

## Dependencies

Shares the "what is a type when used as a value" seam with
[constructors as first-class function values](constructor-as-first-class-function.md): that
item reifies a type *reference* to a callable, this one reshapes the type *value* carrier.
Independent — neither blocks the other — but a shared carrier representation would serve both.

**Requires:** none — foundation.

**Unblocks:** [consolidate identified code duplication](../refactor/consolidate-identified-duplication.md) — dissolving the carrier fork lets the scheduler `Object`/`Type` finalize arms collapse to one.
