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
  known, because each child is interned first. Nothing in the recipe walks a type: it is
  one layer deep. The only buffer it needs is the symbol sort an order-blind
  record or union digests under.
- **Identity is interner-independent.** The digest is a pure function of
  content, so two independently built types with the same content are equal with
  no shared registry.
- **Nothing in the vocabulary is generative.** The one node keyed on something
  outside type content is an opaque view's carrier: a `Carrier` holding a
  `ContentKey`, an identity the module layer computes from the view's own
  content and the lattice never interprets, folded into the carrier's digest
  ahead of its name and the bound it met. Two views of equal content therefore
  share a carrier, and two of different content never unify
  ([module design](../../../design/modules.md#carriers-and-paths)).

The hasher lives in `digest.rs` and only there. Every payload begins with a
distinct domain tag byte so no two variants can share a digest, every text run is
length-prefixed so concatenation is unambiguous, and every child digest,
`ContentKey` and integer is fed little-endian.

A scheme's digest is its tag, its group's arity and each variable's bound in
group order, and its body's digest. The names are never fed, so two
alpha-variants are one handle and the group holds whichever spelling was
interned first. A function type's and a shape's digests take no group: a
variable inside one is a `Quantified` child, digested by its index and bound.

### Typed handles

A raw `Handle` is identity only. Every door and relation the rest of koan calls
takes a **typed handle** over it, which adds nothing at run time and says what
the type may hold:

- a [`KType`](handle.rs) is **concrete**: outside a sealed `Signature` or
  `SetMember` node it holds no free `Quantified`, no lexical variable, no head
  `Parameter` and no scheme. An opaque carrier is concrete, since a
  value carries it and dispatch reads it. Every value but a quantified callable
  carries one;
- a `Parametric` may hold a variable, but no scheme;
- a `Scheme` is a quantified callable's type: a `Scheme` node, a non-empty
  `FOR ALL` group over a function type or an expression shape. `Scheme::group`
  reads its group alone, and `DeclaredType::view` reads a declared type whole —
  its group, empty for a type, and its body's node, the positions read as
  `Parametric`;
- a `DeclaredType` is a type or a `Scheme` — what a callable, a registered
  shape, a signature member and a function value are typed by.

Only the lattice wraps a raw handle into a typed one: the wrapping trait is
sealed. So a `KType` is concrete because every door that yields one builds it
from concrete children or checks it. A `KType` converts into a `Parametric`; a
`Parametric` becomes a `KType` only by a substitution that answers every
variable, or through [`TypeChain::concrete`](chain.rs), the one checked
conversion, which reads a flag interning stored. Every door that builds a type
from child types is generic over the handle, so it yields a `KType` from
`KType` children and a `Parametric` from parametric ones. A `KType` asked of a
parametric child is a compile error, which a `compile_fail` doctest on
`TypeRegistry::list` pins. A scheme is minted only by `function_scheme` and
`shape_scheme`, which intern the plain body and then bind the group over it,
and answer a `DeclaredType`: the plain type where the group is empty.

Inside the lattice every walk and relation runs over references to stored nodes
([storage](#storage-a-root-and-its-branches)), and [typed.rs](typed.rs) wraps
each relation for the rest of koan: it looks each operand up once, and says,
where it wraps a result, why the result keeps its handle's promise.

## Storage: a root and its branches

A [`TypeChain`](chain.rs) is built over a bump its owner keeps for it,
and every node it interns — with every slice a node holds — lives in that bump.
Nothing the lattice owns carries drop glue, so the region releases the table and
every node with it, whole.

The registry is two tiers. The **root** holds every type the load builds, in
program storage for the life of the program. A **branch** holds the types the
run builds in one region: a small table in that region's bump, parent the root,
created at the first intern and released with the region. A type the run
builds lives in the region of the value it is laid down with, so a run-built
type lives exactly as long as the values that carry it, and every handle a value
carries resolves from the value's own region — in its branch or in the root.
Each door interns into the branch of the region its writer allocates into: a
`Shares` tenant into its host's, a result built into a home into the home's.
How a value's types reach another region is the crossing's
([crossing](../../../src/values/README.md#crossing)).

Each node is a [`StoredNode`](stored.rs) laid alone in the bump: its digest,
then 32 B of content, 48 B in all. The content names each child — and each
bound, lower end, code kind, carrier's met bound and application signature — by
a reference to that child's node, so a walk or a relation steps from node to
node and never consults a table. A shape's element keeps its slot's priority
class, a `u16`, beside the slot's reference. A field no hot path reads sits
behind one reference laid on an intern miss: a carrier's key and name, a
lexical variable's name, a signature's schema, a deferred return's surface, and
a sealed member's component.
[`TypeChain::node`](chain.rs) reads a stored node back as a `TypeNode`,
every field by value and each child as a handle, so no reader outside the
lattice sees this layout.

A tier's table holds a reference to each node and nothing else. The digest is
already uniformly distributed, so the table hashes it by its low word and finds
a node by comparing against the digest the node holds: the digest is stored
once, in the node. A handle is looked up once, where it enters the lattice — at
a typed door or a public reader, or where it is read out of a quantifier
group's bounds or a schema, which keep handles because koan reads them — and a
lookup is at most two probes, one per tier. The registry holds `Never` and
`Any` by reference, so producing either costs no lookup. Interning is
insert-if-absent along the chain, so building the same content twice in a region
yields one node and two equal handles. Identity is by digest, so two branches
holding one content hold one handle and two copies: two nodes compare, order
and hash by their digests, and a reference is an identity only within one
table.

The two tiers keep different tables. The root's is a hashbrown table in
program storage, built during the load, where its grown build and its
stranded bucket arrays are paid once. A miss there is one group probe of
control bytes, so a branch handle that enters at the root pays almost nothing
for the detour. A branch's is a [`DigestTable`](../bump/digest_table.rs) in
its region: 16 B slots, each the digest's low word beside the node's
reference, probed linearly at a load of at most one half. A region's writer
lays fixed runs and has no allocator, so the table grows by laying a run of
twice the length and re-placing each slot by its stored word, reading no node.
A lookup probes the root first, then the branch, and skips a branch that holds
nothing.

The lattice's [node-table benchmark](../../benches/node_table.rs) times both
tables over 48 B linked stand-in nodes, from 1,000 to 60,000 entries, each
grown from its smallest size and presized
(`cargo bench -p lattice --bench node_table`):

| | digest table | hashbrown |
|---|---|---|
| dependent hit, warm, 60k | 28–33 ns | 27–36 ns |
| dependent hit, cold, 60k | 258 ns | 348 ns |
| miss, 60k | 12 ns | 2.2 ns |
| dependent hit, branch of 8 | 3.5 ns | 8.5 ns |
| table bytes per entry, grown | 33–44 B | 12–20 B |
| build, grown over presized | 1.2–5.4× | 2.8–5.2× |

Warm, the two hit alike, since hashbrown's control bytes stay cached; cold, a
digest-table hit reads a slot line and the node, where hashbrown reads a
control group, a bucket and the node. Hashbrown's miss is one well-predicted
group probe, where linear probing takes a branch per occupied slot, which is
why the root keeps hashbrown. With a hashbrown root of 16,000 and a
digest-table branch of 256, root first costs a lookup stream drawing a tenth
of its keys from the branch 23.2 ns per lookup against 27.0 ns branch first,
and a stream drawn wholly from the branch 6.5 ns against 6.1 ns.

Each node whose children decide them carries four flags computed off those
children at intern — whether a free quantifier, whether any rigid variable,
whether anything parametric, and whether an opaque carrier is reachable — and
every other variant's are a constant of its kind, so each probe is one read of
the node. The probes are the lattice's own;
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
re-walk, never a wrong answer. A verdict over a branch node outlives the branch
harmlessly for the same reason: it is a fact about content, and holds for that
content wherever it is interned again. The registry therefore owns nothing on
the global heap, and itself rests in a bump its owner keeps for it — in a loaded
program, the [substrate's own](../../../src/program/README.md), released whole
with the rest.

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
distinct indices name distinct members, so a union canonical in relative form
is canonical absolute.

A member's schema names types over the member — its own cyclic edges, its
siblings' — while the member must reach its schema: the region's one cycle.
[`Component::tie`](component.rs) closes it. The seal lays each component once,
in the region, as its digest read it: every member's name, kind and schema, with
each reference into the component still a relative `Sibling` and each
cross-component one the referent's member handle. Every member node shares that
component, so the seal interns the members first, then rewrites each schema
absolute, re-interning each union through the same canonicalizing door, and
repoints the component at the absolute table. The rename `Sibling(i) ↦ member_i`
preserves every subsumption verdict, so the union is the one the relative schema
held. Every reader after the seal reads the absolute schema with one field read,
[`TypeChain::member_schema`](chain.rs). The table sits behind a cell that
erases its lifetime, so nodes stay covariant; its read is the crate's one
`unsafe`, sound because `tie` alone writes it, once, before the component is
returned. The lattice's own walks and
relations never read a member's schema; only koan does, through that read.
