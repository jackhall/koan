# The type lattice

The type lattice is a closed algebra over interned type nodes: the node vocabulary, the interning
registry, the identity recipe, the structural relations between types, and the unifier that
solves a quantified position. It lives in `src/type_lattice`, exported from the library root, and
imports the label and symbol types from [`parse`](../../src/parse/labels.rs), `ScopeId` from
[`memory`](../../src/memory/scope_id.rs), and nothing else. No value, cell, AST, scope, working
part or execute-side type reaches it. Everything that matches a type against something that is
not a type lives with that thing and calls the lattice.

## Boundary

The lattice owns every type that appears inside a node: `TypeNode`, the `KType` handle,
`TypeRegistry`, `TypeDigest`, `KKind`, the `Record` field schema, `SigSchema` with its member maps,
`NodeSchema`, the `DispatchTokenElement` an expression shape is spelled in, the
`DeferredReturnSurface` a deferred return is shadowed as, `Specificity`, and the unifier's
`Variance`, `Collector` and `UnifyFailure`. It also owns `ReductionMode` and `FoldDirection`: a
signature declares how a run of its operators reduces, that declaration is part of the
signature's identity, and so the vocabulary is the lattice's. The operator registry imports it
from here.

Outside the lattice, and calling into it:

- **Admission.** `matches_value`, `matches_held`, `matches_type`, `accepts_carried`,
  `accepts_working_part`, `accepts_part` and `slot_ktype` match a type against a value, a type
  value or a parser part. They live with the values and parts they inspect and descend a type only
  through `Union`. Which channel a bare-name part is tried on — token capture or resolved value —
  is admission's rule; the lattice has no opinion, because a token leaf and `Str` are unrelated
  atoms.
- **Projection.** Turning a `SIG` declaration or a module draft into a schema needs the declaring
  scope; it lives with the elaborator and hands the lattice a finished schema whose own-member
  references are already re-sourced to `ScopeId::SENTINEL`.
- **Window elaboration.** Building a recursive group's relative schemas from the AST is the
  elaborator's; the lattice only opens and seals the window.
- **Region storage.** Re-homing a dispatch token into a region is
  [`memory`](../memory-model.md)'s.
- **Slot well-formedness and lazy-slot reads.** The rule that a builtin's union carrier slot may not
  contain `KExpression` beside a capturing member, and the seal-time derivation of which positions of
  a shape are lazy, read shape elements through the lattice and decide nothing inside it.
- **The overload picker.** Ranking live callables feeds the lattice's `shape_specificity` the
  slots read off a callable; the verdict is the lattice's.
- **Coercion plans and key probes.** The ascription coercion tables are built over the lattice's
  substitution and `signature` doors, and comparing a shape against a parsed bucket key is done
  over the lattice's node reads; neither is a node type, so neither is here.

The lazy and binder slot leaves — `KExpression`, `SigiledTypeExpr`, `RecordType`, `NameToken`,
`TypeNameToken`, `Identifier` — are atoms: distinct leaves with no relation to anything but
themselves, `Any` and `Never`. What they capture is admission's concern.

## Relations

The public surface is `is_subtype_of`, `is_more_specific_than`, `satisfied_by`, `join`, `meet`,
`union_of`, the quantifier substitutions, the member substitutions and the three `slot_*`
compositions, `admits_with` with its collector, `join_schemas`, `meet_schemas`, `sig_subtype`,
`select_keyworded_satisfier`, `shape_specificity`, interning, rendering, node reads, and the
window and seal doors. Every caller reaches types through these.

### The order

`is_subtype_of` is one reflexive partial order, memoized through the registry's verdict edges
([type-registry.md § Verdict edges](type-registry.md#verdict-edges-memoize-subtyping)).
`is_more_specific_than` is the strict version: unequal handles and a subtype. `satisfied_by` asks
whether the carried type is a subtype of the slot.

- `Never` is the bottom and `Any` the top.
- `OfKind(x) ≤ OfKind(y)` when `y` admits `x`; the kind lattice of [`KKind`](../../src/type_lattice/kind.rs)
  is the whole story on the type channel.
- Lists, dicts and constructor applications are covariant in every child. Records are covariant
  and width-superset: the subtype has every field of the supertype. Functions are contravariant
  in their parameters and width-subset there — the subtype asks for no name the supertype does
  not — and covariant in the return. An expression shape pairs positionally under equal keywords,
  contravariant in its slots and covariant in its return.
- A union is below `b` when every member is; a non-union is below a union when it is below some
  member.
- A signature is below another when `sig_subtype` accepts the pair.
- A rigid variable — `Quantified` or `AbstractType` — is a nominal identity over its bound: below
  it are only itself and `Never`, above it only itself and everything above its bound.
- Quantified shapes relate by instantiation (below).
- A pre-seal `Sibling` is an atom: below it only itself and `Never`, above it only itself and
  `Any`, the same profile as the sealed member it stands for.
- Every other pair is unrelated. Nothing ranks a token leaf against `Str`, a nominal slot against
  a kind slot, or a constrained slot against an unconstrained one: those are not subtype facts,
  and a dispatch that needs to choose between two unrelated slot types has an ambiguity, not a
  verdict.

### Join and meet

`join` is the least upper bound: the larger operand when the two are ordered, otherwise their
union. `meet` is the greatest lower bound and is structural: pointwise through lists, dicts and
constructor arguments; the union of both field sets with shared fields met for records; the
intersection of parameter names with shared parameters joined and returns met for functions;
distribution through a union; `meet_schemas` for two signatures; `Never` where no common shape
exists. The four laws — commutativity, associativity, idempotence, absorption — hold over the whole
node vocabulary, which is what fixes both operations: a structural join is not associative once
`a ≤ a | b`, and a meet that returns `Never` where a common lower bound exists is not associative
either.

Unions are canonical: `union_of` flattens, drops `Never`, deduplicates, and drops every member
that is a subtype of another member, so `Any` absorbs and no two distinct handles are mutually
ordered. A relative schema canonicalizes its unions before the seal, against `Sibling` atoms;
the seal's rewrite renames each `Sibling` to a member with the same relation profile, so it
re-interns a union through the flat door without a second subsumption pass and the result is
canonical.

`join_schemas` is width intersection with per-member depth reconciliation; `meet_schemas` is its
dual and reports a conflict — two manifest types for one name, two kinds for one abstract member,
two modes for one operator run — as the absence of a meet. A schema's keyworded members are
canonical too, by the same subsumption rule `union_of` applies to a union's members: a shape that
another shape is below is dropped, so two interfaces that satisfy each other are one interface.
The order rather than admission, because the drop has to preserve what the schema promises: a
shape admits another by its slots alone, so an admission-keyed drop can discard a member whose
*return* the survivor does not supply, and `meet_schemas` would then land above one of its own
operands. A module's self-signature is its interface, and two
modules that satisfy the same interfaces have one type.

### Quantifiers

A `FOR ALL` group binds rigid variables over an expression shape. Each variable carries a bound,
`Any` unless declared, and its identity is its index in the group plus that bound; the names are
render-only, so alpha-variants intern once. An abstract signature member is the same kind of
thing with a name instead of an index, because members are reached by name and schemas are
edited by name; the two are distinct node kinds that share the rigid rule in the order, the
substitution mechanism, and the role of the rigid side in a specificity check.

A bound is a **variable-free** type, and both doors that mint a rigid variable assert it. That is
what keeps the order's two rigid clauses consistent: below a variable are only itself and `Never`
while above it is everything above its bound, and a rigid bound would put one variable in both
sets at once. A caller minting a bound out of an arbitrary type — the demoted member
`join_schemas` produces over two bindings — runs it through `erase_rigid` first, which replaces
every rigid variable with the bound it stands over; widening an upper bound that way is sound,
since the join owes only that each operand still satisfies it.

A shape is interned in canonical form. A variable with no occurrence is dropped. A variable that
occurs exactly once is replaced by its bound when the occurrence is contravariant and by `Never`
when it is covariant, since a single occurrence is equivalent to that replacement in every
admission and every subtype question. Variables that occur twice or more stay and are renumbered
by first occurrence. The door reports the renumbering so a caller holding declaration-order
bindings can translate.

**Solving.** `admits_with` walks a declared type against a carried one under a variance and,
instead of binding a variable to the first argument it meets, records every argument type that
reaches the variable as a lower contribution (covariant position) or an upper contribution
(contravariant). `solve` then takes, per variable, the maximum of the lower contributions, else
the minimum of the upper ones, else the declared bound, and checks the lower side lies under the
upper and the solution under the bound. A contribution set with no maximum is a failure, not a
join: the solver never mints a union nobody wrote. `(f _ :Elt _ :Elt)` therefore admits
`(1, 2)` with `Elt = Number` and `(1, (1 | "x"))` with `Elt = (Number | Str)`, and rejects
`(1, "x")`. Admission does not depend on the order the slots are read.

**Instantiation.** `instantiate_quantified` substitutes the solution through a shape's slots and
return, and the canonical form drops the emptied group; `erase_quantified` is instantiation with
each variable's bound, which is what a quantified callable reports on the value lane. A shape's
own variables are bound by it, not free in it: substitution of free variables leaves a shape
node alone, and a slot typed with a quantified shape holds no free variable.

**Quantified shapes in the order.** A quantified shape is below a shape when some instantiation
of its variables, each under its bound, puts the instance below the other shape with the other
shape's variables rigid. The instantiation is found by the same collector: each slot pair asks
the other shape's slot to lie under this shape's, the return pair asks this shape's return to lie
under the other's, and `solve` decides. This is what makes canonical form necessary: two shapes
that are each below the other must be one handle.

### Specificity

`shape_specificity` ranks two candidates under one bucket key by admission: `a` is at least as
specific as `b` when `b` admits `a`'s slot types as arguments, with `a`'s variables rigid and
`b`'s solved. Both directions give the four verdicts `StrictlyMore`, `StrictlyLess`, `Equal`,
`Incomparable`. For monomorphic shapes this is the pointwise fold of the order over paired slots;
for a generic candidate it is the classic "more specific method" rule, so `(f _ :Number)` beats
`(f FOR ALL (Elt) _ :Elt)` and `(f _ :Any)` ties with it. Return types are not compared.
`sig_subtype` uses it to select which of a module's satisfying overloads a declared keyworded
member names, and dispatch uses the same door to rank candidates, so the two cannot drift. A pair
that is not two shapes is `Incomparable`: a non-shape reads as the *empty* bucket key, so without
the refusal every pair of unrelated leaves would tie.

## Two walks

Every structural recursion over a node's children goes through one of two drivers, except
rendering: rendering spells syntax between children and inherits the quantifier binder from
above, which a fold cannot express, and a new variant must be spelled there, so its exhaustive
match is the one recursion written by hand.

The **unary driver** owns one arm table — children per compound arm, and the registry door that
reassembles each — behind two entry points. `rebuild` takes a leaf rule and re-interns each
rebuilt composite through the registry's ordinary doors, so a rebuild that changes nothing returns
the input handle. `visit` takes a pre-order rule that descends, skips or stops. Both take a
descent knob: whether `Signature` is descended or treated as a leaf (`visit` has the same knob for
`SetMember`; a sealed member cannot be rebuilt, so `rebuild` has none), and, for `rebuild`, which
union door reassembles a union — the canonicalizing one, or the seal's flat one. The driver hands
every rule a context: whether an enclosing descended signature declares a given abstract member,
how many shape binders lie on the path from the root to the current node, and the variance of
the current position. Member substitution
descends into a nested signature and asks the context about shadowing; quantifier substitution
stops its leaf rule at the first nested shape; identity canonicalization treats a signature as a
leaf. Substitution, canonicalization, sibling rewriting, the quantifier census, and the
member-reference folds are each written as leaf rules.

The **binary driver** owns the pairing policy per arm — positional for lists, dicts and
monomorphic shapes, by name for record fields, function parameters and constructor arguments,
set-wise for unions — the variance flip at a function's parameters and a shape's slots, a width
verdict per arm (superset for records, subset for function parameters, exact for constructor
arguments), and the mismatch case. An instance supplies an entry guard, a leaf verdict, a set-wise
rule, and a structural combine that reads the width verdict generically. `is_subtype_of` is a
short-circuiting instance; `meet` is the instance that rebuilds; `admits_with` is the instance
that collects. `join` is not a walk. Two signatures and two quantified shapes reach the leaf
verdict, because their relations are schema-level and instantiation-level doors, not child
pairings.

Adding a compound variant is a compile error at the drivers' arm tables, at the descent-knob
sites, and in the rendering match, and nowhere else.

## Substitute, then ask

A relation under a substitution is the substitution composed with the ordinary relation:
`slot_satisfied_by` is `satisfied_by` of the substituted type, `slot_more_specific_or_equal` is
`is_subtype_of` of it, `slot_types_equal` is handle equality of it. No second structural descent
exists for any of them. Interning is insert-if-absent on a content-addressed table, a
substitution that binds nothing returns its input handle, and every subtype verdict is memoized,
so the composition costs one intern per changed composite. The guard set of the order is stated
in one place.

## One identity recipe

A node's digest is the one-layer recipe of [type-identity.md](type-identity.md): a tag byte per
node kind, the node's own scalar payload, and its children's digests — a rigid variable's bound
among them — which are already known because children are handles. The tag table
is hand-written, since the tags are the identity, and a test pins that every node kind has one.
The golden handle constants are pinned by value.

There is no second recipe. A signature's identity is the ordinary recipe over its projected
schema: projection has already replaced the declaring scope in every reference to one of the
signature's own abstract members with the canonical sentinel, so a textually identical declaration
projects to the same nodes and a schema digests by its member handles alone.

## Sealing

The registry has two doors for a recursive group. Opening a window interns relative schemas over
`Sibling(index)`. Sealing computes the component digest over the members in canonical order — the
numeric order of their name symbols — and interns each `SetMember` with absolute references to its
siblings. Sealed content never contains a `Sibling`, so no predicate meets one after the seal;
before it, the order relates a `Sibling` as the atom it will become, which is what lets a
relative schema's unions canonicalize before their members exist. Sealing the same group in any
member order yields the same handles, and sealing what is already sealed is the identity.

## Laws

The lattice is tested by its laws, as properties over generated type trees interned into a live
registry, with the golden module beside them in `tests/properties.rs` under the core:

- `join` and `meet` are commutative, associative and idempotent, and absorb each other.
- `is_subtype_of` is a partial order with `Never` at the bottom and `Any` at the top; `a ≤ b`
  holds exactly when `join(a, b) = b` and exactly when `meet(a, b) = a`; a rigid variable has
  exactly itself and `Never` below it.
- `union_of` is insensitive to member order, idempotent and flattening; no member of a canonical
  union is below another.
- Interning content-equal nodes twice yields one handle.
- Substituting a binding list onto a type with no quantified position returns the input handle;
  erasing is substituting every variable's bound; substituting members that a type does not
  reference returns the input handle.
- Canonical shape form is a fixed point; `shape_specificity` flips when its arguments swap, every
  shape is `Equal` to itself, and on monomorphic shapes it is the pointwise fold of the order.
- A solution `admits_with` finds, substituted into the declared type, is satisfied by the carried
  type; it is always a contribution or the declared bound; it is the extremum of the set that
  constrains it — the maximum of the lower contributions where there are any, else the minimum of
  the upper ones, else the bound; and it does not depend on the order the slots are read.
- Sealing is order-insensitive and idempotent.
- Rendering is total and deterministic. That a rendered type parses back to the same handle is a
  property of the type-language parser, tested where the parser lives.

Hand-written tests remain only where a law cannot express the shape, and each says which.

## Open work

- [Integrate the type lattice](../../roadmap/refactor/type-lattice-integration.md)
