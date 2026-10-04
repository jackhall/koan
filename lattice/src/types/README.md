# The type lattice

A closed algebra over interned type nodes. The node vocabulary, the interning
registry, the identity recipe, the structural relations between types, and the
unifier that solves a quantified position. Nothing else.

Everything that matches a type against something that is *not* a type — a value,
a parser part, a declaration — lives with that thing and calls in here. How the
lattice serves a program from load to call is
[gradual typing](../../../design/gradual-typing.md).

## The invariants

The lattice is a mathematical object, and everything built on it assumes its
laws. A broken law rarely fails where it is broken: it shows up later as an
overload that goes missing, a load that depends on the order two members were
written in, or a call that runs a candidate it never admitted.
[The laws](laws.md) gives each one's reason and a worked example of what
breaks without it.

1. **A value's type is concrete.** An unsolved variable is a pair of ends,
   `[lower, bound]`, or a solved type, and is never read as its bound. The
   typed handles `KType`, `Parametric` and `Scheme` make the difference a
   compile error.
2. **The order is a partial order by handle, over concrete types only.** It
   holds no unification: solving belongs to *fits* and the unifier, at load and
   at the call. Two handles that lie under each other are one handle.
   Everything that is built reads the order.
3. **`join` and `meet` are exact.** `join` is the least upper bound and `meet`
   the greatest lower bound, and the four lattice laws hold by handle.
4. ***Fits* is a preorder containing the order, and nothing is built from it.**
   Every question reads it; no union, overload set or cache key does.
5. **A solve collects, then solves.** Its answer does not depend on the order
   the slots are read in, nor on the order a declared union stores its members.
   It yields a pair of ends per variable and takes a
   point, the least instance, only where a call needs one.
6. **A load-time verdict holds at every run.** A static type is an interval,
   every rule maps both of its ends, and *always* and *never* survive whatever
   a run binds a lexical variable to.
7. **One type, one handle.** Identity is the content digest, so every canonical
   shape — a union's, a record's field order, a quantifier group's numbering —
   is part of identity.
8. **The laws are the tests.** A new node or relation extends the generators
   and the laws. A failing law is fixed in the lattice, never by narrowing a
   generator or restating the law up to equivalence.

## The parts

| Doc | Holds |
|---|---|
| [The laws](laws.md) | each law, what rests on it, what breaks without it, and what enforces it |
| [Identity and storage](identity.md) | the content digest, typed handles, the one region, records and schemas, recursive groups |
| [The node vocabulary](vocabulary.md) | every node variant, the kind order, the three families, the code family |
| [The relations](relations.md) | the order, *fits*, `join` and `meet`, signature types, quantified binders |
| [Solving](solving.md) | substitution, the unifier, and priority classes: admission, ranking and judging |

## The boundary, and why the build holds it

The lattice names exactly two things outside itself: the classified symbol
types from [`symbols`](../symbols/README.md), and `ScopeId`, the
bump-allocation seam and the component walk from
[the bump tier](../../README.md#the-bump-tier). No value, cell, AST, scope,
working part or execute-side type reaches it — the
[parser](../../../src/parse/README.md) included, which is what keeps the two
from naming each other: both rest on `symbols`, a leaf.

The compiler enforces that: the lattice's crate depends on no koan code, so a
path into koan is a build error rather than a review finding.

The rule is what keeps the algebra closed. A relation that needed a value would
be a relation the lattice could not state as a law over generated types, and the
[property suite](laws.md#how-the-laws-are-tested) is only possible because nothing here has a runtime.

The edge runs the other way too, for constants alone: `parse`'s builtin shape
table types each slot by a `KType`, and since a builtin leaf's handle is a `const`
content digest the table states a type with no registry in hand. Every
composite the table spells — the code containers `List(Name)`,
`List(Declaration)`, `Dict(TypeCode, Block)`, `Dict(Name, Block)` and
`Dict(Name, TypeCode)`, the unions `TypeCode` and `List(Name) | Dict(Name,
TypeCode)` (`QUANTIFIER_CODE`, a `FOR ALL` group), and the empty record — is
pinned the same way, and every registry pre-seeds them, so the table's static
data names no type a registry must first intern.
`KType::same_as` is the equality that comparison uses, handle against handle in
`const` context, where the derived `PartialEq` cannot go. Nothing but the handles,
that comparison and the code order's walk over them ([The code
family](vocabulary.md#the-code-family)) crosses back.

## Writing a new walk

Every structural recursion here goes through one of the two drivers in
[walk](walk.rs), with rendering the single hand-written exhaustive match. Adding
a compound node variant is a compile error at the drivers' arm tables, in the
renderer, and at [`TypeNode::view`](node.rs), which reads a node's children as
another typed handle — **and nowhere else**. That is the
property the drivers exist for. Adding any variant, leaf or compound, is also a
compile error at [`family_top`](order.rs), [`leaf_name`](handle.rs) and
[`TypeNode::group`](node.rs), which have no wildcard arm, so no type goes
without a family, no leaf without its spelling, and no binder without its
group.

Ask first whether the walk is unary or binary, then whether it rebuilds.

- **Unary rebuild** ([walk/unary.rs](walk/unary.rs)) — supply a leaf rule
  `FnMut(Handle, &TypeNode, &Context) -> Option<Handle>`: `Some(k)` replaces the
  node and stops, `None` lets the driver descend, and the one knob picks which
  union door reassembles a union. A signature and a sealed member are leaves to
  both unary drivers: one is closed content, the other content-addressed by its
  component. Substitution and sibling rewriting are this.
- **Unary visit** — supply a pre-order rule returning descend / skip / stop.
  Occurrence censuses and reference folds are this. A probe over the free
  `Quantified` positions alone takes `visit_free_quantified`, which skips nested
  binders for it.
- **Binary** ([walk/binary.rs](walk/binary.rs)) — implement `Lockstep`: an entry
  guard, a leaf verdict, a set-wise rule for unions, and a structural combine that
  reads the arm's width verdict generically. The order — serving *fits* too —
  the solver's meet, which the public `meet` runs over concrete operands, and
  the unifier's collector are the three instances. Two signature
  types, and two callables of which one is quantified, reach the leaf verdict
  rather than a child pairing, because their relations are application-level and
  instantiation-level doors.

The context a unary rule is handed answers the two questions the arm table
alone cannot: how many binders lie between the root and here — `binder_depth`,
which counts shapes and quantified function types alike — and the variance of
the current position.

[Rendering](render.rs) is exempt, and for a stated reason: it spells syntax
*between* children and inherits the quantifier binder from above, which neither
driver expresses. It spells a type as a program writes it: a quantifier group
as `FOR ALL #[Elt Key]`, or as a dict of each name to its bound,
`FOR ALL #{Elt: Number, Key: Any}`, once any bound is not `Any`; a signature's
head parameters as a bounded group, `SIG FOR ALL #{Elt: Any} (top: Elt)`, and an
application in a meet parenthesized, `(… WITH {Elt = Number}) & (… WITH {Elt = Str})`;
and an expression shape's head quoted, `#(PURE _ :Elt)`. Every entry point takes the registry and the symbol interner,
never a bundle — the lattice knows about types and symbols and nothing else.

## Open work

- [Recursion over run-time types](../../../roadmap/rewrite/recursion-over-run-time-types.md)
  — every structural walk, relation and rendering over types as deep as a
  run-time value's carried type.
- [Unplanned work](../../../roadmap/rewrite/README.md#unplanned-work) — a
  singleton static type for a type value, and anonymous structural recursion
  in types.
