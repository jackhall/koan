# Outcome and obligation boxing

**Problem.** Every step rebuilds its continuation chain on the heap, one one-shot box per
layer. The dhat profile of the audit shapes attributes ≈22 allocations per tail-loop step
to the combinators in [src/machine/execute/outcome.rs](../../src/machine/execute/outcome.rs)
and [src/machine/execute/obligation.rs](../../src/machine/execute/obligation.rs):
`ignore_results` (10/step) wraps a `ResumeFn` — itself already a
`Box<dyn FnOnce>` built at the call site — in a fresh `NodeContinuation` box, and
`with_obligation` (8/step) wraps that box in a third whose only act is depositing the
obligation into the ambient slot-step state before delegating. The dep-finish path stacks
the same way: `short_circuit` boxes a `TerminalDepFinish` into the `NodeContinuation`
that runs it, and `seal_witnessed` adds one more layer projecting a `WitnessedDepFinish`
onto that delivery. Each closure is built, installed on a slot, run once and dropped; the
heap round-trip exists only because every currency in the chain is a `Box<dyn FnOnce>`,
so a combinator can add behavior only by re-boxing what it was handed.

**Acceptance criteria.**

- A steady-state tail-loop step allocates no continuation boxes: a re-profile of
  `audit/shapes/tail_loop_steps100.koan` attributes no per-step term to `ignore_results`,
  `with_obligation`, `short_circuit`, or `seal_witnessed`.
- The `step` term in [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) — the
  per-tail-loop-step marginal cost, whose meaning [audit/README.md](../../audit/README.md)
  carries — drops by the boxing share, and the tail-loop bound in
  `tests/allocation_baseline.rs` is re-measured onto the new reading.

**Directions.**

- *Continuation placement — open.* Where a one-shot continuation's captures live once the
  per-step `Box` is gone. Alternatives: place the closure in a region the slot's anchor
  already pins — the continuation is sealed against `Rc<SlotFrame>` at install
  (`ContinuationFamily`), so the seal already asserts an owner that covers the captures —
  or keep the heap but collapse the layers so one box is built per continuation
  (a reduction of the term, not its removal). The `seal_witnessed` layer folds away under
  either shape by making the delivery generic over the finish's result.
- *Obligation carriage — open.* `with_obligation` exists to deposit the obligation at the
  top of the step; the wrapper closure is one way to carry it there. The alternative is
  data: the obligation rides beside the continuation (on `NodeWork` / the slot), and the
  drain deposits it before running the step, so no wrapping happens at all. Recommended:
  carry it as data — it retypes a channel the slot already owns rather than adding a
  closure layer whose only body is a store.

## Dependencies

[`ReturnObligation`](../../src/machine/execute/obligation.rs) is `Copy`, so the recommended
obligation-carriage shape can ride it beside the continuation as plain slot data.

**Requires:** none.

**Unblocks:** none.
