# Lifetimes

What each lifetime in the crate proves, what rests on the proof, and where a
claim is sealed behind the retype rather than proved. The
[crate README](../README.md#the-contract-four-embedder-types) says what each
brand *is*; this is why each one is there.

A lifetime here is one of two things. A **borrow** is a reference the borrow
checker holds for a scope — of storage, of the graph, of the region table — and
its proof is that scope: nothing it covers can move or die while it is held. A
**brand** is a lifetime quantified per call (`for<'cell>`), whose whole content
is its declared relations to the others; its proof is that nothing at it can
leave the call, and nothing can be coerced to it that was not minted at it.
Several are both: a real borrow made invariant so it also brands.

| lifetime | kind | relations | variance where it matters |
|---|---|---|---|
| `'graph` | borrow of storage outside the graph | outlives everything | graph invariant |
| `'step` | exclusive borrow of the cells, per `enter` | `'graph: 'step`; none to `'here` or any habitat brand | `Ready` covariant |
| `'here` | shared borrow of the region table, per `enter` | `'graph: 'here`; `'here: 'scratch` | invariant |
| `'scratch` | brand of the scratch habitat, per `enter` | `'here: 'scratch`, `'lent: 'scratch`, nothing else | invariant |
| `'lent` | brand of the half a tree cell borrows, per `enter` | `'graph: 'lent`; `'lent: 'scratch`; none to `'here` | invariant |
| `'cell` | a habitat half's brand, generically | `'graph: 'cell` | per door; `Writer`, `Header`, `Active` and `OnceRun` invariant |
| `'door` | a door's own life, generically | filled with `'step` by every step and build door, with the storage borrow by `Storage` | `Writer`, `Header`, `Run` and `Prose` covariant |
| `'severed` | a copied operand's brand, per build | `'graph: 'severed`; none to `'cell` | quantified |
| `'h` | a home header's brand, per read | `'graph: 'h: 'b`, `'b` the operand's brand | quantified |
| `'their` | the consumer's scratch brand, per delivery | `'graph: 'their` | quantified |
| `'home` | a carrier's liveness brand | filled with `'step` | `Ready` covariant |
| `'long`, `'short` | the covariance witness's pair | `'graph: 'long: 'short` | the identity must compile |
| `'dispose` | exclusive borrow of the graph, per release | none | internal |

## `'graph`: storage outlives the graph, and no region is storage

`'graph` is the borrow of storage the embedder keeps outside the graph, and
the graph is invariant in it
([`CellGraph`](graph.rs)), so it never shortens to a step's brand. The graph
lives inside `'graph` by construction: a `CellGraph<'graph, …>` is a value, and
a value cannot name its own lifetime, so the one lifetime the type can carry is
of something that outlives it. That is the point rather than a looseness, since
both facts the substrate trusts are about storage against regions:

- **Storage outlives every step.** A `&'graph` borrow laid into a cell — in a
  continuation's captures, under a region borrow in a value, through
  `Storage`'s writer — is valid at every later reattach of that cell's slots,
  because every step runs inside the graph and the graph runs inside `'graph`.
- **Nothing region-lived can be spelled `'graph`.** Every region lives inside
  the graph, so a region borrow is strictly shorter than `'graph`, and the
  invariant brands below keep it from being widened.

The second fact carries the crossing. The README says a `'graph` borrow
carries no reach and no price and crosses a copy as it is: a copied operand is
severed by lifetime, and a `'graph` part of its view embeds into the
destination untouched. That is sound only because a region borrow cannot pose
as one. Were a region borrow ever spelled `'graph`, it would ride a copied
view unpriced, outlive its region, and dangle. So `'graph` is not a
convenience for reaching program text; it is the line between what the
substrate prices and what it need not.

**The erased spelling.** A slot the substrate keeps across steps — the
continuation, the scratch state, a once-written slot, a receipt, a habitat
header — holds its
family's shape at `'graph`, `Erased<'graph, V> = V::At<'graph>`
([reattach.rs](reattach.rs)). That spelling claims the value's `'cell` part is
valid for `'graph`, which is false: it is valid for its region's life. The claim
is sealed — the erased value is never read at `'graph`, only retyped back to a
brand by a door that holds the proof — and it is a *spelling*, not a proof. A
slot has to have some type rustc accepts, and `'graph` is the one lifetime
every family is accepted at: a family nesting a storage borrow under a region
borrow, `&'cell Entry<'graph, 'cell>`, is a type only where `'graph: 'cell`,
which the trait states as its where-clause. `'static` is not
such a lifetime, since `&'static Entry<'graph, 'static>` would need
`'graph: 'static`.

**Why it is on every type.** Not as a proof obligation per struct. The bound
`V: Reattachable<'graph>` names it, and so every type that holds a family's
shape or hands one back must name it too: `StepContext`, `Ready`, `Active`,
`Dormant`, `OnceRun`, `OnceView`, `CrossedOperand`, `Header`, `HomeHeader`, the
slots and the pools. A
holder could in principle lose it; a door cannot, because a door's output is
the family at the graph's own `'graph`, and that identity has to be pinned by
the door's type. If a door were instead generic in `'graph` at the method, with
only `'graph: 'cell` witnessed, a caller could instantiate it at `'static` and
reattach bytes written at a storage borrow as a `'static` one — a lengthening,
reached from safe code.

**What it costs.** A closure quantified over a brand, `for<'cell>`, cannot
prove `'graph: 'cell` of the type it returns, so a placement's build hands back
an `Active` — the value erased beside an invariant `'cell` marker — rather than
the bare shape, and takes the bound as a value, `&'cell &'graph ()`, the
witness every build receives ([carrier.rs](carrier.rs)). That is the price of
`'graph` being a real borrow the brands sit under rather than a free name.

## `'step`: a carrier and a door die with the step, and nothing dies during it

`'step` is the exclusive borrow of the cells half of the graph that `enter`
holds for the closure's length ([`StepContext`](graph.rs)), and the brand of
every `Ready` carrier the step's doors mint or redeem and of every door onto a
habitat the step takes. Three proofs:

- **A carrier cannot leave the step.** `Ready<'graph, 'step, …>` names the
  borrow, and the borrow ends with the closure, so the only state an embedder
  can keep across steps is `Dormant`, which names no step and carries no live
  value.
- **No cell dies while a carrier is live.** Every death verb — release, absorb,
  seal — takes the graph exclusively, which `'step` forbids for the step's
  length. So the liveness `redeem` checks once holds for as long as the carrier
  it hands back exists. This is what fills `'home`.
- **A door cannot leave the step.** A door onto a habitat half — `Writer`,
  `Header`, and the `Run` and `Prose` a writer opens — names the half's entry in
  the region table, not its chunks, and a `create_tree` that grows the table
  moves every tree cell's entry between steps. So every door a step takes, the
  builds' of `alloc_into`, `alloc_here` and `deliver_scratch` included, carries
  `'step` as its `'door` ([region.rs](region.rs)). `'step` outlives no brand a
  slot is re-anchored at, nor any brand a build is quantified at, so no slot,
  value or habitat can hold a door, while what the door wrote, at `'cell`, rides
  every one of them.

`'step` has no outlives relation to `'here`: a carrier is not a region borrow,
and a region borrow is not a carrier. A foreign carrier is read at
`&'cell self`, a borrow of the context strictly inside the step, which can
coerce to neither. Nor has it one to `'scratch`, `'lent`, a build's `'cell` or a
delivery's `'their`, which is what keeps a door in its step.

## `'here`: the executing cell's storage is fixed for the step, at a name only this step holds

`'here` is the shared borrow of the region table that `enter` holds for the
step, beside its exclusive borrow of everything else, made invariant by a
marker on the context. The borrow proves that nothing can move or drop a bump
during the step — the verbs that do need the table exclusively — so a value
built in the cell's region is held as a plain `&'here`, with no carrier and no
price: its reach is the cell itself, alive for as long as the borrow. For a
tenant, `'here` is its host's, which is why a tenant's writes embed host-homed
borrows for free.

The invariance is what makes the continuation slot honest. The slot is written
at this cell's `'here` and erased; `enter` on the same cell reattaches it at
the next step's `'here`. Both brands name the same region, whose life spans
every step in it, so the referents hold. Invariance closes the two routes that
would break this: a step cannot widen a `'here` reference to anything longer,
and nothing at a shorter brand — a `'scratch` borrow, a read's inner borrow —
can be written into a `'here` slot. Quantification per `enter` closes the
third: a `'here` reference cannot leave the closure, so the one door from
`'here` into anything that outlives the step is `lift`, which prices it.

## `'scratch`: a reset strands no reader

`'scratch` is the brand of the executing cell's scratch habitat, invariant and
quantified per `enter` like `'here`, and `'here: 'scratch` is its whole
relation. The proof is directional: a scratch value may reference `'here`
storage, and a scratch borrow can go nowhere that asks for `'here` — which
every door into storage does. So when the habitat is handed back whole at the
end of a step, nothing in storage names it, and the reset is a bump reset with
no reader to strand. Each route by which a scratch borrow could reach storage is
pinned by a `compile_fail` doctest on `scratch_writer`.

The scratch state is the one thing that crosses a park at `'scratch`, in a
slot of its own over both brands, `At<'here, 'scratch>`, erased at
`('graph, 'graph)`. That sealed claim is honest for the same reason as the
continuation's plus one more: the reset is held off while the slot is occupied
([`names_scratch`](slots.rs)), so the bytes a reattached `'scratch` part names
are still there.

The habitat's header slot is cleared at every reset and is no part of what
holds one off, so no header laid at `'scratch` outlives the bytes it may name.
`Writer` is invariant, so the region's writer at `'here` is
never taken at `'scratch`: a scratch-lived value is never laid in storage, where
its bytes would be stranded until the cell dies.

## `'lent`: a borrowed half outlives the borrower

`'lent` is the brand of the half a tree cell borrows from its lender from its
creation to its disposal ([lend.rs](lend.rs)): the scratch of its tree parent's
write home (`lent_scratch`), or the region of a named cell (`lent_region`). The
lent half outlives the borrower on a count per half. A lent scratch's reset
waits on its scratch borrowers, which [`scratch_named`](graph.rs) reads, and
its disposal on the borrower as a child; a lent region's disposal waits on its
region borrowers, and a region's chunks never move while it lives. Both counts
drop at the borrower's disposal, not its release, because a dead borrower's
scratch outlives the release and may name the lent bytes. So the lent bytes
outlive every step of the borrower and every scratch it parks: `'lent:
'scratch` is honest, and the borrower's scratch may name the lent half. `'lent`
has no relation to `'here`, so nothing in the lent half names the borrower's
region, which retires at the borrower's disposal while the lent half stays.
A value laid in the lent half names `'graph`, the lent half itself and what the
lender's own storage may name, so the lender reads it back at its own brand.

## `'cell`: the brand a door is generic over

`'cell` is not a brand of its own but the parameter the brands fill: the
lifetime of `Reattachable::At`, of a `Writer` and a `Header`, of `Active` and `OnceRun`. A
door written over `'cell` is written once for every half a value can be laid
in — the executing cell's region at `'here`, its scratch habitat at `'scratch`,
a placement's destination at a brand quantified over the build, `Storage` at
`'graph`.

The quantification is the proof. A placement's build is `for<'cell>`, so inside
it no brand of the executing cell is nameable: what it hands back can hold only
what it wrote through the writer it was given, operands that arrived at `'cell`
because the verdict pinned them, and `'graph`. `Active` is invariant in
`'cell` so a value never moves to a brand its constructor did not establish,
and `OnceRun` is invariant so every slot of a run is set at the run's own
brand; `OnceView`, which only reads, is covariant, and exists only once the
family is proven `Covariant`.

`Writer` and `Header` are invariant in `'cell` for one reason: each is a door
into one habitat half, and a door at one half's brand standing in for another's
would lay a value meant for a shorter-lived half in a longer-lived one's bytes
or slot. Every door that hands out both takes them off one region borrow, so a
writer and a header of different halves never meet in one call. A build's header door is at the
build's own `'cell` even when the destination is the executing cell, so a
header laid through it cannot be kept at `'here`; the step's own door, at
`'here`, is the one whose header the step keeps.

## `'severed`: a copy is a lifetime, not a check

A copied operand arrives at `'severed`, quantified over the build call with
`'graph: 'severed` and no relation to `'cell`. That absence is the whole
mechanism: the region part of a severed view cannot be embedded in the
destination, because nothing states it outlives `'cell`, so the only copy that
typechecks is a deep one through the destination's writer. The `'graph` part
embeds as it is, since `'graph: 'severed` is stated and `'graph` is not a
region's. Nothing is minted for a copy and no hold is taken, and the brand is
what makes that safe rather than a rule the embedder keeps. The pinned arm
arrives at `'cell` instead, because the mint has already folded the home's
reach into the destination's holds.

## `'their`: a producer names no brand of the consumer's

`deliver_scratch` builds in the consumer's scratch habitat through a closure
quantified over `'their`, the consumer's brand, with `'graph: 'their`
witnessed as a value and nothing else stated. Inside it the producer's own
`'here` is unrelated to `'their`, so a producer borrow cannot be embedded; and
`'their` is unrelated to the producer's `'step`, so nothing written can be
carried out of the call. The result comes back to the consumer at its own
`'scratch`, which is why this path has no verdict, no mint and no price: the
value holds nothing but what the build wrote and `'graph`.

## `'h`: a home header is read, never written into

A crossed operand's `HomeHeader` is at the operand's brand `'b`, and its one
door, `read`, hands the header to a closure quantified over `'h`, with
`'h: 'b` and `'graph: 'h` witnessed as a value, `&'b &'h &'graph ()`
([header.rs](header.rs)). The header is invariant, so a door handing it out at
`'b` would let a build write a destination borrow, valid at `'b`, into the
home's slot, where the home would read it back at its own brand. Inside the
closure the only things valid at every `'h: 'b` are the header's own contents
and `'graph` borrows, so nothing else can be written in, and the result cannot
name `'h`, so what comes out is at `'b` at most.

## `'home`: a read takes no proof of liveness

`'home` is `Ready`'s parameter: the cell whose region stores this value is
live, and its storage fixed-address, for all of `'home`
([carrier.rs](carrier.rs)). The step's doors fill it with `'step`, whose proof
is above. Every read rests on it and none re-checks, which is why "a carrier is
reachable only inside an `enter` scope" is a lifetime and not a rule. `Ready`
is covariant in it, since shortening a liveness claim is always sound.

## `'long` and `'short`: the covariance witness

`Covariant::shorten<'long, 'short>` takes the family at `'long` and returns it
at `'short`, with `'graph: 'long: 'short` ([reattach.rs](reattach.rs)). The
`covariant!` macro writes its body as the identity, which compiles exactly when
the family's shape is covariant in its brand and is refused for one holding a
`Cell` at it. The pair is the compiler's own variance check, exposed as a
trait, so that a pinned operand arriving at a shorter brand and a read view
shortening a longer-lived home are each a coercion the family has proven it can
take.

## `'dispose`: a cascade runs under no step

A release's disposal cascade runs outside any `enter`, holding the graph
exclusively for the cascade's length ([`Parked`](graph.rs)). No `'here` or
`'step` can be live under an exclusive borrow, so the bumps it drops have no
reader, and the scratch region it parks is handed back the same way `enter`
hands it back.

## The sealed claims, together

Every retype in the crate is one `retype` between a family's shape at two
brands, checked for size and alignment at compile time and otherwise trusted
([reattach.rs](reattach.rs)). What each use trusts:

- **The continuation and scratch-state slots.** Written at a cell's own
  brands, reattached by `enter` on that same cell. Proof: a region outlives
  every step in it, `'here` is invariant so nothing shorter was written in,
  the scratch reset is held off while the scratch slot is occupied, and no door
  onto a habitat was written in, since its `'step` outlives neither brand.
- **A once-written slot.** Set through an invariant `OnceRun` at the run's
  brand, read through a covariant `OnceView` at a brand the view's constructor
  proved shorter. Proof: the family is `Covariant`, so a shortened read takes
  no borrow in.
- **`Active`.** Erased by a constructor that established its region referents
  outlive `'cell`, re-anchored at the same `'cell` by an invariant marker.
- **A header slot.** Laid through an invariant `Header` at its habitat half's
  brand, reattached by `Header::get` at that half's brand of a later step or
  build, or at a borrower's `'lent` while the half is lent. Proof: what was laid names the half's own bytes, `'graph` (a
  `Storage` writer among it), storage the half's cell holds, or, for a scratch
  half, the write home's region — never a door onto a habitat, whose `'step`
  outlives no habitat brand; a region's slot travels with its chunks, no
  recycled region keeps one, and a scratch slot is cleared at every reset.
- **A peeked header.** Anchored by `CellGraph::peek` through `Active` at a
  brand its closure is generic over, between steps. Proof: the header's
  referents are any region header's (`Header::get`), and `peek` holds the graph
  shared for the call, so no verb can move or drop the region or release the
  holds they name; the closure is handed no door, so it lays nothing.
- **A home header.** Anchored by `HomeHeader::read` through `Active` at the
  `'h` its closure is generic over. Proof: the header is read only from a
  region the operand's reach names, so its referents are kept on the grounds
  the operand's own view is — minted into the destination for a pinned
  operand, held for the step for a copied one — and the closure can write in
  nothing valid at a shorter brand.
- **`Dormant`.** Parked as `MaybeUninit` and asserts nothing: a carrier at rest
  must be movable after its home's storage is gone. `redeem` establishes the
  claim before reconstituting, and the staleness argument in
  [graph/README.md](graph/README.md) is what makes that check sufficient.

What no lifetime proves: that the graph is not dropped with a cell still
reachable from outside. The brands cover that from the other side — nothing
branded leaves its `enter` — so there is nothing outside to reach from.
