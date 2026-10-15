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
  type at the statement that binds it, and a path variable's hop.
- Every parked fact's frontier is computed at the load's quiescence, the
  seams are listed per segment, and a frontier resumes at the seam where its
  last member binds, at entry or mid-body alike.
- Facts a resumption settles land in the build cell its seam and binding
  name, and a call a resumption judges *always* runs without admitting or
  ranking.
- A use left with no candidate at a resumption faults at the resumption
  point, naming the call.
- A verdict a resumption reaches holds at every run of a frame under that
  binding, by the laws the load's verdicts hold by.
- The properties hold as property tests in the dispatch suite, generated over
  programs and bindings with the closed one-shot load as the oracle:
  monotone, confluent, resume equals restart, quiescence is exact, and
  termination.
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
- *The termination proof* — open. A shared specialization's key ranges over
  root types and invariant content, a finite set per program; what remains
  is that one resumption's facts range over a finite set under join, meet
  and the solve. Recommended: bound a resumption's domain by the types its
  binding and the root can name, and show the solve mints nothing outside
  it.
- *Deepening the borrow at a mid-body seam* — open. The build tree fixes a
  run cell's lend at its creation and the drain resolves it before the frame
  is born, so a seam mid-body needs cellgraph to re-lend a live cell, refused
  while the cell's scratch state is parked, its scratch held or a tenant's
  scratch state parked, each of which may name the old lender at `'lent`
  ([a counted borrow of a named cell](../../cellgraph/roadmap/named-lend.md));
  the alternative is adopting what the scratch holds over the old lend before
  the frame takes the deeper one. A `Shares` frame reads through its host's
  lend, so facts per specialization for one need a per-cell lend or a `Fresh`
  placement for a specialized body. No specialization's branch links to its
  parent's, since no brand lets a build child hold a reference into its
  parent's region; facts that reference a parent binding's types need a link
  or an adopt.

## Dependencies

The worklist is a monotone data-flow framework (Kildall, 1973; Kam and
Ullman, 1977), and the residual is a solver's undischarged constraint
(Pottier and Rémy, 2005).

**Requires:**

- [The build tree](../type-tiers/build-tree.md) — the build cells the
  solver's products land in, and the steps it runs as.

**Unblocks:**

- [Activation typing](activation-typing.md) — a frame's facts settled at its
  seams.
