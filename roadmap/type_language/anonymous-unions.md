# Anonymous structural unions

Untagged disjunction types — `:(Number | Str | Bool)` — as a first-class
type and value, distinct from today's nominal tagged `UNION`.

**Problem.** koan has only nominal tagged unions: `UNION Name = (tag :Type…)`
declares a tagged-union nominal (a `RecursiveSet` member of `NominalKind::Tagged`) whose values carry a tag
discriminant (`src/builtins/union.rs`). There is no untagged disjunction
`KType` variant (`src/machine/model/types/ktype.rs`), and the `:(...)` type
language has no union form. A function or MATCH / TRY that legitimately
produces "a Number or a Str" must either declare a nominal tagged union and
construct a tagged value in every arm, or coarsen the slot to `Any`.

**Impact.**

- *An anonymous structural union type `:(A | B | C)`.* An order-blind,
  idempotent member set; a slot typed `:(A | B)` admits any value whose
  type is one of the members. The union is the join of its members — each
  member is a subtype of the union.
- *Ergonomic agreed return types.* The agreed `T` of an FN or a
  [MATCH / TRY arm](../../design/execution-model.md#arms-as-own-blocks) can be
  `:(A | B)` without a nominal declaration.
- *A union value is eliminated by ordinary type-dispatch.* Passing one to a
  type-dispatched function selects the matching arm by the value's runtime
  type, so consuming a union needs no new MATCH mode.
- *A dedicated union-value constructor.* Construction is its own builtin,
  decoupled from MATCH.

**Directions.**

- *New KType variant — decided.* An untagged disjunction `KType` variant,
  distinct from a `NominalKind::Tagged` nominal. Member set order-blind and
  idempotent (`A | A = A`); admissibility is set-based (a `:(A | B)` slot
  admits A-typed and B-typed values).
- *Construction — decided.* A dedicated union-value constructor builtin.
  MATCH is not modified to auto-wrap arm results — its arms agree on a declared
  return type instead (see
  [execution-model.md § Arms as own blocks](../../design/execution-model.md#arms-as-own-blocks)).
- *Surface `|` — open; rides n-ary operators.* The `:(A | B | C)` infix
  surface rides the dispatched-operator machinery from
  [user-definable n-ary operators](../operator_chaining/n-ary-operators.md): `|`
  desugars (the parse→dispatch bridge) to a dispatched, associative-flattening
  union builtin, so arbitrary arity falls out of associativity rather than new
  parse arity. Precedence inside `:(...)` (e.g. `List A | B`) is settled there.
- *Elimination — decided (dispatch); type-MATCH deferred.* A union is
  consumed via ordinary type-dispatch. A tag-free "match by type" arm shape
  in [`branch_walk`](../../src/builtins/branch_walk.rs) is optional sugar over
  that mechanism and is deferred.

## Dependencies

**Requires:**

- [User-definable n-ary operators](../operator_chaining/n-ary-operators.md) — the `|`
  chaining surface rides its dispatched-operator machinery.

**Unblocks:**


The underlying type and constructor builtin could be prototyped against a
variadic type-constructor overload (the `RECORD` / nominal-`UNION` path)
before the `|` surface lands. This item lets the shipped MATCH / TRY / FN
declared return type be spelled as a structural union `:(A | B)`.
