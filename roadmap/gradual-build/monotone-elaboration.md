# Monotone elaboration

The load's type pass as a solver that parks on an unknown and resumes when a
binding removes it.

**Problem.** The load types a program in one walk in evaluation order
(the `statics` pass in `src/dispatch/statics.rs`, run as `Language::check`
over the type channel's write-once cells,
[the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)).
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
- Every lexical variable is an unknown, a `USING … SCOPE` block's surfaced
  names and a quote's holes, `\` marks and `$` names among them, and so is a
  module binder whose declared signature leaves a head parameter unpinned. An
  unknown, in an edge and in a key, is named by its lexical address — the
  segment and the coordinate the shape binds it at — never by its symbol or
  its lattice node.
- A binding removes a parameter's content, a `FOR ALL` group's solution and
  a `:Type` argument at frame entry, a lexical variable's type at the
  statement that binds it, and a path variable's hop at the statement that
  binds its root. A surfaced name binds at entry where its operand is a
  parameter, and at its `USING` statement otherwise. A quote's code lists its
  own entry as the seam of its holes, marks and `$` names.
- A mention of a fellow member of its own component, itself included, made
  in the component's own statements or in a block or `MODULE` or `GROUP`
  body they run inline, reads the fellow's annotation, the type its birth
  declares — at most `Module` for a `MODULE` or `GROUP` body — or
  `[Never, Any]`, and records no edge. A callable body nested in a member
  reads the fellow's fact.
- Every parked fact's frontier is computed at the load's quiescence, and the
  seams are listed per segment.
- A resumption takes the load's residual, or a resumption's, and a binding,
  and returns the facts it settles beside the residual it leaves, or the
  refusal it reaches. It runs inside the load, its types interned into the
  root.
- A verdict a resumption reaches holds at every run of a frame under that
  binding, by the laws the load's verdicts hold by.
- The properties hold as property tests in the dispatch suite, generated over
  programs and bindings with the closed one-shot load as the oracle:
  monotone, confluent, resume equals restart, quiescence is exact, and
  termination — a re-run never widens its fact, and each fact re-runs at
  most once per resumption, so the re-runs of one resumption total at most
  its facts.
- The per-expression typing rules in `elaborate` keep their meaning: every
  rule maps both ends of an interval, and the lattice is unchanged.

**Directions.**

- *The driver* — decided. A monotone worklist in the manner of Kildall, per
  [the build module's design](../../src/build/README.md#type-facts-and-the-solver),
  scheduled by the two load walks' evaluation order: the facts form no
  cycle, so that order is a topological one and each fact is popped once,
  after its inputs. Every read of a load-time fact goes through a fact store,
  which adds the fact's frontier to the reader's, and a resumption re-walks
  the seam's bodies, re-running only the facts its binding wakes. A literal
  fact graph run from a queue is
  [unplanned work](README.md#unplanned-work).
- *The unit of parking* — decided. The type fact; the build cell is coarser,
  one seam under one binding.
- *Resumption scope* — decided. Per frontier, read off where its members
  bind; neither entry-only nor every-binding is set program-wide.
- *A module binder's binding* — decided. The application of its declared
  signature its argument is retyped to, every head parameter pinned: what a
  restart declares as a pin. A path through it reads the pin by the load's
  own rule; a path the pins do not close stays a class.
- *Where a resumption runs* — decided. Inside the load, so the passes stay
  at `'graph`; a specialization's step running it over its own branch is
  [activation typing](activation-typing.md)'s.
- *The termination proof* — decided. One resumption's facts range over the
  types of bounded depth over the root's types and the binding's content, a
  finite set: join, meet and the solve never deepen a type, substitution
  deepens by at most the binding's depth at each variable position, and the
  facts form no cycle, since a parameter's and a return's static type come
  from the declaration (`callable_type`), a local's from an earlier unit,
  and a mention inside a cyclic component is a leaf with no edge. The last is
  a rule the driver enforces, not a property it inherits: `Pass::component`
  in `src/dispatch/statics.rs` breaks a cycle by typing its members in slot
  order and reading an untyped fellow at `[Never, Any]`, and a worklist that
  gave the fellow an edge would narrow `LET a = LIST [b]; LET b = LIST [a]`
  forever, each step monotone. How many specializations live is bounded by
  [eviction](../../src/build/README.md#references-between-the-trees), not by the text.

## Dependencies

The worklist is a monotone data-flow framework (Kildall, 1973; Kam and
Ullman, 1977), and the residual is a solver's undischarged constraint
(Pottier and Rémy, 2005).

**Requires:** none — the load's two passes stand.

**Unblocks:**

- [Activation typing](activation-typing.md) — a frame's facts settled at its
  seams.
