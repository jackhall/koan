# Bounded type variables

A bound written on a `FOR ALL` parameter or a signature's `TYPE` member.

**Problem.** The lattice carries a bound on both kinds of type variable —
`Quantified` and `AbstractType` ([node.rs](../../src/type_lattice/node.rs)) —
the unifier checks a solution against it
([`Collector::solve`](../../src/type_lattice/unify.rs)), and the signature
relation checks a member's binding against it
([sig_relations.rs](../../src/type_lattice/sig_relations.rs)). No koan program
can write one: the elaborator bounds every `FOR ALL` name by `Any`
([expression.rs](../../src/elaborate/expression.rs)) and every `TYPE` member by
`Any` ([declaration.rs](../../src/elaborate/declaration.rs)). So a generic
function cannot keep types or code out of its type parameter, and a value
sealed behind a signature's type member satisfies no family top. The lattice's
comments say a type variable "stands over" its bound, which reads as the
variable lying above it. Two lattice rules are wrong for a bound other than
`Any`, though no bound reaches them today: the order compares a union on either
side member by member before it reads a variable's bound
([`pairing`](../../src/type_lattice/walk/binary.rs)), so a variable bounded by
`Value | Type` lies under neither its bound nor any union above it; and
[`admits_part`](../../src/values/admission.rs) lets a quantified slot take every
raw part, whatever its bound.

**Acceptance criteria.**

- `FOR ALL ((Elt UNDER Value) Key)` bounds `Elt` by `Value` and `Key` by `Any`,
  in a lambda, an `EXPR` head, and the `FN` and `EXPR` types alike.
- `TYPE (Elt UNDER Value)` bounds a signature member by `Value`, and an opaque
  ascription's fresh type for that member carries the same bound.
- A name written without a bound is bounded by `Any`.
- A call whose arguments solve a bounded parameter to a type not under its
  bound is refused, as a group with no solution is.
- A module that binds a bounded member to a type not under its bound does not
  satisfy the signature.
- A value sealed behind a member bounded by `Value` satisfies `Value`.
- A bound that names a `FOR ALL` name or an abstract member is refused where it
  is elaborated.
- A type variable lies under its bound and under every type above its bound,
  a union included: a variable bounded by `Number | Str` lies under
  `Number | Str` and under `Number | Str | Bool`.
- A quantified slot takes a raw part only when its bound takes it: a slot
  bounded by `Value` refuses a quote.
- The lattice's source comments say a type variable is bounded by its bound.
- The tutorial shows a bounded `FOR ALL` name and a bounded `TYPE` member.

**Directions.**

- *Spelling — decided.* `UNDER`, per name, grouped as `TYPE (Elem AS Wrap)` is.
  It is the lattice's own word for a variable's relation to its bound, and
  `MATCH … UNDER` ([control expression shapes and errors](control-and-errors.md))
  claims the same relation of a scrutinee. `<:` would add a symbol token beside
  the `<` operator and the glued `:` sigil, and `Elt :Value` would read, as `:`
  does on every type name, as the name's kind.
- *Closed bounds — decided.* A bound names no type variable, since the order
  assumes nothing above a bound is itself a variable
  ([order.rs](../../src/type_lattice/order.rs)).
- *A bound on a higher-kinded member — open.* `TYPE ((Elem AS Wrap) UNDER …)`
  would bound what the constructor's applications produce, or be refused.
  Recommended: refuse it until a program needs one.

## Dependencies

**Requires:** none.
