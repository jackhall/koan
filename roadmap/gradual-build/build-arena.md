# The build arena

The drain's books outside the cell graph, settlements keyed on load content
and compositions keyed on syntax, with the evaluator reading what the build
decided.

**Problem.** The build tree is a cellgraph tree cell driven by two side paths
of the drain: the load door enters the build root through a raw `enter` with
no `Step` ([`load.rs`](../../src/scheduler/load.rs)), and a missed
specialization is entered off the ready stack, synchronously, through
`enter_beside`, ending `Quiesced` as no other cell may
([`drain.rs`](../../src/scheduler/drain.rs), `seek`). Its one step ends
`enlist()` ([`specialize.rs`](../../src/program/settle.rs)): it
settles nothing, and the evaluator re-derives the build's typing per read —
`bound_path`, retype-and-judge, ranking at a use no key covers. A frame's
typing context rides cellgraph's lend of a writable half, fixed at the
frame's creation ([tree cells](../../cellgraph/src/tree/README.md#death)),
so a `Shares` frame's lend is counted on its host's scratch until that scratch
resets and a lend can close a wait cycle no creation refuses. The drain
mutates the region header between steps through `peek`, a door cellgraph
documents as read-only, and the header is one record of several
modules' concerns — branch, table and home. A specialization is keyed on content
the run builds as readily as on the load's, so a loop handing a keyed callee a
module built each hop births a cell per hop and grows the agent tier by each
binding ([build storage](../../design/build-storage.md#settlements-and-compositions)); and
each parent bounds its own children, so the live set is the root bound times
the nested bound to the nesting depth.

**Acceptance criteria.**

- The build's books live in a **build arena** the agent owns, outside the
  cell graph, two kinds of node under one root. A **settlement** is the open
  facts of one frontier class settled under one binding, keyed on every name
  its seam binds, each name bound to content the frozen load names — a type
  whose in-root bit is set, a type value naming one, or a module whose content
  tree is closed; it holds its settled facts, its residual and its children's
  table, and replaces the specialization. A **composition** is the shape of
  one composed syntax and the facts that wait on nothing but that syntax,
  with its children's table. The arena has parent links, a borrower count
  that drops at the borrower's release, tenants included, a counted pin from
  a settlement to the composition whose bump holds a tree its binding names,
  and one budget over the bytes no frame holds, where a frame holds the arena
  node it borrows and every arena node a parent link or a pin reaches from
  it, evicted least recently used, leaf first, so an evicted settlement
  releases its pin before the composition it pinned is a candidate. The cell
  graph holds run cells alone, and the drain reads no cellgraph door between
  steps.
- No arena node holds a type. A settlement under the root rests its types
  and verdicts in the agent tier, where they outlive it; a composition, and
  every settlement under one, rests nothing there, and nothing a frame under
  a composition adopts enters it. A type composed code declares is run
  content: the frame mints it at the declaring statement, routed by its
  mint-site class, and it leaves the body as minted content. A fact names a
  type by its digest, which a load type resolves in the root, a composed
  declared type in the frame's chain once minted, and a binding under a
  composition against the handles its key carries; a type a fact derives
  under a composition is a term over those handles, which the frame mints
  into its own tiers once per binding event.
- No pointer laid in a region, a scratch, a parked state or a value names an
  arena node: a callable's reference is a handle, a slot and its generation,
  which the arena checks at each use and which names nothing once the slot
  is reused. A step reads its arena node's shape and facts through its
  cell's arena reference, at the step's own lifetime, and copies out indexes,
  digests and type handles alone; a site is an index per shape. A hand-off
  successor takes its frame's reference when the drain creates it, before
  the frame's release, and drops it at its own; a tenant's count runs from
  its creation to its release. So an arena node's bytes outlive every
  reference to them once its borrowers have released.
- The drain keeps the books: a frame's request carries its key, binding and an
  image of the binding's types laid at no cell's brand; the books answer hit
  or miss; on a miss the drain runs
  [the load's fact function](../../src/build/README.md#the-one-walk) in the
  settlement with the agent's store, synchronously, and births the frame at
  the settlement's rest. Nothing homed in a run cell is referenced by an
  arena node.
- A settlement reads its parent's facts through the arena's parent link, and
  a fact is settled once per settlement.
- A frame whose binding at its entry names run content births no settlement:
  it runs the fact function in its own activation over the carried types,
  into its own tiers, once per binding event. A frame reuses the decision
  its tail chain or a caller made for the same body under the same arena
  node, up to the first binding event whose carried types differ, so a
  tail loop through mutually recursive bodies decides each body once, and
  a property law holds every reuse equal to a decision made from scratch. A statement seam births no settlement either, since what entry
  content determines at it is settled at the entry seam; the rest is run
  content, which the activation decides at the statement's binding event,
  as it decides a capture an encloser binds at a statement. A loop calling a keyed callee on load
  content holds O(1) settlements, and the agent tier grows by distinct
  load-content bindings alone.
- The evaluator re-derives no typing the build decided. At every site it
  reads the recorded decision — the candidate list in rank order with its
  elimination table, the solution points, closed contributions, the tail
  decision, the denoted type, the instance solution, a path resolved closed
  under the module's content, the route (hop, seek or cold), the target type
  of a retype — and `bound_path`, `denoted`, ranking over closed candidates,
  re-solving a selected candidate's group and the overlap check at an `EVAL`
  of a written quote are gone. What runs at run time is admission of a
  *maybe* candidate against carried types, an unsettled `satisfies`, a
  barrier's crossing, the view door's fit over a tree the build left open,
  the key's digest and the re-stamp itself.
- The frozen load holds the facts of every unit whose frontier is empty, each
  unit's binder split and each use's narrowed candidate list, so an
  activation evaluates only its dependent units.
- Every run cell born under an arena node carries that node's reference for
  its life, tenants included, and reads its shape and decisions through it;
  no cellgraph lend carries a typing context.
- A callable carries a handle to the arena node its birth ran under and the
  requests that built each settlement of its chain: the key, the binding's
  handles, the content digest of each module the binding names and, where
  that module's tree lives in a composition, that composition's code value.
  The frame lays them once in its region at its first closure birth; they
  are no part of the callable's content, and the closure's settled bits
  count the stamp's handles among its captured types. A call through a stale
  handle replays them top-down through the miss path, borrowing each level
  it reaches, hit or rebuilt, and taking a composition's pin as it re-finds
  it, until the frame is born, so the settlement it reaches is the one its
  birth ran under; a non-keying callable borrows the settlement its handle
  names; and a callable born in composed code carries its code's syntax. The
  one birth still cold is a call keyed on a `FOR ALL` name past a header or
  a barrier.
- A settlement binds a module by a closed tree: under the root one the load
  computed, and under a composition one the load or any composition
  computed. Each layer indexes the closed trees it computes by their
  identity, the digest the run computes for the content, and a request
  resolves a module's digest to its tree in the load's table, then each
  ancestor settlement's, then the pinned composition's. A written callee
  bound to a module composed at run time runs in its activation, so nothing
  composed enters the agent tier; an activation binds a module by the
  value's digest; and a cold birth binds with the precision the settlement
  would have. A settlement's facts, and the refusals it faults at its entry,
  are a function of its key and its content alone, never of when they were
  settled.
- A refusal an arena node reaches faults the asker at the resumption point,
  worded as the load words it.
- Build memory no frame holds is bounded by the budget, and what frames hold
  is linear in the frame stack's distinct arena nodes and the compositions
  they pin, not in nesting depth.
- A code value composed at run time has its shape, and the facts that wait on
  nothing but its syntax, in a composition keyed on the code's syntax digest,
  a child of the arena's root. The drain finds it by digest at each `EVAL` of
  the code and at each call of a callable born in it, a hit stamping the
  composition's use and a miss laying the rewritten syntax in the
  composition's bump and running the fact function there. Segments number per
  shape, and a key under a composition digests the syntax digest beside the
  segment. The `$` names and `\` marks are captures at the code's entry seam:
  they enter no key, and a fact waiting on one settles in the frame's
  activation whatever its binding names. A loop composing fresh code each hop
  holds O(1) compositions.

**Directions.**

- *The budget's unit — decided.* Bytes: every arena node is its own bump,
  sized at its rest and evicted whole, and a composition's shape varies in
  size by orders of magnitude, so a count of arena nodes would bound
  nothing. A held arena node is no candidate, where a frame holds what it
  borrows and what a parent link or a pin reaches from it, since evicting
  any of them frees nothing before the frame releases; so a replay that
  borrows each level it reaches never loses one while building the next, a
  chain exceeds the budget only while a frame uses it, and a composition
  pinned by an unheld settlement is evicted after it, leaf first. An arena
  bump takes the `!needs_drop` gate cellgraph's `Writer` enforces as its
  own, since the arena lies outside the cell graph.
- *`EVAL` shapes — decided.* As a module splits: a module's shape is its
  text, its captures bind at their statements, and its content is the code
  digest over the captures' digests. So composed code's shape is a function
  of its syntax alone, cached in a composition under the budget; its `$`
  names bind at the `EVAL` as a module's captures do; and the value's digest,
  syntax over bindings, is what code equality already reads. A code value
  crossing cells carries what it carries now, its syntax and bindings, and
  the shape is re-found wherever it runs. Rejected: the value's home region
  at its birth, which must say what crosses with the value and shapes code
  never run; the `EVAL` frame's region, which rebuilds per `EVAL`.
- *Where a composed body's types live — decided.* Nowhere in the arena. A
  composition's bump is evicted, and cellgraph's lend gives `'lent` no
  relation to `'here`, so a region node could never reference a type laid
  there; and the exit walk adopts minted content alone, so a type a composed
  body declares is minted by the frame, as a module's types are minted where
  it is tied rather than kept in its text. Composed bodies then classify as
  written ones do, no tier or brand is added, and verdicts follow the rules
  they have. Rejected: types in the composition's bump read as a scratch
  tier, which would make every composed body caching and write verdicts into
  the composition after its rest.
- *A term — decided.* The lattice gains a term, a plain-data copy of a
  type's nodes, which a chain exports and mints through the transfer
  adoption uses, so the one descent over a type's nodes stays in the
  lattice. Rejected: a koan-side term minted through the chain's
  constructors, which copies the lattice's rebuild table; and an image
  branch in the arena node's bump, which puts type nodes in an arena node.
- *A composition's lifetime — decided.* By the invariant that no byte in a
  half or a value references an arena node, which is what keeps a settlement
  sound already: a step reads the shape at its own lifetime, not at `'lent`,
  since `'lent: 'scratch` would let a scratch keep it, and not at `'graph`,
  since a parked state stores any `&'graph`
  ([lifetimes](../../cellgraph/src/lifetimes.md)). What crosses a park holds
  a segment, a site index or a digest: the activation's shape and load, an
  evaluation's and a block's birth, the activation store's records, a composed
  code value, which holds its syntax and digest and no shape, and a callable
  born in composed code, whose body is a digest and a segment. Written code
  reads through the same door, one array index per read, rather than a second
  evaluator path. A settlement whose binding names a tree in a composition's
  bump pins that composition as a borrower, since a child settlement born
  later reads its parent's binding through that shape. Rejected: the count
  following the bytes through splice and tenancy, which is the count
  [build storage](../../design/build-storage.md#why-a-reference-is-free)
  refuses.
- *An arena node's self-reference — decided.* An arena node owns the bump
  its records, and a composition's `Shapes`, are laid in and reference, and
  a settlement's records reach its parent's bump and its pinned
  composition's as well. So each arena node is a `self_cell` whose owner
  holds its bump and a counted pointer to each arena node its records
  reach, and a step reads the dependent at the step's own lifetime through
  `borrow_dependent`; the dependent is covariant, and koan writes no
  `unsafe`. Rejected: one sealed `unsafe` erasing the root's lifetime and
  reattaching it per step, whose claim the borrow checker holds through
  `self_cell` instead; the bump as a cellgraph region under the named lend,
  which breaks cellgraph's abstraction or costs a slab cell per composition,
  and whose borrowers would be any cell of the tree; an index-based
  `Shapes`, the largest refactor, of which the site index alone is kept.
- *Warm types under a composition — deferred.* To
  [warm composed types](warm-composed-types.md): a settlement under a
  composition rests nothing, so its invariant sites are warm through a
  `Shares` host's scratch alone and a `Fresh` loop mints them each hop, as a
  run-content binding does.
- *What a callable carries — decided.* The requests that built its chain,
  replayed on a stale handle, so an evicted settlement is rebuilt through
  the miss path rather than the frame born cold, and the rebuilt settlement
  has the parent, key and binding the chain had, so sharing with new seeks
  returns. A level's request is its key, its handles and the content digest
  of each module it binds, since a key is a hash and the handles are types,
  so neither re-finds a tree; a level bound to run content or to `$` and `\`
  captures contributes nothing. The stamp's handles travel as the closure's
  captured types do, through exit adoption, the hand-off's adoption and a
  crossing between agents, where the handle is voided and the stamp replays.
  Rejected: a count carried by values, which needs a set in every region
  header, a check in every cross-region copy, a merge at a splice and a drop
  at disposal, counts at the region's precision and makes the budget soft; a
  flat settlement under the re-found composition keyed on the callable's
  captures, which leaves a captured function cold, gives an inner body's
  facts a second parent, breaks "a capture never enters a key" and settles
  facts that depend on eviction history; and a chain of key digests alone,
  since a closure's captures hold what its body reads, a type derived from a
  keyed name as readily as the name, so an enclosing settlement is not
  rebuildable from them.
- *An activation's memo — decided.* A frame carries a decision memo, one
  entry per body it or its tail chain decided, at most eight, the least
  recently used dropped, handed on as a call hands on its digest memos: by
  pointer for a spawned call and a `Shares` hop, and copied for a `Fresh`
  hop, whose predecessor is released first. An entry is keyed by its body
  and the arena node the frame is born under, and holds one layer per
  binding event, keyed by the event's site and the digests bound there; a
  frame reuses layers while each event's key matches and decides from the
  first that differs, so a hit checks every input the walk reads. Rejected:
  the last hop's records alone, which a mutually recursive tail loop never
  hits; and an entry keyed by its entry bindings alone, which reuses a
  statement's decision when the statement binds a different type.
- *How a binding crosses — decided.* As an image: the asker lays the binding's
  types into an owned bump the drain carries with the request, and a
  settlement under the root relays them into the agent tier at its first
  step. Under a composition nothing is relayed: the facts name the binding by
  its digest, and each frame resolves it against the handles its key carries.
- *The layers — decided.* The load decides every unit with an empty frontier,
  records each unit's binder split and narrows each use's candidate list; all
  three are the fact function's frontier bookkeeping.
- *A wake over a shared frontier — decided.* No frontier is laid flat
  ([the store](../../src/build/README.md#the-store)), so a store's wake
  memoizes, per binding, whether each frontier it reaches meets the binding,
  as the keys fold memoizes seams. A frontier is empty exactly where it holds
  no unknown, so the load reads emptiness off the frontier itself. A
  settlement reads a frontier's emptiness and seam relative to its binding, since a
  resumption's frontier may name unknowns its parent's binding already binds.
- *A module's binding — decided.* By the tree its layer computed, under the
  root and under a composition alike, since a settlement's facts and the
  refusals it faults at entry must be a function of its key and content and
  never of when they were settled: a path the tree resolves closed stays
  unknown under a digest, so a refusal raised once could go unraised after
  an eviction. Under a composition the tree names bodies in the computing
  composition's shape, which the settlement pins and the stamp's code value
  rebuilds. A replay holds a module's digest and no tree, so each layer
  indexes the closed trees it computes by their identity, which for a closed
  tree is the content's digest, and the request resolves the digest through
  the load's table, its ancestors' and the pinned composition's; `Print`,
  the tree's identity in `scope/tree.rs`, is renamed `Identity`, since it is
  a digest and no rendering. Under the root a settlement binds load content
  alone, so a written callee bound to a module composed at run time runs in
  its activation and nothing composed enters the agent tier. An activation
  binds by the value's digest, since the run reads past the tree's horizon.
  Rejected: digests under a composition, which lose every fact past a member
  of a module built in composed code; copying the tree or the shape into the
  settlement, since the tree's segments name the computing shape and a shape
  copy is a second composition; a tree handle in the stamp, which names a
  load tree alone, since a tree in a settlement's or a composition's bump is
  rebuilt at a new place; and a tree copied into the stamp, which still
  needs the computing composition. The build
  module's law that three stores agree stands as tested, all three under a
  module's digest and the load's store and a layer under its tree; at ship
  the settlement store joins the tree pair.
- *The syntax digest — decided.* A composition is found by a digest over the
  code's lowered syntax, computed by slice and splice from their operands'
  digests rather than by the shape builder, whose code digest is over
  resolved code and would cost a hit a shape build. The definition is
  provisional until composing by parts shows it digests incrementally.
- *The `src/build` partition — decided.* By ownership: the arena, its
  budget, its eviction and the drain's books are the scheduler's runtime
  state, as the ready stack is, and the fact function the drain runs on a
  miss reaches it as a door `program` hands it, as the load's step does; the
  stores, keys and folds stay `build`'s, so the import rules stand. The file
  split inside `build` is the work's.
- *Elaboration evaluating deterministic code — deferred.* To
  [code names](../metaprogramming/code-names.md): an arena node has no step
  to park, so the sub-run it would need is that item's.
- *Cross-agent seeding — deferred.* To [agents](../rewrite/agents.md).

## Dependencies

[Type tiers](../../design/type-tiers.md) and
[the scheduler](../../src/scheduler/README.md#specializations) describe the
mechanisms this replaces; [build storage](../../design/build-storage.md) pins
the end state. A settlement, a composition and an activation run
[the load's fact function](../../src/build/README.md#the-one-walk) over
[its store](../../src/build/README.md#the-store).

**Requires:** none.

**Unblocks:**

- [Run cells alone](../../cellgraph/roadmap/run-cells-alone.md) — cellgraph's
  build-tree doors go once no build cell is a cell.
- [Agents](../rewrite/agents.md) — the per-agent build arena.
- [Setting the build budget](bound-settings.md) — the budget as a setting.
- [A birth solved once](birth-solved-once.md) — the frame request carries the
  call's solution.
- [Code splicing](../metaprogramming/code-splicing.md) — the composition a
  run-built shape lives in.
- [Warm composed types](warm-composed-types.md) — the types a settlement
  under a composition derives, kept warm.
