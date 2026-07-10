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

**Acceptance criteria.**

- An untagged disjunction type is spelled `:(A | B | C)` and resolves to a
  dedicated union `KType` variant, distinct from a `NominalKind::Tagged`
  nominal.
- The member set is order-blind and idempotent: `:(A | B)` and `:(B | A)` are
  the same type, and `:(A | A)` is `:A`.
- A slot typed `:(A | B)` admits any value whose type is `A` or `B`, and each
  member is a subtype of the union.
- The agreed `T` of an FN or a
  [MATCH / TRY arm](../../design/execution/calls-and-values.md#arms-as-own-blocks) can be
  `:(A | B)` with no nominal declaration.
- A union value passed to a type-dispatched function selects the arm matching
  the value's runtime type.
- A dedicated union-value constructor builtin constructs union values,
  separate from MATCH.
- A `|` run (`Number | Str | Bool`) reduces through a unary-mode operator
  group to that constructor, building the whole union in one pass.
- A tagged union is expressible as the anonymous-union join of per-variant
  `Newtype`s (with [tagged-union variants as dispatchable
  types](tagged-variant-types.md)), so `NominalKind::Tagged` dissolves into
  `Newtype` — the sum-side counterpart of the shipped struct → record-repr
  `NEWTYPE` collapse.
- The nominal-finalize paths the dissolution rewrites
  ([newtype_def.rs](../../src/builtins/newtype_def.rs),
  [union.rs](../../src/builtins/union.rs)) cross their seal identities into
  folded placements as declared operands (the `RegionTypeFamily` pattern of
  [constructors.rs](../../src/machine/execute/dispatch/constructors.rs));
  no ambient-lifetime `KType` is captured into a folded placement on the
  finalize path.

**Directions.**

- *New KType variant — decided.* An untagged disjunction `KType` variant,
  distinct from a `NominalKind::Tagged` nominal. Member set order-blind and
  idempotent (`A | A = A`); admissibility is set-based (a `:(A | B)` slot
  admits A-typed and B-typed values).
- *Construction — decided.* A dedicated union-value constructor builtin.
  MATCH is not modified to auto-wrap arm results — its arms agree on a declared
  return type instead (see
  [execution/calls-and-values.md § Arms as own blocks](../../design/execution/calls-and-values.md#arms-as-own-blocks)).
- *Surface `|` — open; rides n-ary operators.* The `:(A | B | C)` infix
  surface rides the dispatched-operator machinery from
  [user-definable n-ary operators](n-ary-operators.md): `|`
  desugars (the parse→dispatch bridge) to a dispatched, associative-flattening
  union builtin, so arbitrary arity falls out of associativity rather than new
  parse arity. Precedence inside `:(...)` (e.g. `List A | B`) is settled there.
- *Elimination — decided (dispatch); MATCH lowering owned here.* A union is
  consumed via ordinary type-dispatch. The tag-free "match by type" arm shape in
  [`branch_walk`](../../src/builtins/branch_walk.rs) is sugar over that mechanism,
  and converging the existing nominal-tagged `MATCH` onto the same lowering —
  removing its parallel name-matching form, the deferred half of [tagged-union
  variants as dispatchable types](tagged-variant-types.md) — is this item's
  responsibility, since both elimination paths collapse onto the one match-by-type
  mechanism this item introduces.

## Dependencies

Soft ordering: the underlying type and constructor builtin can be prototyped against
the nominal-`UNION` path before the `|` surface lands.

**Requires:**

- [User-definable n-ary operators](n-ary-operators.md) — the `|`
  chaining surface rides its dispatched-operator machinery.

**Unblocks:**

- [Fold-closure capture provenance](fold-closure-provenance.md) — the
  dissolution's rewrite of the nominal finalize carries the seal-identity
  operand-crossing.
- [Tagged-union variants as dispatchable types](tagged-variant-types.md) —
  its `MATCH`-lowering criterion completes through the match-by-type
  mechanism owned here.
