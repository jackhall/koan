# The type lattice

A closed algebra over interned type nodes. The node vocabulary, the interning
registry, the identity recipe, the structural relations between types, and the
unifier that solves a quantified position. Nothing else.

Everything that matches a type against something that is *not* a type — a value,
a parser part, a declaration — lives with that thing and calls in here.

## The boundary, and why it is a test

The lattice imports exactly two things from the rest of koan: the classified
label and symbol types from [`parse`](../parse/README.md), and `ScopeId` plus
the bump-allocation seam from [`memory`](../memory/README.md). No value, cell,
AST, scope, working part or execute-side type reaches it.

The compiler cannot enforce that — a public module may name anything in its own
crate — so [`tests::boundary`](tests/boundary.rs) reads this module's own source
and fails on any other `crate::` path, with an explicit allowlist of the items
each of the two permitted edges may name.

The rule is what keeps the algebra closed. A relation that needed a value would
be a relation the lattice could not state as a law over generated types, and the
property suite below is only possible because nothing here has a runtime.

## Identity: a handle *is* a content digest

A [`KType`](handle.rs) is a bare `u128` — no pointer, no index, no reference to
the registry that minted it. That one word is the type's content digest, so
equality, hashing and ordering all derive on it: **comparing two types is
comparing two integers**, and no structural descent exists to fall back to. The
width is chosen so an accidental collision is less likely than a hardware fault,
which is what makes digest equality *be* type equality with no repair path.

Three things follow, and each is load-bearing:

- **There is one recipe** ([digest.rs](digest.rs)). A node's digest is its tag
  byte, its own scalar payload, and its children's digests — which are already
  known, because children are handles. Nothing in the recipe walks a type: it is
  one layer deep. The only buffer it needs is the symbol sort an order-blind
  record or union digests under.
- **Identity is interner-independent.** The digest is a pure function of
  content, so two independently built types with the same content are equal with
  no shared registry.
- **Generativity is one explicit mechanism**, applied in two places: a minted
  `ScopeId` nonce folded into the content ahead of everything else, carried by a
  recursive-group window and by an abstract member. Two opaque ascriptions of one
  signature never unify, and nothing else in the vocabulary is generative.

The hasher lives in `digest.rs` and only there. Every payload begins with a
distinct domain tag byte so no two variants can share a digest, every text run is
length-prefixed so concatenation is unambiguous, and every child digest,
`ScopeId` and integer is fed little-endian.

## The node vocabulary

A [`TypeNode`](node.rs) is one interned type's content: a variant tag, a scalar
payload, and **handles to its child types** — never owned substructure. Every run
a node holds is a slice in the run region, so a node is `Copy` and carries no
drop glue, and reading one out of the registry copies a few words rather than a
subtree.

- **Leaves** — `Number`, `Str`, `Bool`, `Null`, `Identifier`, and the two bounds
  `Any` (the top, and the default bound of a rigid variable) and `Never` (the
  uninhabited bottom, the identity element of both `join` and union
  canonicalization).
- **Binder-position slots** that capture syntax raw and never resolve —
  `NameToken`, `TypeNameToken`, `KExpression`, `SigiledTypeExpr`, `RecordType`.
  They are types because a declarator's slot has to be typed like any other, not
  because anything is ever matched against them structurally.
- **`OfKind(KKind)`** — a type-accepting argument slot carrying the shallow
  [`KKind`](kind.rs) it admits. It is **type-channel only**: it admits a type
  *value*, never a runtime instance. A value is matched by a type, never by a
  kind.
- **Composites** — `List`, `Dict`, `Record` (structural, width- and
  depth-subtyped), `KFunction` (a named-parameter record plus a return),
  `Union` (canonical: deduplicated, no nesting, no member below another, two or
  more, in the order first written), and `ConstructorApply`.
- **`ExpressionShape`** — the type of a keyworded, positional definition reached
  by dispatch: the interleaved keyword/argument element sequence a call must
  spell, the type parameters bound ahead of it, and the return. It is
  *representationally* distinct from `KFunction`: a lambda takes a record of named
  arguments and is reached by name; a shape is reached by its keyword sequence and
  its argument *positions* are load-bearing, which a canonically ordered record
  erases. So no shape is ever equal to, satisfies, or is satisfied by a lambda
  type.
- **Two rigid variables**, and the split is deliberate. `Quantified` is
  positional and bound by the enclosing shape, so two shapes alpha-equivalent
  under a renaming intern to one node. `AbstractType` is *named*, because
  signature members are reached by name and schemas are edited by name. They
  share the rigid rule in the order, the substitution mechanism, and the role of
  the rigid side in a specificity check.
- **`Signature`** — owned interface content. A `SIG`-declared interface, a
  module's self-sig, and the empty signature that `:Module` lowers to are all
  this one node, distinguished only by the schema. It carries no binder and no
  label: two textually identical declarations are one type.
- **`Sibling` and `SetMember`** — the pre-seal and post-seal forms of a
  co-declared nominal group. See *Recursive groups*, below.

### `KKind` is the order on the type channel

Kinds form one subsumption lattice —
`AnyType > { Signature, ProperType > { NewType, TypeConstructor } }` — and
`OfKind(x) ≤ OfKind(y)` iff `y.admits(x)`. The signature wall lives here: a
proper-type slot names what can type an ordinary value, which a signature is not.
`AnyType` is a *slot* expectation only, never a classification `kind_of` produces.

## Storage: one region, one heap table

A [`TypeRegistry`](registry.rs) is built over the run region's bump allocator,
and every node it interns — with every slice a node holds — lives in that region.
Nothing the lattice owns carries drop glue, so the region releases the table and
every node with it, whole.

The handle *is* the lookup key and the digest is already uniformly distributed,
so the node map hashes it with an identity hasher and a lookup costs about what
an array index would. Interning is insert-if-absent, so building the same content
twice in a run yields one node and two equal handles. Beside each node the entry
stores two flags computed off its children at intern — whether a free quantifier,
and whether any rigid variable, is reachable — so both probes are one table read.

The **verdict table** is the one heap-owned part, keyed by
`(subject, candidate, relation)`. It is on the heap because a bound on verdict
storage is a permissible knob, and a table that may shrink cannot live in a region
that releases nothing before the run ends. A verdict over a digest pair is a pure
function — once computed it never changes — so **verdicts are never
load-bearing**: a cold registry costs a re-walk, never a wrong answer.

Every door that sorts, flattens or canonicalizes takes a **scratch allocator**
from its caller and builds its transient buffers there, and every door computes
its digest off the caller's own slices first, so content is bumped into the run
region only on a miss. Interning a type or running a relation touches the global
heap nowhere — a bracket the suite asserts directly ([tests/heap.rs](tests/heap.rs)).

## The relations

[`is_subtype_of`](order.rs) is **one reflexive partial order**, memoized through
the registry's verdict edges. There are no tie-break tiers: nothing ranks a token
leaf against `Str`, a nominal slot against a kind slot, or a constrained slot
against an unconstrained one. Those are not subtype facts, and a dispatch that
must choose between two unrelated slot types has an ambiguity, not a verdict.

**The guard set lives in `order.rs` and only there.** Every relation that reads
the order — the three slot compositions, the unifier's leaf, the specificity
verdict, the signature relation's value-slot rule — reaches it through
`is_subtype_of`, so there is no second descent to keep in step with this one.
`is_more_specific_than` is its strict version and `satisfied_by` the same
question read from a slot's side.

[`join`](lattice.rs) **is not a walk.** It is the larger operand when the two are
ordered and their canonical union otherwise, which is what makes it associative —
a structural join stops being associative the moment `a ≤ a | b`. `meet` is the
binary driver's rebuilding instance and is total: a pair with no common
refinement meets at `Never`, which is always a sound lower bound. The four
laws — commutativity, associativity, idempotence and absorption — hold over every
node kind, and that is what fixes both operations.

[`sig_subtype`](sig_relations.rs) is the relation over two signature schemas:
`sub <: sup` iff `sub` supplies every member `sup` names, with each manifest
member equal, each abstract member present at the matching kind and under the
declared bound, each value slot covariantly compatible after abstract-member
substitution, each keyworded member satisfied by an overload the same selection
dispatch would make, and each operator record covered at an equal mode.
`meet_schemas` is what two signatures meet at; **the module lattice has no join
of its own**, since two unordered signatures join to their union.
`shape_specificity` ranks two candidates under one bucket key.

### Substitute, then ask

[substitute.rs](substitute.rs) holds substitution and the relations that are *a
substitution composed with an ordinary one* — slot equality, slot satisfaction,
slot specificity, quantifier instantiation and erasure. No second structural
descent exists for any of them. Interning is insert-if-absent on a
content-addressed table, a substitution that binds nothing returns its input
handle, and every subtype verdict is memoized, so the composition costs one
intern per changed composite.

### The unifier collects, it does not bind

[`admits_with`](unify.rs) walks a declared type against a carried one and
collects what would solve the quantified positions. Instead of binding a variable
to the first argument it meets, it records **every** argument type that reaches
the variable as a lower contribution (covariant position) or an upper one
(contravariant). `Collector::solve` then takes, per variable, the maximum of the
lower contributions, else the minimum of the upper ones, else the declared bound.

A contribution set with no maximum is a **failure, not a join**: the solver never
mints a union nobody wrote, and admission cannot depend on the order the slots
are read. So `(f _ :Elt _ :Elt)` admits `(1, 2)` with `Elt = Number` and
`(1, (1 | "x"))` with `Elt = (Number | Str)`, and rejects `(1, "x")`. A caller
who wants mixed arguments writes the union in the slot type or in the bound.

## Records and schemas

A [`Record`](record.rs) is an ordered, symbol-keyed field list — the shape behind
a struct schema's fields, a function type's parameters, and a constructor
application's arguments. Keys are `BinderSymbol`s, never text: a field name is a
fixed-width content digest carried alongside the binding class its own parse
established, so a lookup is a `u128` compare and no field name is ever copied or
re-classified. Identity is the key's `Symbol` bits alone, so a schema's class
rides past the intern boundary without widening what makes two records the same.

A record is one `Copy` fat pointer into the run region, with no index table: at
record sizes a linear symbol compare beats hashing. Two invariants define it —
**declaration order is preserved** for rendering and positional construction but
**equality ignores it** (the digest agrees by feeding the fields symbol-sorted),
and **names are unique** within a record.

A [`SigSchema`](schema.rs) is the normalized carrier of a signature's shape.
Members are split by *representation* rather than by surface syntax: an abstract
member is a rigid variable with no concrete witness, a manifest member fixes a
concrete type. Every channel is a slice in the run region stored in one canonical
order — named tables symbol-sorted with each name once, the keyworded channel and
the operator channel each in their own canonical order — so **a reader walks a
table in that order and never sorts one**, and a lookup by name is a binary
search. The two unnamed channels' order is fixed in exactly one place, the door a
schema enters the lattice through.

Projecting a declaration into a schema needs the declaring scope and lives with
the elaborator; the lattice takes a finished draft.

## Recursive groups: identity is the SCC, not the declaration

A [`RecursiveGroupWindow`](window.rs) is the declarator-local pre-seal record a
group of co-declared nominal types elaborates against. It is a **held record, not
registry state** — several can be open at once, which a registry-hosted stack
could not express — and it lives in the frame region of the node that declares
the group, never the run region and never the heap. Nothing on a window is
digestible and nothing on it survives the seal.

Inside the window, a reference to a co-declared member is a `Sibling` handle: a
bare relative index, ordinary interned content, meaningful only against the
window that minted it.

At the last fill the window seals, and **identity is not the declared group**: it
is each member's strongly-connected component under the sibling-reference
relation, presented canonically in name-symbol order. `seal_group` extracts the
reference edges, runs Tarjan, and digests the condensation in topological order —
every component after the components it references, so a cross-component
reference folds the referent's already-finished handle as ordinary external
content while an intra-component one stays relative.

The consequences are the point:

- A standalone declaration is a singleton component whose presentation is
  byte-identical to the whole-declaration recipe, so no existing single-type
  digest moves.
- Adding an unreferenced member to a group perturbs nobody else's identity.
- A non-recursive member declared inside a group unifies with its standalone twin.

A relative schema builds its unions through the ordinary canonicalizing door
while it still holds `Sibling` handles, and that is exact: the order relates a
`Sibling` as an atom with the same profile as the sealed member it stands for, and
distinct indices name distinct members, so a union canonical before the seal is
canonical after it.

## Writing a new walk

Every structural recursion here goes through one of the two drivers in
[walk](walk.rs), with rendering the single hand-written exhaustive match. Adding
a compound node variant is a compile error at the drivers' arm tables, at the
descent-knob sites, and in the renderer — **and nowhere else**. That is the
property the drivers exist for.

Ask first whether the walk is unary or binary, then whether it rebuilds.

- **Unary rebuild** ([walk/unary.rs](walk/unary.rs)) — supply a leaf rule
  `FnMut(KType, &TypeNode, &Context) -> Option<KType>`: `Some(k)` replaces the
  node and stops, `None` lets the driver descend. Then pick the descent knobs:
  whether a nested `Signature` is descended or treated as a leaf, and which union
  door reassembles a union. Substitution, binder canonicalization and sibling
  rewriting are all this.
- **Unary visit** — supply a pre-order rule returning descend / skip / stop.
  Occurrence censuses and reference folds are this.
- **Binary** ([walk/binary.rs](walk/binary.rs)) — implement `Lockstep`: an entry
  guard, a leaf verdict, a set-wise rule for unions, and a structural combine that
  reads the arm's width verdict generically. The order, the meet and the unifier's
  collector are the three instances. Two signatures and two quantified shapes
  reach the leaf verdict rather than a child pairing, because their relations are
  schema-level and instantiation-level doors.

The context a unary rule is handed answers the three questions the arm table
alone cannot: whether an enclosing descended signature declares a given abstract
member, how many shape binders lie between the root and here, and the variance of
the current position.

[Rendering](render.rs) is exempt, and for a stated reason: it spells syntax
*between* children and inherits the quantifier binder from above, which neither
driver expresses. Every entry point takes the registry and the label interner,
never a bundle — the lattice knows about types and labels and nothing else.

## Laws, not shapes

The lattice is tested by its **laws**, as properties over generated type trees
interned into a live registry ([tests/properties.rs](tests/properties.rs)). The
order's reflexivity, antisymmetry and transitivity; join and meet's four laws;
substitution's fixpoints; the digest's agreement with structural equality.
Hand-written tests remain only where a law cannot express the shape
([tests/residue.rs](tests/residue.rs)), and each says which.

Beside them the suite pins three things a law would not catch: the import
boundary ([tests/boundary.rs](tests/boundary.rs)), golden digests for the builtin
vocabulary so an identity move is visible in a diff
([tests/golden.rs](tests/golden.rs)), and the heap-allocation bracket
([tests/heap.rs](tests/heap.rs)).
