# The relations

Part of [the type lattice](README.md)'s design.

Two relations, and the split is the point. [`is_subtype_of`](order.rs) is **the
order**: reflexive, transitive and antisymmetric, memoized through the
registry's verdict edges, and it **never solves**. It relates **concrete types
and nothing else** — it takes `KType`s — and two signature types compare by
their applications' pins. It is what every construction reads: interning, a
canonical union, `join`, `meet`, an overload set's subsumption, a cache key.
[`fits`](order.rs) is what every **question** reads: admission, a static
verdict, ranking, a settled ascription, the return check, the view door. It
takes parametric types and schemes, and solves: a quantified binder fits
another when some instantiation of its group puts the instance under the other,
and a module's signature fits a declared one when its members do. It holds the
**rigid rule**: a variable lies under its bound and everything above it, and
above only itself, `Never` and its lower end. *Fits* contains the order and is
reflexive and transitive, but two handles may fit each other, so nothing is
built from it. `satisfied_by` is *fits* read from a slot's side. Each relation
records its verdict under its own `Relation`, by handle pair, so a caller
compares no slot types of its own. *Fits*, the unifier, and the ranking and
judging relations are the only relations that read a parametric type.

There are no tie-break tiers: nothing ranks a token leaf against `Str`, a
nominal slot against a kind slot, or a constrained slot against an
unconstrained one. Those are not subtype facts, and a dispatch that must choose
between two unrelated slot types has an ambiguity, not a verdict.

**The guard set lives in `order.rs` and only there.** The two relations are one
descent, the `Order` lockstep, which differs between them at its leaf alone, and
a nested pair goes back through the entry point of the relation it was asked
under. Every reader of either — the unifier's leaf, the specificity verdict,
*fits*' value-slot rule — reaches it through `is_subtype_of` or `fits`, so there
is no second descent to keep in step with this one.

**An application lies under its family.** Two applications of one family are
ordered argument by argument, covariantly, and an application lies strictly
under the bare family it applies, which stands for every application of it.
Covariance is sound because a family's representation is covariant in its
parameters: the elaborator refuses one placing a parameter at a contravariant
position, which [`quantifies_contravariantly`](registry.rs) finds.

[`join`](lattice.rs) **is not a walk.** It is the larger operand when the two are
ordered and their canonical union otherwise, which is what makes it associative —
a structural join stops being associative the moment `a ≤ a | b`. `meet`,
spelled `A & B`, is the binary driver's rebuilding instance and is total: a pair
with no common refinement meets at `Never`, which is always a sound lower bound.
Both take and return `KType`s. The four laws — commutativity, associativity,
idempotence and absorption — hold by handle over every concrete type, and that
is what fixes both operations. A meet over a `FOR ALL` variable or a head
parameter is refused where the program writes it
([the elaborator](../../../src/elaborate/README.md#what-a-type-expression-is)): each call
solves the variable, so the meet cannot be taken at load, and no intersection
type keeps it symbolic.

**A union holding a variable keeps it.** [`union_of`](registry.rs) reduces the
concrete members among themselves by the order and keeps every parametric
member beside them, even one whose bound lies under a concrete member:
`Elt | Number | Str` stays three members with `Elt` bounded by `Number | Str`,
and fits `Number | Str` both ways. `Any` absorbs a variable too, since that is
the top's definition rather than the order. An opaque carrier is concrete, so
the order reduces it like any member, under the union of the rest as well as
under one member.

**The solver's meet** is private to the lattice. Where a solve meets two
parametric types — a variable's least instance over its upper contributions
alone, or the value slots *fits* pools from an offered signature — it relates a
variable by the rigid rule. A union meets member by member, each member against
the other side *whole*, so a variable whose bound spans several members
survives: with `Elt` bounded by `Number | Str`, `(Elt | Bool)` met with
`Number | Str` is `Elt`.

Because the order never solves, its laws hold **by handle over every concrete
type**, signature types included: two handles that lie under each other are one
handle. A quantified binder never reaches the order, and two that admit each
other do so in *fits* alone — `∀Elt :{x :Elt, y :Elt} -> Elt` fits
`∀A B :{x :A, y :B} -> A | B` at its instance over `A | B`. A union and an
overload set keep the first of two concrete members that lie under each other,
and [`unsubsumed`](order.rs) is where they do; an overload set keeps every
scheme it holds, deduplicated by handle.

## Signature types

A signature type reads as a **set of applications**
([signatures.rs](signatures.rs)), an application being a declared signature
with some of its head parameters pinned: a `Signature` is the one application
pinning nothing — the empty signature, `Module`, is the empty set — a
`SignatureApply` is itself, and a `SignatureMeet` is its members. In the order,
one application lies under another of the same signature when its pins include
the other's, each at an equal type, so `Stack WITH {Elt = Number}` lies under
`Stack`, and two different pins of one parameter are unordered, even `Number`
and `Number | Str`. A set lies under another when each application of the upper
lies above some application of the lower, so the empty set is the top. The meet
of two sets is their union less each application lying above another, which
makes it exact: `(Stack WITH {Elt = Number}) & (Stack WITH {Elt = Str})` holds
both applications and lies under each. **The module lattice has no join of its
own**, since two unordered signature types join to their union. Whether one
declared signature's members include another's is a question for *fits*, never
the order.

[`sig_fits`](sig_relations.rs) is *fits* over two signature types. The offered
side fits each asked application on its own, its members pooled across the
offered applications: each manifest member equal, each value slot under the
declared type, each keyworded member satisfied by some offered overload at its
key, and each operator record covered at an equal mode. An asked application's
unpinned head parameters are solved first, as a call solves its group — one
collector, to which each asked member contributes what the offer gives it, and
the [least instance](solving.md#the-unifier-collects-it-does-not-bind) taken — and then
every member is checked under that solution. A variable is read by its side:

| | offered side | asked side |
|---|---|---|
| a member's own `FOR ALL` variable | solved afresh for each member, as a call solves it | rigid |
| an unpinned head parameter | one rigid unknown per application | one variable, solved and discarded |
| a pinned head parameter | the pin | the pin |

For each asked keyworded member, one offered overload at its key contributes,
each tried in turn: pooling a key's overloads would make *fits* not transitive.
A module with a `PUSH` at `Number` and one at `Str` fits the meet of `Stack`'s
two pinned applications, which lies under `Stack`, so it must fit `Stack` too,
and it does, at the first overload's `Elt`. An offered overload with a group of
its own contributes nothing and is checked after the solve. *Fits* asks only
that some overload satisfy each keyworded member, so a tie is an ambiguity where
a call meets it; refusing one would break transitivity the same way. A module's
self-signature stands on the offered side like any signature, and on the asked
side compares by handle. A member's variable may solve to any type, a signature
holding a quantified member included: containment by instantiation is
undecidable in general, so *fits* is what the collector answers, and a relation
it misses is refused at load and at run alike.
[`fits_application`](sig_relations.rs) hands back what one application solves
each parameter to — the pins, then the solution — which
[the view door](../../../src/knot/module/README.md#the-view-door) reads. A keyworded
member whose offered bucket ranks its slots otherwise fails as
`RankingMismatch`. `shape_specificity` ranks two candidates under one bucket key
— and it reads shapes alone, since a function type ranks in no bucket.

## Quantified binders

A quantified binder relates to another of its kind only in *fits*, and there by
**instantiation**, not structurally: some instantiation of the subject's variables, each under its
bound, must put the instance below the other with the other's variables rigid.
[`admits_shape`](sig_relations.rs) is that clause for two shapes, position by
position; [`admits_function`](sig_relations.rs) is its twin for two function
types, name by name — each parameter pair asking the candidate's parameter to
lie under the declared one and the return pair the reverse, then one `solve`.
The solve asks only that each variable's pair of ends denote some type
([the unifier](solving.md#the-unifier-collects-it-does-not-bind)); it picks no instance.
[`instance_under`](sig_relations.rs) shares `admits_function`'s walk and picks
one: the least instance of a function scheme under a function type it is
wanted at, each variable bound to the least instance of its pair. It fails
where the scheme does not fit that type, and names each variable no
contribution reaches rather than read it as its bound. The instance fits the
wanted type, and need not lie under it in the order: a signature is ordered by
its applications, so an instance returning a signature fits a wanted
signature asking for no member without lying under it. Where the wanted type
names lexical variables, the instance fits it at every binding of them, each
entry of the solution so bound within its variable's bound.
Width is the order's own either way: a function subtype asks for no name its
supertype does not. A quantified binder is a `Scheme`, which the order never
takes.

**A binder keeps every variable it declares.** Both doors that mint one —
[`shape_scheme`](registry.rs) and [`function_scheme`](registry.rs) — number a group
over the positions it binds: first the variables some position names, by first
occurrence, a shape walking its slots in element order and a function its
parameters in **symbol-sorted key order** — a record's identity is order-blind,
so the numbering must be too — and then the return; then every variable no
position names, in declared order. No variable is dropped, so a call solves each
one: under `EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Type = #(Elt)`, `KIND 1` binds
`Elt` to `Number`. The names are render-only, and the digest feeds the group's
arity and each variable's bound, so two alpha-variants are one handle. The door
hands its caller back the declaration-index → group-index map alongside the
handle, which is what a call needs to bind each type parameter to its solution.
A variable no argument reaches binds its bound, the least instance of
`[Never, bound]`.
