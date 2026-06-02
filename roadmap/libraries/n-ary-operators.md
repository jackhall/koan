# User-definable n-ary operators

Operators that users declare — naturally in modules — chain to arbitrary
arity, and whose meaning resolves through dispatch rather than a fixed
parse-time builder.

**Problem.** [`operators.rs`](../../src/parse/operators.rs) is a flat
compile-time table of single-character triggers (`!`, `.`, `?`), each with a
fixed unary or binary builder that desugars the trigger into a synthetic
keyword expression. A user cannot declare a new operator; a run of the same
operator cannot fold into one n-ary form; and arity is fixed at the builder.
The desugar is already a parse→dispatch bridge — `?` lowers to the dispatched
`TRY` builtin — but the registry itself is closed and compile-time, so the
meaning of each operator is wired at the builder rather than resolved against
operand types.

**Impact.**

- *Users declare their own operators.* Naturally module-scoped, alongside the
  types and functions the operator acts on.
- *Operators chain to arbitrary arity.* A run of one operator folds into a
  single form.
- *An operator's meaning resolves through dispatch.* The operand types decide
  which implementation runs, extending the existing `?`→`TRY` parse→dispatch
  bridge to user code.
- *Substrate for paired and disjunction operators.* The `|` surface of
  [anonymous structural unions](../type_language/anonymous-unions.md) and a
  cleaner home for [group-based operators](group-based-operators.md) both ride
  this machinery.

**Directions.**

- *Meaning via dispatch — decided.* An operator desugars to a keyword-headed
  form resolved by dispatch, not fixed at the parse builder. How much of the
  trigger→keyword mapping stays in `operators.rs` versus a richer registry is
  open.
- *Arity — open.* n-ary as a parse-time variadic collector (fold a run of one
  trigger into a single call) versus binary parse plus an associative-flattening
  builtin (arity falls out of associativity). Recommended: the
  associative-flattening builtin where the operator is associative, since it
  needs no new parse arity.
- *User-definable registration — open.* A runtime registry with slot allocation
  deferred to dispatch (the user-definable path) versus a compile-time table
  (rigid). The module-scoped declaration surface is open. The user-definable
  requirement forces the runtime path.
- *Precedence — open.* The flat table has no precedence tiers; user-defined
  operators need a precedence story.
- *Relationship to group-based operators — open.*
  [Group-based operators](group-based-operators.md) is one client of this
  machinery; whether groups land on top of it or independently is that item's
  call (its syntax-level shorthand variant can ship against today's flat
  registry).

This item needs substantial design work and is deferred until its design is
settled.

## Dependencies

**Requires:** none yet — foundation.

**Unblocks:** [Anonymous structural unions](../type_language/anonymous-unions.md)
— the `|` chaining surface rides this machinery.

Shares the dispatched-operator mechanism with
[group-based operators](group-based-operators.md), but neither hard-blocks the
other: the group shorthand variant can ship against the existing flat registry.
