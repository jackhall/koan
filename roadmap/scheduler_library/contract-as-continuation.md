# Return contracts ride continuations

**Problem.** `Workload::Contract`
([workload.rs](../../workgraph/src/scheduler/workload.rs)) exists only to
verify the output of `Workload::Continuation`, yet it rides the trait as a
separate stored type: sealed on `NodeFrame.contract`
([nodes.rs](../../workgraph/src/scheduler/nodes.rs)) under its own
`RegionSet` witness (its backing is the caller's home region, independent of
the cart), zipped into the run-step open as its own lane, kept-first across
tail `replace`s by the run loop's `prev_contract.or(new_contract)`
([run_loop.rs](../../src/machine/execute/run_loop.rs); keep-first: a tail
chain reuses its slot, and the first call's contract must survive every
replacement because it governs the chain's final value), and handed to the
declared-return finalize hook
([finalize.rs](../../src/machine/execute/finalize.rs)) at the Done boundary
— all storage and plumbing for a checker the scheduler never inspects. A
sealed contract cell is lifetime-free and self-pinning, so a continuation
can capture it directly; the only path with no continuation left to wrap is
splice-forward, where the consumer slot is aliased and the check runs at
rehome ([workcell.md § What is deliberately absent](../../design/workcell.md)
frames the target: an output obligation is a continuation capture, not a
trait type).

**Acceptance criteria.**

- `Workload` carries no `Contract` type; `NodeFrame` stores no contract; the
  run-step open zips no contract lane.
- A continuation with a declared-return obligation captures its checker (the
  self-pinning sealed contract cell plus the trace label); the check runs
  before the terminal leaves the step.
- Tail replacements thread the inherited checker: koan applies the
  keep-first rule where it builds the replacement continuation, and
  `Scheduler::replace` performs no contract mediation.
- A tail-spliced consumer's residual return obligation is discharged before
  any consumer reads the rehomed terminal, via the mechanism chosen in
  Directions.
- An errored step's diagnostic still carries the contract-derived trace
  label.

**Directions.**

- *Splice check point — open.* (a) A checker micro-step registered on the
  consumer slot at splice time — rehome stays dumb, but each spliced finish
  with a declared return costs one scheduled step; (b) an inline check at
  `rehome_terminal` — splice stays free, but a workload hook returns to the
  rehome boundary.
- *Finalize-hook fate — open.* With checks riding continuations, whether the
  workload's Done-boundary finalize hook disappears entirely or survives
  with relocation-only duties (interacts with
  [scheduler-owned-frame-storage.md](scheduler-owned-frame-storage.md)'s
  envelope-in finalize).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Carving the workcell crate](workcell-extraction.md) — the cell contract
  has no contract type to carve around.
