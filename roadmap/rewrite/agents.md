# Agents

Concurrent runs of one program, each on a thread of its own, sharing the
frozen load.

**Problem.** A program is one process: one cell graph, one drain
([the scheduler](../../src/scheduler/README.md)), one build tree, one frozen
load of types with one agent tier above it, and one symbol table, all laid in
storage its substrate owns ([the program](../../src/program/README.md)).
Nothing in the language or the runtime names a second concurrent run of a
program. The frozen load's types are `Sync`
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)),
but the rest of what the load lays is not shared: the symbol table's frozen
map sits beside the agent's overlay in one interner, the build tree's books
are the drain's, and every region is one thread's.

**Acceptance criteria.**

- An agent is a thread with its own cell graph, drain, build arena, agent
  tier and symbol overlay; the program's agents run under a
  `std::thread::scope` the frozen load outlives, and each holds
  `&'load Frozen<'load>`.
- Nothing but values crosses between agents after the load: no build data,
  no shared books, no mirror. What an agent recomputes is what the load left
  undecided, built in its own arena.
- A value crossing between agents adopts its carried type into the receiver's
  chain, and a load type crosses by a bit test.
- A name `EVAL` parses in one agent renders from that agent's overlay, and
  equal text is one symbol in every agent, since a symbol is its text's
  digest.
- Statistics are kept per thread, and the program's memory is the frozen
  load plus, per agent, its budget and its stack.

**Directions.**

- *What spawns an agent — open.* A language construct that births one over a
  value, or the runtime, one per entry point. Nothing is ruled.
- *How a value crosses — open.* A result at a join, a message at the spawn,
  or a channel value. Nothing is ruled.
- *Per-agent rebuild — open.* An agent always rebuilds its specializations,
  or seeds one from another agent's record through the
  [build image](build-image.md). Recommended: always rebuilt, so no books
  are shared.
- *What is shared — decided.* The frozen load alone, by construction
  ([build storage](../../design/build-storage.md#the-frozen-load)).

## Dependencies

**Requires:**

- [The build arena](../gradual-build/build-arena.md) — the per-agent build
  arena.

**Unblocks:** none — a leaf.
