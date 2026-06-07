# Constructors as first-class function values

A type's constructor is reachable as a `KObject::KFunction`, so it binds wherever a
function value does.

**Problem.** A bare type name in a value position resolves to a `KObject::KTypeValue` (a Type
value) via [`resolve_type_leaf_carrier`](../../src/machine/execute/dispatch/resolve_type_expr.rs),
never a callable function value. Construction only works as a verb-led call expression routed
through the [`type_call`](../../src/machine/execute/dispatch/single_poll.rs) fast lane into
[`newtype_construct`](../../src/builtins/newtype_def.rs) — the constructor itself can't be
passed as a higher-order argument, stored in a `LET`, or dropped into an `:(FN …)`-typed slot.
A combinator like `MAP` over a list of records has no way to name "the `Point` constructor" as
the function it applies.

**Acceptance criteria.**

- A type reference reaches a `KObject::KFunction` typed `:(FN (fields…) -> <Type>)` whose body
  constructs the type — its parameter record is the repr's field record for a record-repr
  newtype, or a single positional slot for a scalar newtype.
- That constructor function binds wherever a function value does — a higher-order argument, an
  `FN`-typed slot, a `LET`.
- The reification is uniform across record-repr newtypes (ex-structs), scalar newtypes, and
  (once landed) tagged-union variants — one mechanism over `NominalKind::Newtype`, not a
  per-kind path.

**Directions.**

- *Reification trigger — open.* When a type carrier becomes a constructor function rather than
  a Type value. Options: (a) implicit by position — a `SetRef`-identity carrier bound into an
  `:(FN …)`-typed slot (or otherwise used where a function is expected) reifies; (b) an explicit
  surface form (`(<Type> CONSTRUCTOR)` or similar) that names the constructor. Recommended:
  prototype (a) at the function-value bind seam, since it needs no new surface keyword.
- *Synthesized signature — open.* Whether the constructor `KFunction` is a native builtin minted
  per nominal type or a shared dispatch shim parameterized by the member identity. Recommended:
  defer until the trigger lands — the signature shape (`(fields…) -> Type`) is fixed either way.

## Dependencies

**Requires:**

- [Collapse `STRUCT` into a record-repr `NEWTYPE`](struct-newtype-collapse.md) — supplies the
  single `NominalKind::Newtype` construction path a constructor function reifies over, instead of
  a per-kind struct/newtype split.

**Unblocks:** none tracked yet.
