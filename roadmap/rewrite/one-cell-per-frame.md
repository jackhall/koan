# One cell per frame

**Optional.** Lands when a measurement of cells and parks per hop asks for it.

**Problem.** A called body runs as one cell per non-leaf expression. The body
runner asks for one evaluation per statement, a `Shares` tenant of the frame,
and the evaluator asks for every part that is no literal, name or quote as a
tenant of its own ([what a node is](../../src/dispatch/README.md#what-a-node-is)),
so `(F (G (H x)))` is three nested cells. The drain is a step machine with no
stack: a step waits on a child only by ending — storing its continuation,
registering a receipt run — and being re-entered when the run fills. So a
frame parks once per statement, once per node with a non-leaf part, and once
per spawned call, and each park is a create, an enter, a release and a receipt
slot ([the body runner](../../src/program/README.md#the-body-runner) refuses a
cell per unit for that reason, one level up). The frame's last statement is
handed off to a `Fresh` tree child only because its evaluation is a separate
cell: the frame must be released before the hop its evaluation tails into,
and stays dead but undisposed until that child disposes
([tail hops](../../src/scheduler/README.md#tail-hops)).

**Acceptance criteria.**

- A called body's frame is one cell: it parks only on a spawned frame or on a
  builtin whose native step parks, and the mutual and self-hopping loops in
  [`loops.rs`](../../src/dispatch/tests/loops.rs) run one cell per hop beside
  each frame the hop's body spawns.
- The hand-off is gone: a frame's step evaluates its last statement itself
  and tails, and no end of the scheduler's births a cell under a dying frame.
- A value asked for with `Threads` is still built in the loop's cell, and one
  asked for with `Forwards` in its home, so every loop in `loops.rs` holds
  the same bytes per hop as before.
- The top level keeps a `Fresh` child per statement, so what a top-level
  statement allocates beyond its value still dies with it.

**Directions.**

- *An evaluation stack in the frame's continuation — decided.* The evaluator
  becomes an explicit-stack machine over the body's shape, run inside the
  frame's step; a park stores the stack of partly evaluated nodes beside the
  runner's state. Fewer parks, each carrying more.
- *Where a `Threads` part is built — open.* Placement is a cell's: `Threads`
  works by bearing the evaluation as a tenant of the loop cell, so its
  `'here` is that region, and a step names no place but its own
  ([what a step may name](../../src/scheduler/README.md#what-a-step-may-name)).
  Either the frame's step takes a second writer for the loop cell, as it has
  one for its scratch — a brand beside `'here` and `'scratch`, sound because
  the loop cell is the frame's ancestor — or a part asked for with `Threads`
  stays the one kind of part that gets a cell of its own.
- *Spawn and hop as one end — decided.* With the hand-off gone, a call the
  frame's step makes either spawns a frame and parks or tails into one, both
  over operands held beside their kept parts, so a birth laid beside the
  kept part of each argument, as every call's is
  ([a frame keeps what it was handed](../../src/program/README.md#frames-contracts-and-tails)),
  is the only shape left.

## Dependencies

Sequenced after the [conditionals](../conditionals/README.md) and
[metaprogramming](../metaprogramming/README.md) projects, each of which adds
kinds of node the evaluator runs: one machine written after them is written
once.

**Requires:** none.

**Unblocks:** none — a leaf.
