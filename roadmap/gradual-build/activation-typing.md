# Activation typing

A frame's facts settled at its seams, over the content its bindings bring.

**Problem.** A path through a parameter is a path variable of a class
([paths](../../src/dispatch/README.md#paths)): two paths of one class are one
type, and a call solving a variable from both is *always*. A candidate that
meets the path against a closed type, an overload at `Number` or a slot
`:Number`, or against a second class, is *maybe*, admitted and ranked at each
use, though at the frame's entry the argument's content closes the class and
every such verdict could be drawn once for the whole body. The load's pass is
not re-entered between the load and a read
([the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)),
and the run binds a path variable per read (`elaborate::bound_path`).

**Acceptance criteria.**

- A frame whose key completes a frontier at entry has every fact in that
  frontier settled in the specialization its seam and binding name before
  its first value step, and a call judged *always* there runs without
  admitting or ranking.
- A specialization's one step runs its resumption to quiescence and
  quiesces, over a chain through its own branch. Facts the resumption settles
  land in that cell's header, and what it leaves parked is the cell's
  residual.
- A second frame under the same binding, whatever its placement, reads the
  facts its specialization settled and re-derives none.
- No resumption runs in a run cell: a fact whose frontier the key does not
  cover — a statement's binding among its members, a parameter that does not
  key — stays unsettled and is admitted, ranked and substituted per read, and
  no run cell's lend changes after its creation.
- No run cell lays into a build cell: a specialization's branch holds the
  nodes the drain laid from its binding and what its resumption minted, and a
  frame lays only into its region and scratch.
- Every refusal a resumption reaches — a use left with no candidate, an
  ambiguity, or a return, ascription, annotation or call by name that can
  never be satisfied — faults the asker at the resumption point, worded as
  the load words it; a use left with no candidate names the call.
- Every type a specialization interns, and every verdict over one, lives in
  the specialization's branch and goes with its eviction and its last
  borrower's death.
- A verdict a specialization reaches holds at every run of a frame under
  that binding, by the laws the load's verdicts hold by.

**Directions.**

- *What triggers re-entry* — decided. The seams the load lists: a frame
  resumes at entry when a rigid's binding completes a frontier, and at a
  statement when a binding there completes one. Nothing counts *maybe*
  verdicts to decide.
- *What a resumption refuses* — decided. Every refusal it reaches faults at
  the resumption point, as the load refuses it, since a resumption is a load
  with its binding declared. A frame may so fault over a path it never
  takes, as the load refuses code no run reaches.
- *Deepening the borrow at a mid-body seam* — decided. Never. A statement's
  binding is either determined by entry content — a `MATCH` arm's `it` over
  a parameter, a `USING … SCOPE` key over one, a path hop rooted at one —
  and settled at the entry seam, since a load with that content declared
  would type it closed; or produced by the run — an `EVAL`'s or an action's
  result — which is varying content and keys nothing, so its facts stay
  unsettled. Cellgraph's lend stays fixed at creation.
  A `Shares` frame reaches its specialization through
  [a lend of its own](../../design/type-tiers.md#the-specialization-as-the-warm-tier). A child specialization
  whose facts name a type its parent minted relays it into its own branch,
  insert-if-absent, so no branch links its parent's and a run cell's chain
  probes one specialization.
- *A specialization that resumes* — decided, on the working assumption that
  no binding wakes a quiesced build cell: a specialization is one seam under
  one binding, what its resumption leaves parked waits on a binding its key
  does not fix, and two frames under it may fix that binding differently,
  so the parked remainder is the cell's residual and the work over it runs
  in the cell the next binding names — a child specialization, which reads
  the residual through the parent region it borrows. `Step::enlist` grows no
  successor. A case where the same cell is
  the right place to resume — a remaining unknown with one possible binding
  — would reopen this; none is known. A capture is
  answered by keying on the seam's names: `inner`'s fact on a capture `xs`
  waits at `outer`'s entry, so `outer` keys on `xs` where `xs` is a
  parameter that keys, and a fact whose frontier the chain's keys do not
  cover stays unsettled. A body whose name
  escapes keys on its names like any other, and its every hop seeks with the
  actual content, since it can tail-call itself through a value the load
  cannot follow; its mint sites stay varying.
- *Where a resumption runs* — decided. Only in a build cell's step. A fact no
  key covers stays unsettled and is read per read: the same programs, values
  and refusals on every taken path, with such a refusal surfacing at the use
  rather than the seam. A statement binding of a module digests, so a
  statement seam is cold only because a run cell's lend is fixed at creation;
  a body remainder born as a cell with its own lend would reopen it, and
  nothing here forecloses that.
- *A frame of a body that keys nothing, under an encloser's specialization* —
  open. A callable whose facts all wait on its encloser's names keys nothing of
  its own, so the encloser's specialization settles them, but its frame seeks
  nothing and is born borrowing its parent's scratch, which cannot read them.
  It may seek on its segment alone, a child of the specialization its closure
  was born under, or borrow that specialization itself.
- *Which side of a keyed module argument a binding reads* — open. The asker
  digests a keyed module argument and lays its types as it holds it, while the
  frame binds it retyped to its parameter's declared type, and past a barrier
  crossed: a loop handing its own `m` to an arm binds the retyped module the
  loop's frame holds. A resumption settles its facts against the binding's
  types, so it must reach the content the frame binds.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
