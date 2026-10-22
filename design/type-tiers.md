# Type tiers

A type the load builds lives in the frozen load for the life of the program,
and every agent reads it. A type the build rests lives in the agent tier for
the life of the agent. A type the run builds lives in one of two shorter
tiers, chosen by what can still reach it, and is released with that tier: a
region's branch with the region, a scratch branch with its habitat's reset.

This doc tells that story across the modules that carry it.
[The type lattice](../lattice/src/types/identity.md#storage-a-root-and-its-branches)
holds each tier's table and the node layout, [the load's walk](../src/dispatch/README.md#static-types)
types every site and classifies every body, [values](../src/values/README.md#crossing)
re-intern at a crossing and [knots](../src/knot/README.md#a-lambda) at a
closure's birth, and [cellgraph](../cellgraph/README.md#the-contract-four-embedder-types)
brands each tier's door and owns the scratch habitat's reset.
[Build storage](build-storage.md) says what the load and the build decide and
where each rests it.

## The tiers

| tier | lives from | to | holds |
|---|---|---|---|
| frozen load | the load | the program's end | every type the load builds, shared by every agent |
| agent tier | the agent's birth | its death | every type the agent's build cells rest: a binding's nodes and the types the cell's settled facts name, among them what the invariant sites of a caching frame keyed on that binding mint |
| region branch | the region's birth | its death | a porous body's run-built types, and what a caching body's exits carry |
| scratch branch | the habitat's reset | its next reset | a step's scratch types, and every type a caching body builds |

A handle's identity is its content's digest, so it names one type in every
tier, and a reset moves where a type resolves, never what it is. Every branch's table is a
[`DigestTable`](../lattice/src/bump/digest_table.rs) differing only in what
lays its runs; the frozen load's is a run of slots laid once at the freeze and
never written, and the agent tier's grows in the agent's bump. An intern
probes the tiers a step reads longest-lived first, the frozen load, then the
agent tier, then any branch. A read by handle is routed by the handle's
in-root bit
([identity](../lattice/src/types/identity.md#identity-a-handle-is-a-content-digest-under-two-flags)):
a load handle probes the frozen load alone, and any other probes the agent
tier, then the branches longest-lived first, then follows a branch's link to
its spawner's, caching the hit. A verdict over two load types is read from
the load's table, one over load and agent types from the agent's, and any
other from the branches' tables, shortest-lived first. The frozen load is
closed after the load and the agent tier is written only by a build cell at
its rest, so every type the run builds lives in a branch, a composite over
load types alone included.

A crossing carries a value's types to where it lands
([storage](../lattice/src/types/identity.md#storage-a-root-and-its-branches)).
A pinned birth into a cell that has laid no branch — a spawned child, a hop's
successor in a new cell — links the cell's branch to its home's, so a birth
costs the same whatever the home holds. Every other pin — a result, a birth
into a tenant, whose branch is its host's — merges the home's branch, and what
the home reaches through its links up to a branch the destination reaches,
into the destination's; a copy re-lays each type it reads. No pin walks the
value it carries.

## A node lives in a tier its children outlive

A node names its children by reference, so a node's tier is a lifetime
question per node: it lives in a tier that every child's tier outlives. The
tiers are ordered — the frozen load, the agent tier, the region, and a
scratch branch under the region of the cell that writes it — so a node
references nodes of its own tier or a longer-lived one, by covariance alone:
a region node references region, agent and load nodes, and content crosses
from scratch into region by copy: a node relaid in the destination tier, its
children relaid before it, insert-if-absent, so a content relaid twice is one
node. No chain reads a scratch branch of another cell, so every tier a chain
reads outlives the one it writes.

## Porosity

A type minted at run — a carrier [the view door](../src/knot/module/README.md#the-view-door)
mints, a nominal declared per call — and every type built over one is
**minted content**. Porosity says where minted content can sit, as two flags
folded once per type at the load, beside the lattice's others.

A type is **porous** where minted content can lie under it or fit it. A
carrier is an atom of [the order](../lattice/src/types/laws.md#the-order-is-a-partial-order),
under itself, a union holding it and `Any` alone, but a type built over one
reaches further: a list or record over it lies under `Value`, a record with an
extra field at it lies under every record type by width, and a view's
signature lies under `Module`, the top of the signature types, and fits every
signature its members satisfy. At a contravariant position every type but
`Any` admits `P | C`, so `FN :{x :(Number | C)} -> Null` lies under
`FN :{x :Number} -> Null`. So `Any`, `Value`, every record type, every
signature type and every variable are porous; a function or shape type is
porous where a parameter or slot is not typed `Any` or where its return is;
and a list, dict, union or family application is porous where a child is.

A type is **exposing** where a value seen at it can hand a reader a part or
payload of minted content. A part keeps its own memo under `Any`, a union or
a carrier ([values](../src/values/README.md#the-type-memo-and-satisfies)),
and `Value` or a variable restamps nothing; a module seen at a signature hands
back each member at the type the signature declares it at, which may itself be
exposing; a type value's payload is the type it names, whatever
its kind; a nominal's payload is read at its representation. So `Any`,
`Value`, every kind, every signature type, every variable, a carrier and a
nominal are exposing; a union is exposing where it is porous or a child is
exposing; and a list, dict, record or family application is exposing where a
child is exposing or is a porous function type. A function type exposes
nothing: what a function reaches it reaches through its captures, which the
exit walk closes ([below](#the-scratch-branch-as-a-cache)).

A value is **porous** where its static type's upper end is porous or
exposing, and, where that type is exact and closed, where it is exposing: an
exact closed site carries that very type, and the surface door hides what the
type does not name, so a retyped record shows no extra field to any reader
but a knot's copy, which the exit walk closes too. An open capture, typed
`[Never, Any]`, is porous by definition.

Porosity promises only that a value at a non-porous site reaches no minted
content, never that its handles are the load's (§ The tiers).

## Two classes of body

Every frame body — the program, a callable, a quote's code — is classified
where the program loads, the arms and module bodies it runs counted with it. A
value enters a body through a parameter, a capture, a surfaced
`USING … SCOPE` name, an `EVAL`'s result, a hole or a call's result, and
minted content enters only where that value is porous. It leaves only through
the body's return or an argument of a call the body makes into a frame — a
registration, a function called by name, a surfaced head, a candidate the load
cannot read — and only where a porous value meets a porous slot there; and
through every `EVAL`, which offers values to the code's open `\` marks at
slots the body does not type. A value that holds nothing the body minted is
no exit, at a call and at the return alike. A value the body received — a
parameter, a capture or a `LET` bound bare to one, named bare, or a member of
one read through the builtin `ATTR` — holds nothing it minted, since a `LET`
bound bare to a received value holds that very value; so does a record literal written in place
whose every field is non-porous or holds nothing minted, and a module whose
[content tree](modules.md#the-load-and-the-run) reaches no minted content: no
view, open or closed, no closure, no module body holding a minting site in
itself or in a block or module body it runs, and no leaf but a value the body
received. The tree reads a local binder through to what it binds, so a module
built over an alias of a parameter, a literal or another such module holds
nothing minted. A record bound by `LET` is read by its static type alone. A
builtin's native runs in the caller's step and enters no frame. A body mints
at a `:|` ascription, which hides each unpinned
head parameter behind a carrier, and at a nominal declaration the load leaves
unknown, declared each time its unit runs; `:!` mints nothing, since it
carries each head parameter at what *fits* solved.

- **sealed**: no porous exit. Nothing it minted is reachable after it
  returns.
- **porous**: a porous exit.

Beside a sealed body the load fixes whether it caches: it mints, and it is
reached by a call or an `EVAL` — a callable or a quote's code. The program
never caches: its bindings are read across its statements, and nothing above
it is parked.

A tie — a closure's or a quote's birth, a cyclic component's — is the one way
minted content leaves under a non-porous slot: through a closure's or a
quote's captures, since neither a function type nor a code type exposes what
its value captures, and through the cells a knot member's seen type hides,
since a knot is [copied](../src/values/README.md#crossing) at each node's own
memo. That route is closed where the value leaves rather than by
classification: before a caching body's value leaves it, every value and
handle its knots hold is adopted into the region, as a knot's copy reads them
(§ The scratch branch as a cache), so no member holds a scratch handle when it
leaves, and the crossing rules carry its types from there. A tie itself costs
nothing.

## Where each body interns

A porous body interns into the branch of the habitat its writer lays into —
its region, or its host's for a `Shares` tenant — through the header door
beside the writer, so interning into another habitat's branch does not
compile. A caching body interns every type it builds into a scratch tier, and
routes a mint site at the load by what the site's
[content tree](modules.md#the-load-and-the-run) says it mints; a nominal
declared per call is classed alike by the type binders its declaration reads:

- **closed**: a tree with no leaf. The load computes the carrier and the
  view's signature, and, where it knows the operand's signature exactly, every
  type the door computes from types alone — what *fits* solves, each barrier's
  substitutions and each coerced member's type — and the root holds them. The
  run mints there only what *fits* solves from an operand the load cannot type
  exactly, a re-view past the tree's
  [horizon](modules.md#the-load-and-the-run) of a nested module whose declared
  application does not fix each parameter at what the module realizes, and what the coercion derives
  from a member's value, such as an empty list's type: content the same at every run, which interns where the frame
  would intern anything and finds itself there after the first.
- **invariant**: a tree whose every leaf is a capture or a parameter the
  body's tail loop passes through unchanged, so the site mints the same
  content each hop, and whose every parameter leaf keys a build cell — a
  type binder, or one declared closed under `Module` or as a type value,
  bound to content the load names
  ([build storage](build-storage.md#a-build-cell)). Its type is a fact the
  build cell settles under the binding, so it lies in the **warm tier**: the
  agent tier, where the cell rests it
  ([below](#the-agent-tier-as-the-warm-tier)), laid by the build and read by
  every frame under the binding, whatever its placement; a frame whose
  binding names run content mints it into its scratch.
- **varying**: any other tree — one with a leaf keyed on a binder inside the
  body, an action's result or an inline `MATCH`, on a parameter the loop
  rebinds, or on a parameter of any other declared type, whose digest costs
  its size. Into the **cold tier**: the frame's own scratch under `Fresh`, held
  for the frame's life and dropped with it, so each hop recycles it; and the
  host's scratch under `Shares`, where it grows with the loop as the loop's
  bytes do.

A composite site follows its children: warm where its static type reaches
invariant carriers alone, cold otherwise, a type that reaches a carrier the
load cannot name included.

A caching body runs every step on one chain over the frozen load, the agent
tier, the region its writer lays into and the scratch it writes — its own
under `Fresh`, its host's under `Shares`. A scratch chain holds the region
tier it reads between the agent tier and its own by value, so building one
per step lays nothing; no branch holds that tier, since a habitat's door
lives one step and a branch lives as long as its half. A type the body builds
stays in its scratch; an invariant site's type is rested in the agent tier by
the build cell before any frame is born under the binding, so every frame
reads it there and lays nothing into it, and the verdicts the build reaches
over those types are recorded there and stay warm across hops and callers. A
body whose binding names run content runs on the same chain and mints into
its scratch, where its warm and cold tiers are one branch.

A chain carries a pairing brand naming the writer it serves, beside the brand
its tiers sit at, and a value door takes a writer and a chain paired to it.
The caching chains are paired to the frame's region writer: a value lies in
the region, its memo in a scratch tier, and a door handed a writer and a chain
paired to another does not compile.

Whether a loop passes a parameter through unchanged is a greatest fixpoint
over the tail calls the load resolves: a call by name, its arguments paired
with the callee's parameters by field, and each candidate of a keyworded use,
paired by slot. Every parameter starts invariant, and one stays so only where,
on every tail edge into its body, the argument at its slot is a bare invariant
parameter of the caller, and on an edge from the body into itself that same
parameter: a loop that swaps two parameters passes neither through unchanged.
A body
whose name escapes as a value can be tail-called from a site the load cannot
read, so its parameters are varying. The rule is conservative: an argument
built over invariant leaves has the same content from the second round on,
yet counts as varying. A wrong class is never unsound — a site classed
invariant whose content the key does not fix stays unsettled and mints into
the scratch each hop, and one classed varying is settled all the same where
its leaves key through a frontier — since the class decides the key, and
soundness rests on the sealed class alone.

## The agent tier as the warm tier

A caching frame whose body keys a build cell — its caching record lists the
parameters its invariant sites read, or a fact seamed at an entry in its
subtree, other than a parameter's own type, waits on one of its names, and
each name is bound to content the load names
([build storage](build-storage.md#a-build-cell)) — is born under a build cell
of its body under the content at those names: one the drain finds in, or
builds into, the arena before the frame is born. The cell lays the binding's
nodes and the types its facts name in the agent tier, so a frame finds them
whatever became of the asker, and every frame of that body under that
binding, whatever park asked for it and whatever its placement, reads the
same nodes and verdicts; a hop of a tail loop of one callee whose name does
not escape reads under the cell its frame ran under, with no lookup. The
cell's facts are evicted under the arena's budget, least recently used and
leaf first, while the types it rested stay for the agent's life, so no frame
loses the warm tier under it. A frame whose binding names run content is born
under no build cell and settles its facts in its own activation, minting into
the scratch it writes, as below.

## The scratch branch as a cache

**Its span is one call.** The evaluation that asks for a call registers a
receipt run in its host's scratch habitat and parks on it, and a receipt run
holds that habitat's reset off until the registration that replaces it or the
cell's death ([the cell](../cellgraph/README.md#the-cell)). So the host's
scratch lives from the ask to the delivery, whatever the callee does in
between, and resets at the first quiet step end after the evaluation wakes.
Every `Shares` callee born under that park with no build cell is a tenant of
the host, whose scratch is its own, so that scratch is its warm tier and its
cold tier at once. A tail loop of `Shares` callees is therefore warm at its
invariant sites, where the same carrier or per-call nominal minted each hop
probes and hits, and the scratch grows by those sites' content once per park
however long the loop runs. Two `Shares` calls asked at one park — the
operands of one operator run, the parts of one tie — are warm to each other,
and so are the statements of a `Shares` callee, since the asker's park holds
across them; two calls from the host's own body in separate statements are
cold, since that body replaces its own receipt between them, save a keyed
callee's, warm through the agent tier. A `Fresh` caching frame holds its own
scratch for its life, its warm and cold tier at once, and drops it with the
frame. It reads no other cell's scratch, so a `Fresh` frame with no build
cell is warm across its own statements alone, and each hop of a `Fresh` tail
loop under such a binding mints its invariant sites afresh. A loop that parks
once a hop lays no receipt run per hop: the scratch habitat keeps the runs
its cells released, drained, on a free list a registration searches first
fit, cleared at the reset. The cache needs no count of its own and no
`unsafe`.

**A memo may be shorter-lived than its value.** A caching body's values are
laid in the region its writer lays into, and their memos in a scratch tier.
The load's walk is what guarantees no reader of that memo outlives the
scratch: nothing the body minted is reachable after it returns, and every
exit adopts what it carries into the region first, walking the value and
every value and handle its knots hold up to a settled part (below), since a
part keeps its own memo under `Any`, a union or a carrier. The lattice does
not check this
([storage](../lattice/src/types/identity.md#storage-a-root-and-its-branches)).
The debug build's crossing check reads a value inside a caching body against
the chain the body runs on, and a value leaving it against the root and the
region.

**What still lingers in the host.** Before a caching body's value leaves it —
its result, an argument at a frame birth or tail call, a called member's
function type with its carrier, the types a call by name contributes to its
callee's solve, the contract a tail call or an `EVAL` hands on, and a
barrier's four handles, the type the caller sees, the declared slot type and
the two substitutions — its types are adopted into the region. A `Fresh`
caching frame that hands its last statement to the evaluator departs while
that statement runs, and its scratch with it, so the binders that statement
reads, which the load lists, and the frame's contract are adopted at the
hand-off. The adopt reads the whole chain the body ran on, since a `Fresh`
type's children may lie both in the agent tier and in the frame's scratch.
A sealed exit carries no minted content, but it may carry a
composite over root types the body built in a scratch tier. What lingers is
bounded by what the body hands out, never by what it minted.

## An adoption stops at a settled part

Exits and invariant sites adopt through one door ([`adopt_value`](../src/values/crossing.rs)), whose contract
is that every memo a value reaches resolves where the value's readers read. A
value carries two bits in its weight's top, memoized as its weight is, on every
composite header and on a knot's root node: **root-closed**, every memo it
reaches is the root's; and **settled**, every memo it reaches resolves in the
root or its home region's branch. A value door sets both where it builds the
value from its parts ([settled.rs](../src/values/settled.rs)): root-closed
where every part is and its own memo lies in the root, settled where every
part has either bit and the door's chain interns where it lays — a chain over
one half alone, paired to that half's own writer, or the load's — so every type
the door interned lands in the root or the branch of the half the value lies
in. A knot's birth stamps its root node the same way, from the types it carries
and the values it holds; a builtin's answers both. A leaf, a type value and a
header over a knot member carry no bits of their own: a header hands over its
own memo wherever it is adopted and is read through to its member's bits. A
part with either bit set already meets the contract — a root type resolves
everywhere, and a settled type in the region every chain over a value there
links — so the walk does not enter it, at an exit and at an invariant site
alike. At an exit it hands over nothing for that part, its own memo included,
since the region's chain reaches both the root and the home branch; an
invariant site adopts into the warm tier, where the value does not lie, so
there the part's own memo is still handed over. The debug check walks with the
same cutoff. The walk reads the top value's bits before it lays anything, so an
exit of a value that meets the contract allocates nothing, and it lays its
stack, knot set and settled list in its caller's step scratch. An exit marks
every value it entered settled after adopting its memos into the region; an
invariant site marks nothing. A clear bit is always safe and a
set bit is true: a restamp keeps the old bits only as far as the new memo
allows — root-closed where the new type lies in the root, settled where its
chain interns where it lays — a copy recomputes both through the same doors at
its destination, its knots re-stamped there, and a region's branch only grows
while the region lives, so a set bit stays true for the value's life. A test
build audits the law at the cutoff: every part an adoption skips is walked
whole and each memo checked against the bit it was skipped on. So a loop that
passes a large value through unchanged, walks a chain one member a hop, or
hands a module built over the chain to a caching helper each hop, walks none of
the chain per hop.

## Verdicts

A verdict is never load-bearing, so its placement is retention alone. A
verdict over two load types is in the load's table, which the load fills
losslessly and no run writes, or in the agent's where the load never asked
it; one over load and agent types is in the agent's; any other is recorded in
the table of the shortest-lived branch holding one of its two types, and
released with that branch.

## Open work

- [The build arena](../roadmap/gradual-build/build-arena.md) — a frame's
  facts settled in its own activation where its binding names run content.
- [Invariance around a tail cycle](../roadmap/gradual-build/cycle-invariance.md)
  — an argument built over invariant leaves classed invariant.
- [Presizing a branch](../roadmap/gradual-build/branch-presizing.md) — a branch
  laid at a predicted size.
