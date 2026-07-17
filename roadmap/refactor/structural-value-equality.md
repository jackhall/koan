# Structural value equality

Compare values by structure, not by their rendered strings — and give the language
`==` / `!=`.

**Problem.** Value equality is string-based and the language has no equality
operator (`< <= > >=` are the only comparisons).
[`Parseable::equal`](../../src/machine/model/types/ktraits.rs) is implemented as
`self.summarize() == other.summarize()` — rendering both operands and comparing the
strings — for [`KObject`](../../src/machine/model/values/kobject.rs),
[`KKey`](../../src/machine/model/values/kkey.rs), and
[`KExpression`](../../src/machine/model/ast.rs). Its one live runtime consumer is
the dict-key path: the `PartialEq` impl on `dyn Serializable`
([`ktraits.rs`](../../src/machine/model/types/ktraits.rs)) delegates to it, and the
sole implementor, `KKey`, compares rendered strings while hashing `f64::to_bits` —
so two NaN keys with different bit patterns are equal-but-hash-unequal, violating
the map contract. For `KObject`/`KExpression` the string compare is latent but
wrong wherever rendering loses identity: `NaN` renders equal to `NaN`; distinct
newtypes with identical representations render alike; records are order-blind by
spec but render in declaration order; `Tagged` `type_args` and container element
types are erased; expressions compare by rendered syntax even when spliced values
differ.

Separately, `KType`'s `PartialEq` is same-lifetime only
([`impl<'a> PartialEq for KType<'a>`](../../src/machine/model/types/ktype.rs))
even though the digest compare it performs is lifetime-independent, and the
cross-lifetime step dispatch needs is discharged by an `unsafe` transmute in
[`KType::accepts_resolved`](../../src/machine/model/types/ktype_predicates.rs)
re-anchoring an entire `Carried` so the same-lifetime predicate suite can run.

**Acceptance criteria.**

- `==` and `!=` builtins exist over `(left :Any) op (right :Any) -> Bool`,
  **binary-only**: they belong to no operator group, and a chain
  (`a == b == c`, or mixed with `<`) is a structured error.
- Value equality is a per-variant structural walk over `KObject`; no rendered-string
  equality remains anywhere (`Parseable::equal` is gone; the deferred-return
  duplicate-overload check compares expressions structurally).
- Numbers compare IEEE: `NaN != NaN`, `-0.0 == 0.0`.
- Nominal identity participates: distinct newtypes/abstract types with equal
  representations are unequal, and a `Wrapped` value is unequal to its bare
  representation; identity compares via digest-based `KType` equality.
- Records compare order-blind under field reordering.
- Containers gate on comparability: `List`/`Dict`/`Record`/`Tagged` compare contents
  iff their memoized/ascribed type parameters are related (one `satisfied_by` the
  other, either direction); unrelated parameters compare unequal. The relation is
  deliberately intransitive and the design doc says so.
- Module and function operands (at any depth of the walk) are a structured error,
  not `false`; the module message names `(TYPE OF m)` comparison as the interface
  idiom.
- `KExpression` equality is structural over parts; spliced results compare by the
  value walk.
- Dict keys are the concrete `KKey` (the `Box<dyn Serializable>` slot and the
  `Serializable` trait are deleted); NaN keys are rejected at construction, `-0.0`
  normalizes to `0.0`, and `KKey`'s `PartialEq`/`Hash` agree.
- `KType` structural equality is lifetime-agnostic (`KType<'a>` compares to
  `KType<'b>` directly), the predicate suite takes heterogeneous slot/value
  lifetimes, and the `unsafe` transmute in `KType::accepts_resolved` is deleted.
- Equality terminates on every constructible value (values are acyclic today; the
  cycle-guard obligation transfers to
  [Constructing circular values](../type_language/circular-value-construction.md)
  if value cycles land).

**Directions.**

- *Dedicated walk vs derive — decided.* A dedicated `value_equal` walk returning
  `Result<bool, _>` (banned operands are errors), so IEEE floats, order-blind
  records, nominal identity, and the comparability gate are explicit.
- *Nominal identity source — decided.* The shipped content digest; no `Rc::ptr_eq`
  in equality semantics.
- *Comparability gate — decided.* Type parameters participate via subtype
  relatedness, trading transitivity for ascription-invariance
  (`f(x) == x` across a coarsening boundary) plus empty-container distinction.
- *Module/function equality — decided.* Banned operands; module values are
  generative (fresh mint per evaluation), so the honest comparisons are interface
  content (`TYPE OF`) or nothing. A directional `SATISFIES` operator stays future
  work.
- *Chaining — decided.* Binary-only; equality joins no pairwise-chain group.
- *Hash consistency — decided.* `KKey` is the only hashable domain; its equality and
  hash read the same bits over a NaN-free, zero-normalized key space.

## Dependencies

The comparison side of
[Constructing circular values](../type_language/circular-value-construction.md) (a
cyclic value must not hang equality). Implementation plan:
`scratch/structural-value-equality-plan.md` (untracked).

**Requires:**


**Unblocks:** none tracked yet.
