# The scheduler as a veneer

What the [scheduler](../../src/scheduler/README.md) has to become before a koan
program runs on it: a ready stack and a protocol, with nothing a neighbour
already keeps.

**Problem.** The scheduler in the tree keeps state its neighbours already keep
and passes through doors its callers should never see.
[`submit.rs`](../../src/scheduler/submit.rs) holds a table of units, dependency
counts and edges, yet a [shape](../../src/scope/README.md#visibility) already
orders a body's units, and a table fed by a step grows by every statement of
every call and never gives an entry back. The work queue in
[`drain.rs`](../../src/scheduler/drain.rs) is first in, first out, so two
children asked in one park run step by step in turn — their effects interleave —
and a recursion asking two children per level holds a whole level of cells live
at once. `Provenance` in
[`continuation.rs`](../../src/scheduler/continuation.rs) records a place and a
consumer that are the same handle in every constructor. `Step` in
[`action.rs`](../../src/scheduler/action.rs) re-exports `alloc_into`, `lift`,
`keep`, `redeem`, `receipt` and the cell's own and its consumer's handles, so a
step has to know which door a result bound for storage goes through, which a
read-only one does, and how a tail hop's arguments are kept and redeemed; and
the producer picks the delivery door though only the consumer knows what the
result is for. `State` and `ScratchState` are enums the scheduler declares for
the layers above it, and a drain succeeds only on an empty graph, which a
program's root can never leave it.

**Acceptance criteria.**

- One operand-free build closure, quantified over its destination's brand,
  serves both `deliver_scratch` and an operand-free `alloc_into`, and a test per
  `Use` reads `finish_fresh`'s result back at the brand the
  [delivery table](../../src/scheduler/README.md#delivery) gives it.
- What a write through a pinned view does is pinned in `cellgraph`: a `Fresh`
  child that pins a `Cell`-bearing structure of its parent's and stores in it a
  reference homed in itself is a `compile_fail` doctest, or is ruled out by an
  obligation `Reattachable`'s safety contract states, and the test is on
  `cellgraph`'s Miri slate.
- A `Scheduler` view owns a ready stack and a request buffer and nothing else
  that outlives a step. `submit`, `edge`, `Unit`, `UnitId`, `Birth` and
  `Submissions` do not exist and `src/scheduler/submit.rs` is gone.
- `Scheduler::run` takes a root work, the cell it is born under and its
  placement, succeeds when that work ends, and reports
  `DrainStalled::Unfinished` when the stack empties first. `Graph::root` hands
  out a storage-only slab cell no drain enters or releases, and a drain over a
  graph holding one succeeds with the root still live.
- Two children asked in one park, each with children of its own, record their
  steps one subtree after the other; a recursion asking two children per level
  peaks at live cells in proportion to its depth; and a hundred siblings asked
  in one park run with one of them live at a time.
- `Provenance` holds a parent, an optional slot and `Use`, and a home, and no
  second handle naming the parent.
- `Step` exposes no `CellHandle` and no carrier door, held by the boundary test
  reading its public methods. A step hands `spawn` and `tail` the new cell's
  state at `'here` and the new cell's first step receives it at its own
  `'here`; `results` yields a scratch fill at `'scratch` and a carrier fill at
  `'here`.
- A test per cell of the [delivery table](../../src/scheduler/README.md#delivery)
  pins the door each end takes under each `Use`, and a child asked with
  `Forwards` builds in its spawner's home.
- The scheduler takes its step state as one bundle parameter and declares no
  state enum; the ones its tests use live in a test bundle under
  `src/scheduler/tests/`.
- A ten-thousand-hop loop at each placement still peaks at three live cells
  with no more heap than a hundred-hop loop, and the scheduler's Miri slate is
  clean.

**Directions.**

- *Whether one build closure serves both delivery doors — open.*
  `deliver_scratch`'s build takes a `'graph: 'their` witness argument and
  `alloc_into`'s takes none, so the closure `finish_fresh` accepts needs a
  witness the veneer can mint at either door. The alternative is two closures,
  one per door, which makes the producer say twice what it builds.
  *Recommended:* compile this first, before any other change, since the three
  ends rest on it.
- *How a state is put to rest and woken — open.* The veneer has to turn the
  state `spawn` and `tail` are handed into dormant carriers and back. Either the
  step bundle supplies the pair of conversions, which become the only code
  above the scheduler that names a carrier door; or a birth state is a fixed
  shape the veneer knows — words, program storage, one value carrier and one
  environment carrier. *Recommended:* the bundle's pair, since a fixed shape
  makes the scheduler name what an environment is.
- *What closes a write through a pinned view — open.* A child that pins its
  parent's activation holds `Cell` slots of a longer-lived region at its own
  `'here`, and `Reattachable`'s contract speaks only of layout. Either the
  contract gains an obligation — no interior mutability reachable at the region
  brand — or koan hands a child only a read-only view and the hazard stays
  documented in `cellgraph`. *Recommended:* settle which with the test the
  criterion names, before the [top level](top-level-on-the-scheduler.md) hands
  an evaluation its environment.
- *The order of the ready stack — decided.* Last in, first out, with a request
  born when it is popped. Depth-first order is the order of a sequential
  language, it bounds the live cells by the current path, and it makes a tail
  successor and a woken consumer ordinary pushes.
- *What becomes of the submission table — decided.* It is deleted. A shape
  numbers its body's units so each follows what it reads, an outside event
  reaches a program only through a monad the shape can see, and a depth-first
  drain performs what it is asked in the order it is asked, so no count and no
  edge ever decides anything at run time.
- *Where a root comes from — decided.* A storage-only slab cell the graph's
  owner takes from `Graph::root`, and the root work is born under it. A root
  that was itself the work would be a cell the drain runs and must not retire,
  and a second call over it would have to re-arm a finished cell, which
  `cellgraph` allows only from inside `enter`.
- *Whether tenancy stays — decided.* It stays. The top level's root work is a
  tenant of its root, and `Shares` rests on it: a callee that builds structure
  its result embeds, and a loop threading state it built itself, which `Fresh`
  siblings would copy whole on every hop.

## Dependencies

**Requires:** none — the substrate and every door the veneer wraps ship.

**Unblocks:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — the body runner is a root work over this drain.
