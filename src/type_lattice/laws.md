# The laws

What the lattice promises, what rests on each promise, and what breaks without
it. Part of [the type lattice](README.md)'s design.

A broken law is rarely visible where it is broken. Nothing panics. Some later
reader — a union, an overload set, a static verdict — gets an answer that
depends on something it should not, and a program misbehaves far from the
change. Each law below therefore names the readers that rest on it and gives
the smallest case that goes wrong without it. The cases describe a lattice
with the law removed, not this one.

## A value's type is concrete

**The law.** A type is **concrete** or **parametric**. A parametric type holds
an unsolved variable: a `FOR ALL` name, a signature's head parameter, a lexical
variable, or a quantified callable's whole type. Every value but a quantified
callable carries a concrete type, and the order, `join`, `meet` and a union's
canonical shape relate concrete types alone. An unsolved variable is a pair of
ends, `[lower, bound]`, or a solved type. It is never read as its bound.

**What rests on it.** Every law below. The order can be a partial order by
handle only because no variable reaches it.

**What breaks.**

- *A variable read as its bound.* Under
  `EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Type = #(Elt)`, `KIND 1` answers
  `Number`, since the call solves `Elt`. Read `Elt` as its bound and the slot
  is `:Any`, nothing is left to solve, and `KIND 1` answers `Any`. The same
  reading puts `FOR ALL #[Elt] :{x :Elt} -> Null` and `:{x :Any} -> Null`
  under each other, which costs [antisymmetry](#the-order-is-a-partial-order).
- *A parametric type among concrete ones.* The concrete lattice promises more
  than *fits* does: antisymmetry by handle, and an exact `join` and `meet`.
  A scheme or a variable let into one concrete type does not stay there. The
  list that holds it, the union that names it and the record with a field of
  it are each built by the order, so each now holds two handles that fit each
  other, or a join that is not least, and every reader of those inherits the
  weaker guarantee. This is why a program may name a quantified type only as a
  signature member's whole type: a signature is sealed content, a leaf no walk
  enters, so the scheme inside it never reaches the order
  ([quantified types](../../design/quantified-types.md)).
- *A variable in a built type.* A list's type is the `join` of its cells'
  types. `join` reads the order, which has no rule for a variable, so a cell
  typed by one has no place in the result.

**What enforces it.** The typed handles
([identity](identity.md#typed-handles)). Only the lattice wraps a raw handle,
every door is generic over its children's handle, and a `KType` asked of a
parametric child is a compile error, pinned by a `compile_fail` doctest on
`TypeRegistry::list`. `TypeRegistry::concrete` is the one checked conversion.

## The order is a partial order

**The law.** Over concrete types, [`is_subtype_of`](relations.md#the-relations)
is reflexive, transitive and antisymmetric **by handle**: two handles that lie
under each other are one handle. It holds no unification, so no variable is
instantiated to make an answer come out.

**What rests on it.** Everything that is *built*: a canonical union, `join`,
`meet`, a value's type memo, an overload set's subsumption, a verdict's cache
key.

**What breaks without antisymmetry.** Two handles then stand at one place in
the order. A union keeps the first of two members that lie under each other,
so `a | b` and `b | a` are two handles for one type, and every reader that
compares handles disagrees with every reader that asks the order:

- two `MATCH … WITH` guards naming the two spellings are not refused as
  repeated, and the second arm is dead;
- a module whose manifest member is the one spelling is refused by a signature
  that names the other, since a manifest member must be *equal*;
- a list's type depends on which cell came first.

**What breaks without transitivity.** The usual way to lose it is to compare a
concrete type with a parametric one by reading the variable as its upper
bound. Take `Left` and `Right`, each bounded by `Number | Str`. `Left` lies
under `Number | Str`, as the rigid rule says. Read `Right` as its bound and
`Number | Str` lies under `Right`. Yet `Left` does not lie under `Right`: they
are two variables, and a call may solve one to `Number` and the other to
`Str`. The approximation is unsound on its own account too, since it puts
`Str` under `Right` where a call binds `Right` to `Number`.

- A union drops each member that lies under another. `Left | Number | Str |
  Right` drops `Number` and `Str` under the approximated `Right` and keeps
  `Left | Right`, which admits neither a number nor a string to any reader
  holding the rigid rule. So [a union holding a variable keeps
  it](relations.md#the-relations), and reduces its concrete members among
  themselves alone.
- Ranking eliminates a candidate another strictly beats. Three candidates that
  beat each other in a cycle eliminate each other, and no candidate survives a
  call that all three admit.

**What enforces it.** `the_order_is_reflexive_and_bounded`,
`the_order_is_antisymmetric` and `the_order_is_transitive` in
[tests/properties.rs](tests/properties.rs), over generated concrete types; the
`KType` parameters of `is_subtype_of`; and the one descent in `order.rs`, so no
second walk can drift from it.

## One type, one handle

**The law.** A type's handle is its content digest, and equal types have equal
handles: comparing two types is comparing two integers, with no structural
fallback. So every canonical shape is part of identity — a union is flat,
deduplicated and holds no member under another; a record's digest ignores field
order; a quantifier group is numbered by position, so two alpha-variants are
one node; a union of the three family tops is `Any`.

**What rests on it.** Handle equality stands for type equality everywhere: the
verdict table's key, a repeated-guard check, a manifest member, a pinned
application, an overload set's deduplication, and antisymmetry itself.

**What breaks.** A door that interns a non-canonical node makes a second handle
for a type that already has one.

- `Value | Type | Code` left uncollapsed is a second top beside `Any`. It lies
  under `Any` and misses only what lies under no family top, such as a
  variable bounded by `Any`. A slot typed by it admits nearly what `:Any`
  admits and is a different type to every handle comparison.
- A union interned with a member under another, `Number | Number | Str`, is
  unequal to `Number | Str` and lies both under and over it: antisymmetry
  fails with no variable in sight.
- A group numbered by declaration order instead of first occurrence makes
  `FOR ALL #[Left Right] :{x :Right, y :Left}` and its renaming two types,
  which fit each other and share no handle.

**What enforces it.** Every composite is built through the registry's doors,
which canonicalize before they digest. `equal_content_interns_once`,
`union_of_is_canonical`, `a_union_holding_the_three_family_tops_is_any` and the
two fixed-point laws over shapes and function types state it;
[tests/golden.rs](tests/golden.rs) pins the builtin vocabulary's digests, so an
identity move shows in a diff.

## `join` and `meet` are exact

**The law.** `join` is the **least** upper bound and `meet` the **greatest**
lower bound. Both are commutative, associative and idempotent, each absorbs
the other, and `a ≤ b` exactly when `join(a, b)` is `b` and `meet(a, b)` is
`a` — all by handle. `Never` and `Any` are their identities.

**What rests on it.** `join` types every container: a list's type is the join
of its cells' types, and nothing later descends into a value to type it again.
A variable's least instance is the join of its lower contributions. `meet` is
the solve's upper end, the `&` a program writes, a retype's target, and the
test behind every *never* verdict and every refused return or ascription.

**What breaks.**

- *A `join` that is not an upper bound.* A list's memo then fails to cover one
  of its cells, and a slot `:(LIST OF Number)` admits a list holding a string,
  since admission reads the memo and never the cells.
- *A `join` that is not least.* A structural join would answer `LIST OF
  (Number | Str)` for `LIST OF Number` and `LIST OF Str`. That type also admits
  a list mixing numbers and strings, which neither operand does. The canonical
  union `(LIST OF Number) | (LIST OF Str)` lies strictly under it.
- *A `join` that is not associative.* The same structural join gives, over
  `LIST OF Number`, `LIST OF Str` and `Str`, either
  `(LIST OF (Number | Str)) | Str` or `(LIST OF Number) | (LIST OF Str) | Str`,
  by the order the three are folded in. A list literal's type then depends on
  the order of its elements, and so does every dispatch over it.
- *A `meet` that is too low.* Two types with a common refinement that meet at
  `Never` make a *never* verdict out of a call that would run, and refuse the
  load of a body whose result does satisfy its declared return.
- *A `meet` that is too high.* A list holding a
  `FN :{x :(Number | Str)} -> Null` and a `FN :{x :(Number | Bool)} -> Null`
  binds the `Elt` of `LIST OF (FN :{x :Elt} -> Null)` to `Number`, the meet of
  the two parameter types. A meet answering `Number | Str` lets the body pass
  a string to the second function.

**What enforces it.** `join_and_meet_are_commutative_and_idempotent`,
`join_and_meet_absorb_each_other`, `join_and_meet_are_associative`,
`never_and_any_are_the_identities` and `the_order_agrees_with_join_and_meet`.
`join` is the larger operand or the canonical union and never a walk, which is
what makes it associative.

## *Fits* is a preorder, and nothing is built from it

**The law.** [*Fits*](relations.md#the-relations) is reflexive and transitive,
and contains the order over concrete types. It is not antisymmetric: two
handles may fit each other. So every *question* reads it — admission, a static
verdict, ranking, an ascription, the view door — and no *construction* does.

**What rests on it.** Transitivity is what lets a program be read in steps: a
module that fits a signature can be passed wherever that signature fits.

**What breaks without transitivity.** A module with a `PUSH` at `Number` and
one at `Str` fits `(Stack WITH {Elt = Number}) & (Stack WITH {Elt = Str})`,
and that meet lies under `Stack`. If *fits* pooled the two overloads into one
solve of `Stack`'s `Elt`, the module would be refused at `Stack` directly:
`Number` and `Str` are no one type. The module could then be ascribed to the
meet and passed on as a `Stack`, but not ascribed to `Stack`. *Fits* instead
tries each overload in turn, and takes an ambiguity as a matter for the call.

**What breaks when something is built from it.**
`FOR ALL #[Elt] :{x :Elt, y :Elt} -> Elt` and
`FOR ALL #[Left Right] :{x :Left, y :Right} -> Left | Right` fit each other
and are two types. An overload set deduplicated by *fits* would keep whichever
was declared first and drop the other's registration; a union canonicalized by
*fits* would have a handle that depends on the order of its members.

**What enforces it.** `fits_is_reflexive`, `fits_is_transitive`,
`fits_is_transitive_through_an_instance`, `fits_contains_the_order`,
`fits_bounds_the_signature_meet`, and, for
[`instance_under`](relations.md#quantified-binders),
`an_instance_exists_where_its_scheme_fits`,
`an_instance_fits_the_type_it_is_wanted_at` and, over a wanted type naming
lexical variables bound as a run binds them,
`an_instance_over_lexical_variables_holds_at_every_binding`. The typed handles keep a scheme out of every
construction: the order, `join` and `meet` take `KType`s.

## A solve does not depend on the order of its slots

**The law.** [The unifier](solving.md#the-unifier-collects-it-does-not-bind)
collects every contribution to a variable before it solves any. Each variable
is solved to a pair — the join of its lower contributions and the meet of its
upper ones with its bound — and the solve fails only where that pair denotes
no type. A call takes the pair's **least instance**; a relation asks only that
the pair denote some type.

**What rests on it.** A call by name, whose record has no order; *fits*'
instantiation clause; a static solve, whose intervals must hold every binding
a call can reach.

**What breaks.** Bind a variable to the first argument that reaches it, and
`FOR ALL #[Elt] :{x :Elt, y :Elt} -> Elt` over `{x = 1, y = "s"}` binds `Elt`
to `Number` and refuses `y`, or binds it to `Str` and refuses `x`, by which
field is read first. A record is read in symbol order, so renaming a parameter
changes which call is admitted. Collected, the call binds `Elt` to
`Number | Str` and both are admitted. Where a program wants one slot to fix a
variable for the next, it says so with a
[priority class](solving.md#priority-classes).

A static solve over lexical variables is taken as the call's own only where the
collector reports it
[reproducible](solving.md#the-unifier-collects-it-does-not-bind): binding each
variable as a run does and then solving gives the static solution so bound, and
fails exactly where it fails.

**What enforces it.** `a_solution_is_the_least_instance_of_its_contributions`,
`admission_without_quantifiers_is_the_order`,
`a_carried_variable_is_admitted_where_its_bound_is`,
`a_solution_reads_only_its_solving_slots` and
`a_reproducible_solve_commutes_with_binding`.

## A load-time verdict holds at every run

**The law.** A static type is an **interval** holding every type a run can
carry there, and may hold lexical variables, each standing for whatever a run
binds. Every rule over static types maps both ends. *Always* means every call
within the arguments' intervals is admitted, at every binding of every lexical
variable; *never* means none is. Anything else is *maybe*, and the call
decides.

**What rests on it.** A candidate judged *never* is dropped, and a use left
with none refuses the load. A use whose winner the load selects runs it
[without admitting or ranking](../dispatch/README.md#selection). A settled
ascription retypes without checking. Each is sound only if the verdict was.

**What breaks.**

- *Reading only the upper end.* Under `EXPR #(WHICH x :Number) -> Str`, the
  call `WHICH a` over a parameter `a :(Number | Str)` is *maybe*: `a` may
  carry `Number`. Treat the argument as exactly its upper end and the verdict
  is *never*, and the load drops a candidate that runs. Only where the call
  itself solves from that upper end — an argument's
  [contribution](../dispatch/README.md#static-types) at a slot that solves —
  may the judge read it as a point.
- *Reading a variable as its bound at a contravariant position.* A parameter
  `f :(FN :{x :Elt} -> Null)`, with `Elt` a lexical variable bounded by `Any`,
  read as `FN :{x :Any} -> Null`, is *always* at a slot of that type. Where a
  run binds `Elt` to `Number`, the function over numbers is handed on as one
  that takes anything. `bound_above` reads the lower end there and answers
  `FN :{x :Never} -> Null`, which lies above every instance.
- *A relation that holds only at some bindings.* `Elt & Number` is `Never`
  over a rigid `Elt` and `Number` where a run binds `Elt` to `Number`. A meet
  taken at load would refuse a program that runs, so
  [the load leaves it unknown](../elaborate/README.md#the-type-channel-at-load).

**What enforces it.** `a_carried_solution_lies_in_its_static_interval`,
`a_verdict_holds_of_every_call_within_its_static_types`,
`a_reproducible_solve_commutes_with_binding` and
`bounding_above_lies_over_every_instance`, which draw carried types within the
static ones and bind each lexical variable as a run does. Debug builds check
it on every run: a finished value's carried type lies within its node's static
type, and a call the load decided runs what selection over the full candidate
list would.

## Bypasses

Each of these keeps the suite green while removing what a law protects:

- wrapping a raw `Handle` into a typed one outside the doors that check it;
- narrowing through `TypeRegistry::concrete` without naming the invariant that
  makes the type concrete;
- reading a variable through `erase_rigid` where `bound_above` or an interval
  is meant;
- a structural descent beside the two [walk drivers](README.md#writing-a-new-walk),
  or a second copy of the order's guard set;
- a construction that asks *fits*;
- a law restated "up to equivalence", or a generator narrowed until a law
  passes;
- a rule over static types that maps one end.

## How the laws are tested

The lattice is tested by its laws, as properties over generated type trees
interned into a live registry ([tests/properties.rs](tests/properties.rs)).
The order's, `join`'s and `meet`'s laws hold by handle over every generated
concrete type (`arb_concrete`, a `KType` strategy, which builds no parametric
type into a concrete one), signature types included, and no law has a twin
stated up to equivalence. *Fits*' laws run over every generated type,
parametric ones and schemes included (`arb_any`). Laws about the lattice's
machinery — the unifier, ranking, substitution, interning — read their draws
raw. The generators draw signatures over head parameters, applications pinning
some of them and meets of two applications, and put carried unions under
quantified positions, where a solve joins and meets. The interval and verdict
laws draw their carried types within the static ones, binding each lexical
variable as a run does.

A law draws a case that meets its precondition by construction rather than by
filtering, since a case that passes without reaching the law is invisible: an
ordered pair or chain is built by widening, two shapes share one key, an
argument shape is mostly its candidate's own instance, and a scheme comes with
its own instance. What still misses is a counted rejection, capped per law, so
a generator that drifts off a precondition fails the suite.

Hand-written tests remain only where a law cannot express the shape
([tests/residue.rs](tests/residue.rs); a family's in
[tests/families.rs](tests/families.rs), an interval's in
[tests/intervals.rs](tests/intervals.rs) and a lexical variable's in
[tests/levels.rs](tests/levels.rs)), and each says which.

Beside them the suite pins three things a law would not catch: the import
boundary ([tests/boundary.rs](tests/boundary.rs)), golden digests for the
builtin vocabulary ([tests/golden.rs](tests/golden.rs)), and the
heap-allocation bracket ([tests/heap.rs](tests/heap.rs)).
