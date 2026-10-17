# Monotone elaboration

The load's type pass as a solver that parks on an unknown and resumes when a
binding removes it.

**Problem.** The load types a program in one walk in evaluation order
(`Language::check` in `src/dispatch/statics.rs`, over the type channel's
write-once cells, [the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)).
A type it cannot fix is left at its bounds, `Any` and `Never` at the ends and
a *maybe* verdict, for the run to elaborate where each read happens
(`elaborate::bound_path`, [load-time types](../../src/scope/README.md#load-time-types)).
Nothing re-enters the pass between the load and a read, so a body over a
module parameter's carrier draws every verdict that meets the carrier against
a closed type at each use, though the content at the frame's entry would
settle them all once; a `FOR ALL` group solved at a call and a path class
bound at a hop are re-derived per read the same way. The pass records no
dependency, so it cannot say which facts a binding would settle.

**Acceptance criteria.**

- The driver is a worklist over type facts: a rule that reads an unknown
  parks its fact on that unknown and records the edge; the load runs to
  quiescence; its output is the fixed facts and the residual, the parked
  facts with their edges.
- A binding unparks exactly the facts on its edges, their rules re-run and
  only narrow, and a fact parks only on an unknown its rule reads: nothing
  closed is left to the run.
- The unknowns a binding removes are a parameter's content, a `FOR ALL`
  group's solution and a `:Type` argument at frame entry, a lexical variable's
  type at the statement that binds it, and a path variable's hop. An unknown,
  in an edge and in a key, is named by its lexical address — the segment and
  the coordinate the shape binds it at — never by its symbol or its lattice
  node.
- A mention of a fellow member of its own component, itself included, reads
  the fellow's annotation, the type its birth declares or `[Never, Any]`, and
  records no edge.
- Every parked fact's frontier is computed at the load's quiescence and the
  seams are listed per segment. A frontier whose members bind at a frame's
  entry resumes in the specialization that entry's binding keys; one with a
  member a statement binds resumes at that statement in the run cell's
  scratch, and no run cell's lend changes after its creation.
- A specialization keys on its segment and the content at the names any
  frontier at its entry waits on, facts in nested bodies included, beside the
  parameters its invariant sites read; a name a hop of the body's own tail
  rebinds is not keyed. A keyed frame seeks whatever its placement.
- A specialization's one step runs its resumption to quiescence and
  quiesces. Facts the resumption settles land in that cell, what it leaves
  parked is the cell's residual, and a call it judges *always* runs without
  admitting or ranking.
- A use left with no candidate at a resumption faults the asker at the
  resumption point, naming the call.
- A verdict a resumption reaches holds at every run of a frame under that
  binding, by the laws the load's verdicts hold by.
- The properties hold as property tests in the dispatch suite, generated over
  programs and bindings with the closed one-shot load as the oracle:
  monotone, confluent, resume equals restart, quiescence is exact, and
  termination — each re-run of a fact strictly narrows it, and the re-runs
  of one resumption total at most its facts times the domain's height.
- The per-expression typing rules in `elaborate` keep their meaning: every
  rule maps both ends of an interval, and the lattice is unchanged.

**Directions.**

- *The driver* — decided. A monotone worklist in the manner of Kildall, per
  [the build module's design](../../src/build/README.md#type-facts-and-the-solver).
- *The unit of parking* — decided. The type fact; the build cell is coarser,
  one seam under one binding.
- *Resumption scope* — decided. Per frontier, read off where its members
  bind; neither entry-only nor every-binding is set program-wide.
- *Refusal at a resumption* — decided. A use with no candidate faults at the
  resumption point, not at the use.
- *The termination proof* — decided. One resumption's facts range over the
  types of bounded depth over the root's types and the binding's content, a
  finite set: join, meet and the solve never deepen a type, substitution
  deepens by at most the binding's depth at each variable position, and the
  facts form no cycle, since a parameter's and a return's static type come
  from the declaration (`callable_type`), a local's from an earlier unit,
  and a mention inside a cyclic component is a leaf with no edge. The last is
  a rule the driver enforces, not a property it inherits: `Pass::component`
  in `src/dispatch/statics.rs` breaks a cycle by typing its members in order
  and reading an untyped fellow at `[Never, Any]`, and a worklist that gave
  the fellow an edge would narrow `LET a = LIST [b]; LET b = LIST [a]`
  forever, each step monotone. How many specializations live is bounded by
  [eviction](../../src/build/README.md#references-between-the-trees), not by the text.
- *Deepening the borrow at a mid-body seam* — decided. Never. A statement's
  binding is either determined by entry content — a `MATCH` arm's `it` over
  a parameter, a `USING … SCOPE` key over one, a path hop rooted at one —
  and settled at the entry seam, since a load with that content declared
  would type it closed; or produced by the run — an `EVAL`'s or an action's
  result — which is varying content and keys nothing, so the run cell
  resumes it into its own scratch. Cellgraph's lend stays fixed at creation.
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
  the residual through the parent region it borrows, or a run cell's
  scratch. `Step::enlist` grows no successor. A case where the same cell is
  the right place to resume — a remaining unknown with one possible binding
  — would reopen this; none is known. A capture is
  answered by keying on the seam's names: `inner`'s fact on a capture `xs`
  waits at `outer`'s entry, so `outer` keys on `xs`, and a fact whose
  frontier the chain's keys do not cover resumes cold. A body whose name
  escapes keys on its names like any other, since a hop into another callee
  seeks with the actual content and only a hop of the same callee reuses a
  specialization with no lookup; its mint sites stay varying.

## Dependencies

The worklist is a monotone data-flow framework (Kildall, 1973; Kam and
Ullman, 1977), and the residual is a solver's undischarged constraint
(Pottier and Rémy, 2005). The [build tree](../../src/build/README.md)'s
cells stand.

**Requires:** none — the build tree's cells and a tenant's own lend stand.

**Unblocks:**

- [Activation typing](activation-typing.md) — a frame's facts settled at its
  seams.
