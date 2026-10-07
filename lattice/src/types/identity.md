# Identity and storage

What a type handle is, what a typed handle promises, and where the lattice keeps
its nodes. Part of [the type lattice](README.md)'s design.

## Identity: a handle *is* a content digest

A [`Handle`](handle.rs) is a bare `u128` — no pointer, no index, no reference to
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
- **Nothing in the vocabulary is generative.** The one node keyed on something
  outside type content is an opaque view's carrier: a `Parameter` holding a
  `ContentKey`, an identity the module layer computes from the content the view
  hides and the lattice never interprets, folded into the carrier's digest ahead
  of its name and bound. Two views of equal content therefore share a carrier,
  and two of different content never unify
  ([module design](../../../design/modules.md#carriers-and-paths)).

The hasher lives in `digest.rs` and only there. Every payload begins with a
distinct domain tag byte so no two variants can share a digest, every text run is
length-prefixed so concatenation is unambiguous, and every child digest,
`ContentKey` and integer is fed little-endian.

### Typed handles

A raw `Handle` is identity only. Every door and relation the rest of koan calls
takes a **typed handle** over it, which adds nothing at run time and says what
the type may hold:

- a [`KType`](handle.rs) is **concrete**: outside a sealed `Signature` or
  `SetMember` node it holds no free `Quantified`, no lexical variable, no head
  `Parameter` and no quantified binder. An opaque carrier is concrete, since a
  value carries it and dispatch reads it. Every value but a quantified callable
  carries one;
- a `Parametric` may hold a variable, but no quantified binder;
- a `Scheme` is a quantified callable's type: a function type or an expression
  shape over a non-empty `FOR ALL` group. Its positions read, through
  `TypeRegistry::scheme_node`, as `Parametric`;
- a `DeclaredType` is a type or a `Scheme` — what a callable, a registered
  shape, a signature member and a function value are typed by.

Only the lattice wraps a raw handle into a typed one: the wrapping trait is
sealed. So a `KType` is concrete because every door that yields one builds it
from concrete children or checks it. A `KType` converts into a `Parametric`; a
`Parametric` becomes a `KType` only by a substitution that answers every
variable, or through [`TypeRegistry::concrete`](registry.rs), the one checked
conversion, which reads a flag interning stored. Every door that builds a type
from child types is generic over the handle, so it yields a `KType` from
`KType` children and a `Parametric` from parametric ones. A `KType` asked of a
parametric child is a compile error, which a `compile_fail` doctest on
`TypeRegistry::list` pins. A quantified binder is minted only by
`function_scheme` and `shape_scheme`, which answer a `DeclaredType`: the plain
type where the group is empty.

Inside the lattice every walk and relation runs over raw handles, and
[typed.rs](typed.rs) wraps each relation for the rest of koan, saying where it
wraps a result why the result keeps its handle's promise.

## Storage: one region

A [`TypeRegistry`](registry.rs) is built over a bump its owner keeps for it,
and every node it interns — with every slice a node holds — lives in that bump.
Nothing the lattice owns carries drop glue, so the region releases the table and
every node with it, whole.

The handle *is* the lookup key and the digest is already uniformly distributed,
so the node map hashes it with an identity hasher and a lookup costs about what
an array index would. Interning is insert-if-absent, so building the same content
twice in a run yields one node and two equal handles. Beside each node the entry
stores three flags computed off its children at intern — whether a free
quantifier, whether any rigid variable, and whether anything parametric is
reachable — so each probe is one table read. The probes are the lattice's own;
the rest of koan reads quantified-ness off a `DeclaredType`'s arm and
concreteness through `concrete`.

The **verdict table** ([verdicts.rs](verdicts.rs)) is keyed by
`(subject, candidate, relation)`. It is a
fixed run of two-slot buckets, laid in the same region the first time a verdict
is recorded and never resized, so it strands nothing in a bump that releases
nothing before the run ends. A key's two digests fold into its bucket, and a
slot compares the whole key, so a collision costs a slot and never a wrong
answer. A full bucket evicts the slot not touched last. The table is lossy by
design: a verdict over a digest pair is a pure function — once computed it never
changes — so **verdicts are never load-bearing**, and a forgotten one costs a
re-walk, never a wrong answer. The registry therefore owns nothing on the global
heap, and itself rests in a bump its owner keeps for it — in a loaded program,
the [substrate's own](../../../src/program/README.md), released whole with the rest.

Every door that sorts, flattens or canonicalizes takes a **scratch allocator**
from its caller and builds its transient buffers there, and every door computes
its digest off the caller's own slices first, so content is bumped into the run
region only on a miss. Interning a type or running a relation touches the global
heap nowhere — a bracket the suite asserts directly ([tests/heap.rs](tests/heap.rs)).

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
Its **origin** says whether a `SIG` declared it or a module carries it — a
module's self-signature, a view's, the empty one — and is part of its identity,
since *fits* reads the two differently on its asking side. A declared
signature's **head parameters** are a table of its own, each name to the
`Parameter` its members read; a manifest member fixes a concrete type. A schema
with no parameter and no member is always the empty signature, so `SIG E = #[]`
and `Module` are one handle. Every channel is a slice in the run region stored in one canonical
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

A window is opened over a whole component: its members in announcement order,
each carrying the binder that owns it — a `UNION`'s variants — or none for a
standalone declaration, beside the indices each declaring binder owns. A binder
is not itself a member; it denotes the union of the members it owns. So a
`NEWTYPE` and a `UNION` declared in one component seal on one digest. The
standalone group and the one-binder group are that constructor's two special
cases, and the binder list is fixed when the window opens — only the member
list fills.

At the last fill the window seals, and **identity is not the declared group**: it
is each member's strongly-connected component under the sibling-reference
relation, presented canonically in name-symbol order. `seal_group` extracts the
reference edges, runs the bump tier's
[Tarjan walk](../../README.md#strongly-connected-components), and digests the condensation in topological order —
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
