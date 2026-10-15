# Type tiers

A type the load builds lives in program storage for the life of the program. A
type the run builds lives in one of three shorter tiers, chosen by what can
still reach it, and is released with that tier: a specialization's branch
with the specialization, a region's branch with the region, a scratch branch
with its habitat's reset.

This doc tells that story across the modules that carry it.
[The type lattice](../lattice/src/types/identity.md#storage-a-root-and-its-branches)
holds each tier's table and the node layout, [the static pass](../src/dispatch/README.md#static-types)
types every site and classifies every body, [values](../src/values/README.md#crossing)
re-intern at a crossing and [knots](../src/knot/README.md#a-lambda) at a
closure's birth, and [cellgraph](../cellgraph/README.md#the-contract-four-embedder-types)
brands each tier's door and owns the scratch habitat's reset.
[The build module](../src/build/README.md) owns the specializations.

## Four tiers

| tier | lives from | to | holds |
|---|---|---|---|
| root | the load | the program's end | every type the load builds |
| specialization branch | the first miss on its binding | its eviction, then its last borrower's disposal | what the invariant sites of a caching frame keyed on that binding mint and copy |
| region branch | the region's birth | its death | a porous body's run-built types, and what a caching body's exits carry |
| scratch branch | the habitat's reset | its next reset | a step's scratch types, and every type a caching body builds |

A handle is its content's digest, so it names one type in every tier, and a
reset moves where a type resolves, never what it is. Every branch's table is a
[`DigestTable`](../lattice/src/bump/digest_table.rs) differing only in what
lays its runs; the root's is a hashbrown table the load grows and freezes. A
node lookup probes the tiers a step reads longest-lived first, the root before
any branch, and a read by handle then follows a branch's link to its
spawner's, caching the hit; a verdict lookup probes them shortest-lived first.
The root is closed after the load, so every type the run builds lives in a
branch, a composite over root types alone included.

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
question per node: it lives in a tier that every child's tier outlives. Region
and root are ordered, and a scratch branch is under the region of the cell
that writes it. So a region node references region and root nodes alone, and
content crosses from scratch into region by copy: a node relaid in the
destination tier, its children relaid before it, insert-if-absent, so a
content relaid twice is one node. A branch lent to a frame in another cell —
a specialization's ([below](#the-specialization-as-the-warm-tier)), or a
scratch branch its asker's park holds ([below](#the-scratch-branch-as-a-cache))
— and that frame's region are unordered: the region retires at the frame's
disposal, before the lent branch goes, unless a pin splices it into an
ancestor, which may outlive it. So neither holds a node over the other. The
lent branch is written through a chain over the root and itself alone, at a brand of its own that
outlives the frame's scratch and is unrelated to its region, and a node over a
child in the frame's region misses at the lay. The frame's scratch is under
both, so its nodes reference either.

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
content, never that its handles are the root's (§ Four tiers).

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
parameter or a capture named bare, or a member of one read through the builtin
`ATTR` — holds nothing it minted; so does a record literal written in place
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
  content each hop. Into the **warm tier**: under `Fresh`, the branch of the
  specialization the frame borrows
  ([below](#the-specialization-as-the-warm-tier)); under `Shares`, the
  host's scratch.
- **varying**: any other tree — one with a leaf keyed on a binder inside the
  body, an action's result or an inline `MATCH`, or on a parameter the loop
  rebinds. Into the **cold tier**: the frame's own scratch under `Fresh`, held
  for the frame's life and dropped with it, so each hop recycles it; and the
  host's scratch under `Shares`, where it grows with the loop as the loop's
  bytes do.

A composite site follows its children: warm where its static type reaches
invariant carriers alone, cold otherwise, a type that reaches a carrier the
load cannot name included.

Under `Shares` the warm and cold tiers are one branch, the host's scratch,
under the host's region. So a `Shares` caching body runs every step on one
chain over the root, the host's region and that scratch, writing the scratch,
and its classes route nothing. A scratch chain holds the tier it reads between
the root and its own by value, so building one per step lays nothing; no
branch holds that tier, since a habitat's door lives one step and a branch
lives as long as its half. Under `Fresh` the two are the lent branch, the
specialization's, and the frame's scratch, and the frame's scratch is under
both it and the frame's region (§ A node lives in a tier its children outlive). So a `Fresh` caching
body reads through a chain whose own tier is its scratch, whose read tier is
the lent branch and whose scratch branch links its region, laid before
anything else lays that branch. A type it builds stays in its scratch, and an
invariant site copies its operands' types and what it mints into the lent
branch, insert-if-absent, so after the first frame under the binding it
hits. Verdicts over the lent branch's types are recorded there and stay warm
across hops and callers.

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
parameter of the caller. A body
whose name escapes as a value can be tail-called from a site the load cannot
read, so its parameters are varying. The rule is conservative: an argument
built over invariant leaves has the same content from the second round on,
yet counts as varying. A wrong class is never unsound — a varying site routed
warm grows the warm tier until it goes, an invariant site routed cold re-lays
each hop — since soundness rests on the sealed class and the lend alone.

## The specialization as the warm tier

A caching frame born `Fresh` whose caching record lists the parameters its
invariant sites read is born borrowing a **specialization** of its body
under the content at those parameters
([keys](../src/build/README.md#keys-which-bindings-make-a-specialization)): a
build cell the drain finds in, or builds into, the table of the build cell the
callee was born under, before the frame is born. The specialization's region
branch is the frame's warm tier, and every frame of that body under that
binding borrows the same one, whatever park asked for it: a second caller
finds the first's invariant types and verdicts laid, and a hop of a tail loop
of one callee borrows the specialization its frame ran under, with no lookup.
A specialization lives until its parent evicts it and its last borrower
disposes, so its warm tier outlives the parks that filled it. A frame whose
specialization's parent is already evicted is born with none, and warms the
scratch its asker's park holds, as below.

## The scratch branch as a cache

**Its span is one call.** The evaluation that asks for a call registers a
receipt run in its host's scratch habitat and parks on it, and a receipt run
holds that habitat's reset off until the registration that replaces it or the
cell's death ([the cell](../cellgraph/README.md#the-cell)). So the host's
scratch lives from the ask to the delivery, whatever the callee does in
between, and resets at the first quiet step end after the evaluation wakes.
Every frame born under that park with no specialization has the same scratch
branch as its warm tier: a `Shares` callee is a tenant of the host, whose
scratch is its own; a `Fresh` callee is **lent** the branch its asker's park
holds by a cellgraph door, and names that scratch until it departs; a hop's
successor born with no specialization keeps its predecessor's warm tier,
lent or own, whatever either's placement. A tail
loop of any placement, a chain across bodies of mixed placement included, is
therefore warm at its invariant sites, where the same carrier or per-call
nominal minted each hop probes and hits, and the scratch grows by those
sites' content once per park however long the loop runs. Two calls asked at
one park — the operands of one operator run, the parts of one tie — are warm
to each other, and so are the statements of a `Shares` callee, since the
asker's park holds across them; two calls from the host's own body in
separate statements are cold, since that body replaces its own receipt
between them, save a keyed `Fresh` callee's, warm through its
specialization. A `Fresh` caching frame also holds its own scratch, its cold
tier, for its life, and drops it with the frame. A loop that parks once a hop
lays no receipt run per hop: the scratch habitat keeps the runs its cells
released, drained, on a free list a registration searches first fit, cleared
at the reset. The cache needs no count of its own and no `unsafe`: a wrong
lend is a miss at the lay, never a dangling read.

**A memo may be shorter-lived than its value.** A caching body's values are
laid in the region its writer lays into, and their memos in a scratch tier.
The static pass is what guarantees no reader of that memo outlives the
scratch: nothing the body minted is reachable after it returns, and every
exit adopts what it carries into the region first, walking the values and
every value and handle their knots hold, since a part keeps its own memo under
`Any`, a union or a carrier. The lattice does not check this
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
type's children may lie both in the lent branch and in the frame's scratch. A
value the frame received — a parameter's, a closure binding's or the
callable's own — adopts its own memo alone: its parts crossed into the region
at the frame's birth, and admission at a slot restamps only the top. So does
an argument the load records as passing one on, a builtin `ATTR` member read
of one included. So a loop that passes a large value through unchanged, or
walks a chain one member a hop, walks none of it per hop.
A sealed exit carries no minted content, but it may carry a
composite over root types the body built in a scratch tier. What lingers is
bounded by what the body hands out, never by what it minted.

## Verdicts

A verdict is never load-bearing, so its placement is retention alone: it is
recorded in the table of the shortest-lived tier holding one of its two
types, and released with that tier.

## Open work

- [Invariance around a tail cycle](../roadmap/type-tiers/cycle-invariance.md)
  — an argument built over invariant leaves classed invariant.
- [Presizing a branch](../roadmap/type-tiers/branch-presizing.md) — a branch
  laid at a predicted size.
- [Evicting specializations](../roadmap/type-tiers/specialization-eviction.md)
  — the policy a build cell evicts its specializations by, so a loop over new
  invariant content each call holds a bounded warm tier.
