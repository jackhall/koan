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

The word's top two bits are flags, not digest, and every comparison, hash,
table word and child digest masks them out: **in-root**, whether the node lies
in the root, which is closed after the load, so the bit is fixed for the
program's life; and **concrete**, the node's concreteness flag. Each is set by
the door that interns or finds the node, and baked into the builtin
constants, so a reader asks either question of the handle alone, with no
lookup: a chain skips the root for a handle whose in-root bit is clear, adopts
a root handle with a bit test, and answers a verdict over two root handles
from the root's table alone
([two flag bits](../../../roadmap/gradual-build/handle-bits.md)).

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
`TypeChain::list` pins. A scheme is minted only by `function_scheme` and
`shape_scheme`, which intern the plain body and then bind the group over it,
and answer a `DeclaredType`: the plain type where the group is empty.

Inside the lattice every walk and relation runs over references to stored nodes
([storage](#storage-a-root-and-its-branches)), and [typed.rs](typed.rs) wraps
each relation for the rest of koan: it looks each operand up once, and says,
where it wraps a result, why the result keeps its handle's promise.

## Storage: a root and its branches

The registry is a root and its branches ([tiers.rs](tiers.rs)). The **root**
holds every type the load builds, in program storage for the life of the
program. While the load runs, a `RootBuilder` grows it in a bump its owner
keeps for it; the load's end freezes it into a `Root`, which has no intern
door, so the run reads it and never writes it. A **branch** holds the types the
run builds in one habitat half — a cell's region or a scratch habitat: a small
table in that half's bytes, laid by the first door that writes into the half,
whoever runs it, and released with the half. Every node a tier holds — with
every slice the node holds — lives where the tier does, and nothing the lattice
owns carries drop glue, so a bump or a half releases its tier and every node in
it whole. A type the
run builds lives in the habitat of the value it is laid down with, so a
run-built type lives exactly as long as the values that carry it, and every
handle a value carries resolves from the value's own habitat — in its branch,
in its region's branch for a scratch value, or in the root. An embedder may
lay a value's memo in a shorter tier than the value's — a region value's in
its scratch — where it proves no reader outlives that tier
([type tiers](../../../design/type-tiers.md#the-scratch-branch-as-a-cache));
the lattice does not check it, and a read after that tier's reset misses.
Each door interns
into the branch of the habitat its writer allocates into: a `Shares` tenant into
its host's, a result built into a home into the home's. A door reads and
interns through a [`TypeChain`](chain.rs) over the half it writes, a
`Habitat`: the root, the branch it writes, and for a scratch habitat a
longer-lived branch between the two — its region's, or a branch lent to it —
read through an object-safe `Tier` that re-reads that half's slot on every
probe, so a branch laid there during the step is seen and the lattice needs no
`unsafe` for the one tier a chain reads at another brand. The chain holds that
half by value beside the one it writes (`Halves`, which `Habitat::region`
hands the chain), so building one lays nothing; no branch holds it, since a
half's door is valid for one step. A chain is built per step and never
stored. Two read-only doors let an embedder see where a type lands:
`TypeChain::half`, the half the chain writes, and `TypeChain::in_root`, whether
a type's node lies in the root — on the load's chain, in the root builder.

How a value's types reach another habitat is the crossing's
([crossing](../../../src/values/README.md#crossing)), over three doors
([transfer.rs](transfer.rs)). Nothing bounds what a pinned value carries short
of walking it — a part keeps its own memo under `Any`, a union, a function type
or a carrier, and a closure's captures carry branch handles under a type that
is not porous — so neither pin door walks the value; both rest on a pinned home
outliving its destination.

- A **link** points the destination's branch at the home's: one pointer, laid
  at a pinned birth whatever the home holds. A spawned child links its
  spawner's; a hop's successor or what a root work leaves links its home's
  where the destination has laid no branch yet, as a new cell has not. A read
  by handle that misses the root and the chain's own tiers walks the links and
  caches the hit in its own branch, so the walk is paid once per handle per
  cell; an intern probes no link, laying a duplicate of content an ancestor
  holds, which identity by digest allows. A home with no branch needs no link:
  its values carry root handles alone. A link follows a pin rather than the
  call tree, and a branch links once, to a branch already laid that does not
  reach it, so links never cycle. A birth links as it lays. A branch nothing
  links may also take its link late, where it links nothing yet: its depth and
  bottom change then, and only a branch linking it would have recorded them.
  An embedder links a scratch branch to its region so, since no branch links
  a scratch.
- A **merge** enters every node of the home's branch into the destination's by
  reference: at a pinned result or view, and at a pinned birth into a half
  that has already laid a branch — a tenant's, whose branch is its host's. A
  home's value may carry a part whose type the home reached only through its
  link, since a read caches the node it reads and not that node's children, so
  the merge goes on through the home's links, branch by branch, up to the
  first branch the destination already reaches. A branch records its link
  depth and a 32-bit tag of its chain's bottom's address as it is laid, so
  that check walks the destination's chain only as far as the depth of the
  branch it checks, and answers at once for a branch on a chain whose tag
  differs; a tag shared by two chains costs steps, never a wrong answer, since
  equal depths compare full addresses. A merge pays for how far above the
  destination the two chains meet, never for how deep they sit. A
  result merged into its spawner stops after the child's branch, and a result
  forwarded to an ancestor stops at once, since the reading cell's branch
  links it.
- An **adopt** re-lays a type the destination lacks, and everything under it,
  at the same digests and flags, children first, for a copy, whose home need
  not outlive its destination: the third conversion on stored content,
  `Content::transfer`, beside `Content::lay` and `StoredNode::view_as`. A handle
  a side table holds — a schema's, a group's bounds, a component's member
  schemas — is adopted after its node is laid, which closes every cycle by a
  hit, and a sealed component is re-laid once, its table already absolute.
  An adopt reads the type from the home's branch and the branches it links,
  or from a whole chain — every tier and link that chain reads — where a
  type's children lie in several of them, as a caching body's do when its
  value leaves it
  ([type tiers](../../../design/type-tiers.md#the-scratch-branch-as-a-cache)).

Each node is a [`StoredNode`](stored.rs) laid alone in its tier: its digest,
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
lookup is one probe per tier the chain reads — two in a region, three in a
scratch habitat — and, for a handle the chain's own tiers lack, one per link
walked, once per cell. The registry holds `Never` and
`Any` by reference, so producing either costs no lookup. Interning is
insert-if-absent along the chain, so building the same content twice in a half
yields one node and two equal handles. Identity is by digest, so two branches
holding one content hold one handle and two copies: two nodes compare, order
and hash by their digests, and a reference is an identity only within one
table.

The two tiers keep different tables. The root's is a hashbrown table in
program storage, built during the load, where its grown build and its
stranded bucket arrays are paid once. A miss there is one group probe of
control bytes, and a branch handle's in-root bit spares it even that. A
branch's is a [`DigestTable`](../bump/digest_table.rs) in
its region: 16 B slots, each the digest's low word beside the node's
reference, probed linearly at a load of at most one half. A region's writer
lays fixed runs and has no allocator, so the table grows by laying a run of
twice the length and re-placing each slot by its stored word, reading no node.
A lookup probes the root first, then the region's branch, then a scratch
branch, then for a read by handle the links, and skips a branch that holds
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

Each node whose children decide them carries six flags computed off those
children at intern — whether a free quantifier, whether any rigid variable,
whether anything parametric, whether an opaque carrier is reachable, and
whether the type is porous or exposing
([porosity](../../../design/type-tiers.md#porosity)) — and
every other variant's are a constant of its kind, so each probe is one read of
the node. The probes are the lattice's own;
the rest of koan reads quantified-ness off a `DeclaredType`'s arm and
concreteness through `concrete`.

The **verdict tables** ([verdicts.rs](verdicts.rs)), one per tier, are keyed
by `(subject, candidate, relation)`. A verdict is recorded in the table of the
shortest-lived tier holding one of its two types — a type's tier being the one a
lookup finds it in, the longest-lived holding it — so a verdict over a branch's
type is released with the branch, and the run's verdicts never evict the
load's. A verdict lookup probes each table that exists, shortest-lived first.
The root's table is a fixed run of two-slot buckets, laid in the root's bump
at the load's first verdict or at the freeze, and never resized, so it strands nothing in a bump that releases nothing before the
run ends; a full bucket evicts the slot not touched last. A branch's is a
[`DigestTable`](../bump/digest_table.rs) of verdict entries, laid at the
branch's first verdict, which grows and drops nothing. Either way a key's two
digests fold into its word or bucket and a slot compares the whole key, so a
collision costs a probe and never a wrong answer. A verdict over a digest pair
is a pure function — once computed it never changes — so **verdicts are never
load-bearing**: the root's table is lossy by design, and a forgotten verdict
costs a re-walk, never a wrong answer. The registry therefore owns nothing on
the global heap: the root rests in a bump its owner keeps for it — in a loaded
program, the [substrate's own](../../../src/program/README.md), released whole
with the rest — and each branch in its half.

Every door that sorts, flattens or canonicalizes takes a **scratch allocator**
from its caller and builds its transient buffers there, and every door computes
its digest off the caller's own slices first, so content is laid in the half
only on a miss. Interning a type or running a relation touches the global
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
