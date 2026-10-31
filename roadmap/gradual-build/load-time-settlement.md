# Settling traceable bindings at load

**Optional.** Lands when two measurements ask for it: how many of a program's
settlements its load could have decided, and what walking each traced body
once per binding it sees costs the load.

**Problem.** Facts are decided in four places — the load, a settlement, a
composition and a frame's activation
([where facts are decided](../../src/build/README.md#where-facts-are-decided))
— and a settlement is the costliest: an arena node with a key, a parent link,
a borrower count, a generation, a pin on a composition, eviction to a budget
and a stamp a callable carries to replay its chain after one
([the build arena](../../src/scheduler/README.md#the-build-arena)). It exists
because a body's facts wait on a binding only the run supplies. Yet most such
bindings are traceable: a functor applied at the top level to a `LET`-bound
module, a `FOR ALL` group solved at a keyworded call the load selected, a
callee reached through a knot's edges or a closed content tree. The tail
facts already run a greatest fixpoint over the call graph of the bodies whose
names do not escape
([load-time types](../../src/scope/README.md#load-time-types)), and keys
already leave out a parameter every tail edge passes on bare
([keys](../../src/build/README.md#keys-which-bindings-make-a-settlement)).

**Acceptance criteria.**

- A keyed call whose every call site passes load content the load can trace
  seeks nothing: the load lays the body's facts under each traced binding in
  program storage, indexed per call site as a route is, and the frame reads
  them through its door as it reads a settlement's today.
- The arena holds compositions alone: no settlement key, parent link, pin,
  held count or stamp replay, and no `Seek` but a composition's.
- A body under a binding the load cannot trace decides in its activation, and
  a loop spawning such a body each hop decides it once, not per hop.
- The property laws hold unchanged: a load-time settlement equals a restart
  with the binding declared, as a settlement does
  ([properties](../../src/build/README.md#properties)).
- The invariant mint sites of a caching body under a traced binding rest their
  types in the frozen load; under an untraced one they mint into the frame's
  scratch, the cold path a binding of run content takes today
  ([where each body interns](../../design/type-tiers.md#where-each-body-interns)).

**Directions.**

- *The trace — decided.* The tails fold extends from "passed on bare" to
  "passed this content": per keyed parameter of a non-escaping body, the set
  of load contents its call sites pass, transitively through the callers' own
  parameters; a body reachable through a value the load cannot follow is
  untraced. How much a program written with modules as parameters leaves
  traceable is unmeasured, and the test programs are the only corpus.
- *Where an untraced decision is kept — open.* A frame's decision memo rests
  in the region its result is built in and a spawned frame's is extended
  copy-on-write from its asker's, so a decision a spawned frame makes dies
  with its home ([activations](../../src/build/README.md#activations)); a
  loop spawning an untraced keyed body each hop would decide it each hop,
  where its settlement decides it once today. Either decisions are written
  back into the asker's memo, which gives up the memo's immutability, or
  bindings of load content get one memo at the run root keyed by digest —
  the arena's table without its tree. The third acceptance criterion is what
  decides between them.

## Dependencies

Sequenced after the [conditionals](../conditionals/README.md) project.

**Requires:** none.

**Unblocks:** none — a leaf.
