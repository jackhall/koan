# Build storage

Where the work of deciding a program lives: the frozen load every agent
shares, the tier an agent keeps for what its run rests, the instances its
frames make, and the depot its compositions are evicted from. The story across
[the type lattice](../lattice/src/types/identity.md#storage-a-root-and-its-branches),
which keeps each tier's table, [the build](../src/build/README.md), which
decides, [the scheduler](../src/scheduler/README.md), which runs, and
[type tiers](type-tiers.md), which says where a type the run builds lives.

## Three tiers and the run's halves

| tier | brand | written | holds | released |
|---|---|---|---|---|
| builtin image | `'static`, one per binary, `Sync` | by no one: generated Rust source, compiled in | the lattice's fixed handles, every builtin overload's shape type, `Result` and `Error`, and the texts of the names they hold; no verdicts | never |
| frozen load | `'load`, one per program, `Sync` | by the load alone, frozen at its end | load content: every type the load builds, its verdicts, the shape, builtins, symbols, sources, the facts it decides | at the program's end |
| agent tier | `'agent`, one per agent | insert-only, by a run's verdict over load or agent types | verdicts over load pairs the load's table lacks; the agent's symbol overlay | at the agent's death, whole |
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
never read through a sealed claim. A depot slot is freed by eviction, so no
byte laid in a region, a scratch, a parked state or a value references one:
a step reads its depot slot's shape and facts at its own lifetime and copies
out indexes, digests and type handles alone, which is what lets the borrower
count drop at release, and a borrowed depot slot is never evicted. The run's own halves keep the rules
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
| the root | the load's type nodes and its verdict table, resting on the builtin image | a node index that is a run of (digest word, node) slots, probed as the building table is but never written; the verdict table read-only |
| the shape | the parsed body and its sites | plain after the load, since every decision is a fact |
| builtins | signatures, knot weights | computed at the load, stored plain |
| symbols | the text of every source name, keyed by digest, above the builtin image's static tier | read-only; a name `EVAL` parses goes in the agent's overlay |
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
closed verdict table; dispatch candidates per site, narrowed by the
load-known argument types; each judgement a binding decides, stored whole as a
question of the program's [binding graph](binding-graph.md); and the
instances of that graph its text reaches from the root, frozen with it and
shared by every agent. What the frozen load cannot hold is a node that
mentions a binding's carried type, a verdict over one, an instance a run cell
makes, and anything `EVAL` builds.

**Covariance.** An agent's graph borrows storage at its own `'graph`, with
`'load: 'graph`, and a node of any cell embeds load nodes through that
covariance, as a chain embeds its root. The root and its content stay
covariant in their lifetime.

## The agent tier

An insert-only tier of nodes and verdicts laid in a bump the agent owns. A
node is handed out as `&'agent StoredNode<'agent>` and never moves, nothing in
the tier is released before the tier is released whole, and the tier goes
with its agent. A run's verdict over load or agent types interns into it,
since no run may write into the load's table. A run cell reads the agent tier,
records verdicts there and interns no type into it: a type the run builds
lives in the run's halves. The agent's symbol overlay,
the names its `EVAL`s parse, sits beside it; a render reads the overlay, the
frozen table, then the builtin image's texts, and they never disagree, since a
symbol is its text's digest.

## Instances and compositions

**Instances.** What a binding decides is an instance of a base's
[binding graph](binding-graph.md#instances), made by evaluating the questions
the load stored, never by typing the text again. The instances the load makes
are frozen with it. Those a run cell makes rest in one bump the agent lends
its base's instances, under a budget, filed by view: the digests of the
content the node's names are bound to. A view whose every content the load
names is kept for the agent's life; any other rests in a small table of its
node's own, where a newer view may take its place. A frame holds a handle per
node, which names no storage, so nothing in a region references an instance.

**Compositions.** The **build depot** holds one kind of slot under one root:
a **composition**, the shape of code composed at run time, the facts that wait
on nothing but its syntax, its own binding graph and the instances frames
under it make, keyed on the code's syntax digest, as a module's shape is its
text while its captures bind at their statements. The drain finds it by
digest at each `EVAL` of the code and each call of a callable born in it, and
rebuilds it on a miss from the syntax the value carries.

| what decides | decided in | lives |
|---|---|---|
| the text alone | the load, its types in the root | the program's life |
| a binding the text reaches from the root | an instance the load makes, frozen with it | the program's life |
| a binding a run cell reaches | an instance in the agent's bump, filed by view | the agent's life, within its budget |
| composed syntax | a composition | until its eviction, shared by every `EVAL` of the code |

A depot slot holds its facts and its graph, and no type a frame reads. The
depot is the agent's own, outside the cell graph, with a borrower count that
drops at the borrower's release, and one budget over the bytes of the depot
slots no frame holds, evicted least recently used, since evicting a held one
frees nothing ([the scheduler](../src/scheduler/README.md#the-build-depot)).
A composition rests nothing in the agent tier, since their number is bounded
by the run: a type composed code declares is run content the frame mints at
the declaring statement, and a fact names a
type by its digest, resolved in the root or in the frame's chain, or minted by
the frame as a term. The cell graph holds run cells alone, and the drain
keeps the build's books.

## Agents

An agent is a thread with its own cell graph, drain, build depot, agent tier
and symbol overlay. Agents share the frozen load and nothing else: no build
data crosses between them after the load, so there is no mirror, no shared
books and no cross-agent pending map, and what an agent recomputes is what
the frozen load leaves undecided. A value crossing between agents adopts its
carried type into the receiver's chain, free when the type is a load type,
since its handle says so. The program's memory is the frozen load plus, per
agent, its budget and its stack
([agents](../roadmap/features/agents.md)).

## Open work

- [Agents](../roadmap/features/agents.md) — threads sharing the frozen load.
- [What the agent tier keeps](../roadmap/one-build/agent-tier-keeps.md) — the
  tier's content once nothing settles into it.
- [The build image](../roadmap/one-build/build-image.md) — the frozen load
  and program storage written out and resumed from.
