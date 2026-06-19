# Structural value equality

Compare values by structure, not by their rendered strings.

**Problem.** Value equality is string-based. [`Parseable::equal`](../../src/machine/model/types/ktraits.rs)
is implemented as `self.summarize() == other.summarize()` — rendering both operands to
a `String` and comparing the strings — for
[`KObject`](../../src/machine/model/values/kobject.rs),
[`KKey`](../../src/machine/model/values/kkey.rs), and
[`KExpression`](../../src/machine/model/ast.rs). `summarize()` is the same renderer
`PRINT` uses. The `PartialEq` / `Eq` / `Hash` impls on `dyn Serializable`
([`ktraits.rs`](../../src/machine/model/types/ktraits.rs)) — the dict-key path —
delegate to it, so dict-key identity is string-based too. There is no per-variant
structural comparison and no derived `PartialEq` / `Eq` on `KObject` / `KType`.

Comparing rendered strings is wrong wherever the rendered form loses or distorts
identity:

- *Numbers.* `NaN` renders equal to `NaN` (should be unequal); `-0.0` / `0.0` and
  formatting collisions conflate distinct values.
- *Nominal identity.* Two different newtypes with identical reprs render alike — the
  `Wrapped { type_id }` identity is dropped.
- *Records.* Records are order-blind by spec, but `summarize()` renders fields in order,
  so reordered-field records compare unequal.
- *Parameterization.* `Tagged` `type_args` and container element types are erased by
  rendering.
- *Functions / expressions.* Compared by syntax, not identity.

**Acceptance criteria.**

- Value equality is a per-variant structural comparison over `KObject` (and dict keys),
  not a comparison of rendered strings; the `summarize`-based `equal` path is gone for
  value equality.
- The enumerated cases are correct: `NaN ≠ NaN`; distinct newtypes with equal reprs are
  unequal; records compare equal under field reordering; container and `Tagged` type
  parameters participate; function/expression equality is defined deliberately (by
  identity, not syntax).
- Dict-key equality and hashing use the structural comparison and stay consistent
  (equal keys hash equal), respecting the record order-blind rule.
- Equality and the renderer terminate on a cyclic value, if and when one is
  constructible.

**Directions.**

- *Derive vs dedicated walk — open.* Either implement `PartialEq` / `Eq` structurally
  on `KObject` / `KType` (custom where needed) or write a dedicated `value_eq` walk.
  Recommended: a dedicated walk, so float/NaN handling, record order-blindness, and
  nominal identity are explicit rather than fighting a derive.
- *Nominal identity source — open.* Newtype/tagged identity compares via `type_id` /
  set pointer (`Rc::ptr_eq`) today; the `Copy` digest from
  [Content-addressed type identity](type-identity-registry.md) would give a cheaper
  comparison. Coordinate so equality does not bake in `Rc::ptr_eq` if the digest lands.
- *Hash consistency — decided.* The dict-key `Hash` impl is rewritten alongside `equal`
  so structural equality and hashing agree — no rendered-string hashing.

## Dependencies

An engine-internal value-semantics item. It is the comparison side of
[Constructing circular values](../type_language/circular-value-construction.md) (a
cyclic value must not hang equality) and is simplified by
[Content-addressed type identity](type-identity-registry.md) (a `Copy` nominal
identity). Update [design/execution/README.md](../../design/execution/README.md) if a
documented value-equality semantics lands.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
