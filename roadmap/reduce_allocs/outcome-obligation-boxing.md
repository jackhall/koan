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
onto that delivery. Each layer exists because every currency in the chain is a
`Box<dyn FnOnce>`, so a combinator can add behavior only by re-boxing what it was handed.

The end-state currency is pinned by
[design/execution/continuations.md](../../design/execution/continuations.md): one uniform
closure signature, generic combinator composition with a single erasure, a two-tier erase
door (bumped `Copy` closures / boxed owning ones), and the obligation as data beside the
closure. This item retypes the currency and converts the re-decide continuations, whose
captures are already `Copy`.

**Acceptance criteria.**

- The stored currency is the two-tier form: `ignore_results`, `with_obligation`,
  `short_circuit`, and `seal_witnessed` no longer exist as dyn-to-dyn re-boxers — their
  behavior composes generically before a single erasure, and the obligation rides as
  data the step deposits into the ambient slot-step state.
- The nine re-decide sites — the birth/tail decide, the keyworded binder wait and
  post-eager-subs redispatch, the fn-value head park, the bare-type-leaf and type-call
  re-resolves, the operator-chain declared-op wait, and the builtin invoke — erase on
  the bumped tier. (The pre-errored slot stays Boxed for its `KError`.)
- A re-profile of `audit/shapes/tail_loop_steps100.koan` attributes no per-step term to
  any continuation-layer function; the `step` term in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drops by the boxing share,
  and the tail-loop bound in `tests/allocation_baseline.rs` is re-measured onto the new
  reading.

## Dependencies

[`ReturnObligation`](../../src/machine/execute/obligation.rs) is `Copy`, so the
data-carriage shape rides it beside the continuation as plain slot data.

**Requires:** none.

**Unblocks:**

- [Body-enter continuation](body-enter-continuation.md)
- [Dep-finish captures](dep-finish-captures.md)
- [Builtin action continuations](builtin-action-continuations.md)
