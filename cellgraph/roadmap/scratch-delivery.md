# Delivery into a parked cell's scratch

The producer half of a push: a step files a value where another cell finds it on
waking.

**Problem.** The scratch habitat goes back whole at the *first `enter` that
finds no scratch continuation at rest over the bump* (`enter` in
[../src/graph.rs](../src/graph.rs)). Whatever a cell carries across a park in
scratch it lays down during the step and names from the scratch continuation it
stores, and that stored continuation is what holds the reset off. So a cell that
parks round after round never sees its scratch go back: each round's
continuation is laid over the bytes of the rounds before it, and the reset sits
at entry, when what the continuation names is about to be read. The one moment
the bump could go back between rounds — the end of the step, where both
continuations are back on the cell (`StepContext::drop`) and no borrow of the
regions survives — is not one the substrate acts at, and a cell that parks
storing nothing holds its scratch bytes until its next entry or its death.

The placement doors are the other half of the gap. `alloc_into` builds a *new*
value in a destination's region and hands the carrier back to the producer;
nothing writes into a structure that already exists in another cell, so a
producer cannot file its result in a slot the consumer laid out. Filing it
itself would mean naming the consumer's `'here` or `'scratch` from outside its
step, which is a lengthening reattach of an invariant family — the shape
[../src/graph/README.md](../src/graph/README.md) refuses everywhere else. So an
embedder that wants a producer to die at `Done` must keep the at-rest handle
somewhere outside every cell, which is the free-standing envelope the crate is
built not to have.

**Acceptance criteria.**

- The scratch habitat goes back at the end of a step rather than the start:
  `enter` runs the step, drops the context, and resets the write home's scratch
  bump when nothing at rest names it — no scratch continuation and no receipt
  run, the cell's own or any of its tenants'. A cell that parks with neither
  holds no scratch bytes while parked.
- A step registers a **receipt run** by slot count, and the substrate lays the
  run down in the write home's scratch habitat at the end of the step, after
  that reset. A registration replaces the run at rest and refuses one whose
  slots are not drained. A cell that parks round after round on receipts alone,
  with no tenant naming its bump, starts every round on an empty bump.
- The run is the substrate's own type, generic over two families — a scratch
  value and a carrier — which the graph names as one **bundle**: a third
  embedder type beside the continuation and the value, declared as a trait with
  a family at each position, taking one graph parameter with a
  delivers-nothing default. A slot is empty, a scratch value or a `Dormant`.
  The embedder constructs no slot and writes none; the outstanding count is the
  run's.
- A `deliver` door fills one slot of another cell's run from a producer's step,
  in one of two ways: it builds a value with no operands in the consumer's
  scratch habitat and stores it, or it stores a `Dormant` the producer already
  holds. The producer names no brand of the consumer's, the door answers whether
  the run is now complete, and it refuses a `Stale` consumer the way
  `alloc_into` does, a consumer with no run at rest, and a slot already filled.
- The consumer's next step takes the run at `'scratch`: a scratch slot comes
  back as its value, and a carrier slot comes back redeemed, as a carrier at the
  step's brand or as redeem's refusal.
- A delivered value is subject to the habitat's rule: a compile-fail case shows
  a scratch-delivered value cannot be embedded in the consumer's storage, and
  one shows a producer cannot hold or return anything at the consumer's brand.
- The Miri slate covers the delivery shape under tree borrows: a slot written by
  one cell's step, read back by the owning cell at a fresh `'scratch`, across a
  park, for a slab home, a tree home and a tenant's host.
- A tenant's run rests in its host's scratch and holds the host's reset off as a
  tenant's scratch continuation does, by a count moved at the tenant's step end
  and at its death. A bump some tenant names goes back at the first step end
  that finds every such slot empty, and at the host's retirement whatever it
  holds.
- `cellgraph/observe/perf.csv` carries the delivery door beside the placement
  doors it is priced against.

**Directions.**

- *Where the reset happens — decided.* At the end of `enter`, on the condition
  the entry reset already uses, widened by the run: nothing at rest names the
  bump, the cell's own or a tenant's. An entry reset can only ever find a bump
  whose contents are about to be read; an exit reset finds the one moment a
  re-parking cell's scratch is dead, between the last read of one round's
  receipts and the laying down of the next. Emptiness of the slots stays the
  whole condition and the only signal — an embedder-declared "reset now" flag
  would admit the one unsound combination, a reset while a stored continuation
  still borrows the bump, that the slot shape makes unrepresentable.
- *Who lays the run down — decided.* The substrate, at step exit, from a count
  the step registered. A run the step laid down itself would sit in the bump
  before the reset and be named by a stored scratch continuation, which is
  exactly what holds the reset off. It follows that the run rests in its own
  slot on the cell beside the two continuations, and `deliver` finds it there:
  no trait on the embedder's scratch family, and no re-anchoring of an embedder
  value at a brand the consumer's step did not mint.
- *The receipt's shape — decided.* One run, one slot type, the substrate's:
  empty, a scratch value, or a `Dormant`, generic over the two families, which
  reach the graph as one bundle rather than as two parameters of their own. The
  bundle carries the `DropFree` bound both families need — a delivered value
  lands in a bump — so no such bound propagates into the graph, the cell or the
  pools, and it keeps the run's own family at one parameter, which the
  reattachable macro already expresses. A
  `Dormant` carries no region brand ([../src/dormant.rs](../src/dormant.rs)), so
  both kinds rest at `'scratch` and two runs would separate nothing; and a
  consumer parked on several producers can await a mix, so completeness across
  two runs is a conjunction the door would have to answer anyway. One carrier
  family per graph is the limit this accepts. A bare signal — an embedder's
  "re-read your slot" token — is a scratch fill whose build allocates nothing.
- *What goes by which fill — decided.* The scratch fill is operand-free, so it
  carries a result built fresh that the consumer reads and does not embed in
  storage; it lasts for as long as the consumer's scratch continuation names it,
  across any number of steps, and never past the cell. Everything else goes as a
  `Dormant` made by the doors that already exist — `alloc_into` with operands, or
  a tenant's own `writer` and `lift`, then `keep`: a result bound for the
  consumer's storage, one that borrows data already there, and one a tenant
  wrote at its own `'here`, a brand a build quantified by `deliver` cannot see.
- *What `deliver`'s slot write costs in `unsafe` — decided: an argument, not a
  site.* Every write on the delivery path is safe code — the writer writes
  through a shared borrow of the bump, the erase forgets a lifetime, the slot
  write is an interior-mutability write of known layout. What the door adds is
  the existing re-anchor at two new call sites: the run's own spine, and a
  delivered scratch value the consumer takes. Each stands on the argument
  `scratch_continuation` already makes — the referents are chunks of the write
  home's scratch bump, pinned to its table index and handed back only at a step
  end that found nothing naming it, the run among those things — with one
  clause added: the value was erased by another cell's step, which could have
  built it nowhere but through the consumer's own scratch writer, since the
  build takes no operands and its brand is quantified by the call, and a cell
  whose run is at rest is not executing.
- *A scratch bump per tenant — decided against.* A tenant's scratch stays its
  host's. A bump shared with a parked tenant rarely finds every slot empty, so
  it often lives as long as the host's storage does; what the habitat guarantees
  regardless is that scratch is never pinned, spliced or absorbed, so every byte
  of it is freed no later than the host's retirement. A bump per tenant would
  buy earlier resets at the price of a birth and a death per tenant in the
  region table, for the same bytes freed at the same retirement.

## Dependencies

**Requires:** none — the [scratch habitat and tenant
cells](../README.md#the-cell) ship.

**Unblocks:**

- [Scheduler on cellgraph](../../roadmap/rewrite/scheduler-on-cellgraph.md) — its producers die at `Done` and file their results in their consumer.
