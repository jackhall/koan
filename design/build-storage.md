# Build storage

Where the work of deciding a program lives: the frozen load every agent
shares, the tier an agent keeps for what its build rests, and the arena its
build cells are evicted from. The story across
[the type lattice](../lattice/src/types/identity.md#storage-a-root-and-its-branches),
which keeps each tier's table, [the build](../src/build/README.md), which
decides, [the scheduler](../src/scheduler/README.md), which runs, and
[type tiers](type-tiers.md), which says where a type the run builds lives.

## Three tiers and the run's halves

| tier | brand | written | holds | released |
|---|---|---|---|---|
| frozen load | `'load`, one per program, `Sync` | by the load alone, frozen at its end | load content: every type the load builds, its verdicts, the shape, builtins, symbols, sources, the facts it decides | at the program's end |
| agent tier | `'agent`, one per agent | insert-only, by a build cell at its rest | what the agent's build cells rest: their types and verdicts; verdicts over load pairs the load's table lacks; the agent's symbol overlay | at the agent's death, whole |
| region, scratch | `'here`, `'scratch` | as [type tiers](type-tiers.md) says | runtime types: what a frame builds, mints in a loop, a deepening list | with the region; at the habitat's reset |

The brands are ordered, `'load: 'agent: 'here: 'scratch`, and a chain reads
the tiers longest-lived first. Every tier below the frozen load is one agent's
own. A node references nodes of its own tier or a longer-lived one, by
covariance alone; no tier is read at a brand unrelated to the chain's own,
and the lattice keeps no apparatus for one.

## Why a reference is free

A reference from a node into another tier is sound when nothing can free its
target before the referencing node dies. Three events free a tier that a cell
owns: an **eviction**, which recycles a cell's region under a budget; a
**splice**, which moves a frame's region bytes into an ancestor that outlives
the frame's disposal; and a **tenancy**, in which a tenant lays into its
host's region and so into its host's life. A reference into an evictable tier
would need a count that follows the bytes through all three, carried by every
pin door and read by the budget's candidate test, and a sealed `unsafe` would
only hide that count, not remove it. Immutability is not the ground: a frozen
tier that could be evicted would dangle all the same.

The frozen load and the agent tier are freed by none of the three. The load is
released at the program's end, the agent tier at its agent's death, and every
cell of the agent dies first. So a region node references a rested type as it
references a load type, and a rested type is never relaid, never counted and
never read through a sealed claim. The run's own halves keep the rules
[type tiers](type-tiers.md#a-node-lives-in-a-tier-its-children-outlive) gives
them.

## The frozen load

Everything the program's operator run reads for its whole life, laid in a
region the program owns above every agent. The agents run under a
`std::thread::scope` that region outlives, each holding `&'load Frozen<'load>`,
and the borrow crosses threads because `Frozen<'load>: Sync` holds by
construction: no `Cell`, no `RefCell`, no `Rc`, no thread-local and no bump
reference anywhere in it.

| part | holds | `Sync` by |
|---|---|---|
| the root | the load's type nodes and its verdict table | a node index that is a run of (digest word, node) slots, probed as the building table is but never written; the verdict table read-only |
| the shape | the parsed body and its sites | plain after the load, since every decision is a fact |
| builtins | signatures, knot weights | computed at the load, stored plain |
| symbols | the text of every source name, keyed by digest | read-only; a name `EVAL` parses goes in the agent's overlay |
| sources | the file registry | owned by the load, so any agent resolves a file id |
| facts | what the load decides, keyed by site | plain tables |

**`Tied`.** A sealed component and its members are a cycle: each member holds
its component, and the component holds its member table, laid first relative
and rewritten absolute once the members are interned. The writer is an
`AtomicPtr<()>` beside a fixed length, so the component is `Send + Sync`,
covariant through the `PhantomData<&'run [T]>` its holder keeps, and its read
stays the lattice's one `unsafe`. Relaxed ordering suffices: every agent is
spawned after the load's last write.

**Verdicts.** The load records every verdict it computes and evicts none. The
load cannot grow after its freeze, so the table is sized once and is complete
for everything the load asked; a verdict a run asks over two load types that
the load never asked is recorded in the agent's tier.

**What the load decides.** The load pushes into the frozen load as much as the
declarations decide, in layers each agent then reads instead of recomputing:
the load's types, schemes, signatures, shapes, builtins and symbols; a
decision at every site whose types are all concrete, no binder reachable; the
closed verdict table; for each generic body, which binders each site's types
mention, so a specialization evaluates only its dependent sites; and dispatch
candidates per site, narrowed by the load-known argument types. What it
cannot hold is a node that mentions a binding's carried type, a verdict over
one, a decision at a dependent site, and anything `EVAL` builds.

**Covariance.** An agent's graph borrows storage at its own `'graph`, with
`'load: 'graph`, and a node of any cell embeds load nodes through that
covariance, as a chain embeds its root. The root and its content stay
covariant in their lifetime.

## The agent tier

An insert-only tier of nodes and verdicts laid in a bump the agent owns. A
node is handed out as `&'agent StoredNode<'agent>` and never moves, nothing in
the tier is released before the tier is released whole, and the tier goes
with its agent. Two things intern into it: a build cell at its rest, laying
the types its facts name, the binding's nodes it relaid from its image, and
the verdicts it reached; and a run's verdict over two load types the load's
table lacks. A run cell reads the agent tier and interns nothing into it: a
type the run builds lives in the run's halves. The agent's symbol overlay,
the names its `EVAL`s parse, sits beside it; a render reads the overlay, then
the frozen table, and the two never disagree, since a symbol is its text's
digest.

## A build cell

A build cell is one frontier class under one binding, keyed on every name its
seam binds, and each of those names is bound to content the frozen load
names: a type whose in-root bit is set, a type value naming such a type, or a
module whose content tree is closed, which the load computes. A binding to
anything else births no build cell; the site it reaches is decided in the
frame's own activation, over the carried types, in the frame's own tiers
([the build arena](../roadmap/gradual-build/build-arena.md)).

| the key names | decided in | lives |
|---|---|---|
| load content | a build cell, its types rested in the agent tier | the agent's life, shared by every frame under the binding |
| run content | the frame's activation | the frame |

A build cell's facts, residual and table live in the **build arena**: the
agent's own, outside the cell graph, with parent links, a borrower count that
drops at the borrower's release, and one budget over the whole arena evicted
least recently used, leaf first. Its types are not in the arena: they rest in
the agent tier and outlive the cell. So a loop whose statement binds
load-named content holds O(1) build cells, since the budget evicts their
facts, while the types those cells rested stay for the agent's life. The cell
graph holds run cells alone, and the drain keeps the build's books.

## Agents

An agent is a thread with its own cell graph, drain, build arena, agent tier
and symbol overlay. Agents share the frozen load and nothing else: no build
data crosses between them after the load, so there is no mirror, no shared
books and no cross-agent pending map, and what an agent recomputes is exactly
what the load left undecided. A value crossing between agents adopts its
carried type into the receiver's chain, free when the type is a load type,
since its handle says so. The program's memory is the frozen load plus, per
agent, its budget and its stack
([agents](../roadmap/rewrite/agents.md)).

## Open work

- [The build arena](../roadmap/gradual-build/build-arena.md) — the arena,
  its keys and its budget, and the activation as a decision's other home.
- [Agents](../roadmap/rewrite/agents.md) — threads sharing the frozen load.
