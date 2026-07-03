# The step construction context

The witnessed hoist proper: guarantees 3 and 5 of
[design/scheduler-library.md](../../design/scheduler-library.md), enforced at
the finish boundary.

**Problem.** A value-copy finish receives its deps as bare relocated values:
the `relocate_values` adapter
([outcome.rs](../../src/machine/execute/outcome.rs):359) copies each dep's
value into the consumer region before the finish runs, so a finish body can
embed a dep in its output without naming the dep's reach — the signature
permits it, and reach totality holds only because each body upholds the
discipline. The witnessed channel (`WitnessedDepFinish`) is hand-operated:
each construction site folds its deps through explicit `transfer_into`
chains. And builtin bodies and finishes (`AwaitContinue`,
[action.rs](../../src/machine/core/kfunction/action.rs):219) allocate at will
through the scope brand and seal via `Scope::seal_value(…, None)` at fifteen
born-pure terminal sites — the `None` operand asserting "reaches no foreign
region" rather than the construction making it structural.

**Acceptance criteria.**

- The library exposes the step construction context of the design doc's
  consumer API: `ctx.region()` (infallible — the step loop already holds the
  consumer's region owner), `ctx.alloc(|b| value)` (reach = own region only),
  and `ctx.alloc_with(&[deps], |b, views| value)` (reach = own region ∪ the
  named deps' reaches, folded by the call shape; dep payloads viewable only
  at the closure brand).
- A dep payload is not obtainable as a bare pre-relocated value: the
  `relocate_values` adapter is deleted, and a finish reads deps as
  brand-confined views (through `alloc_with`) or as sealed carriers
  (`dep.carrier()`) — never as pre-copied `Carried`s. Builtin finishes
  receive the context through `FinishCtx` (the `AwaitContinue` signature
  changes here).
- The born-pure `seal_value(…, None)` terminal sites construct through
  `ctx.alloc`, so region-purity is structural rather than asserted by the
  `None` operand.
- A compile-fail doctest pins escape prevention: a dep view leaving
  `alloc_with`'s construction closure does not compile.
- Existing tests and the Miri audit slate green.

**Directions.**

- *Context ownership — decided* per
  [design/scheduler-library.md](../../design/scheduler-library.md): the
  context is library-owned and handed to the finish by the step loop, whose
  held region owner is what makes `ctx.region()` infallible (guarantee 4's
  enforcement, reused).
- *Migration staging — open.* (a) One PR: land the context, flip the
  `TerminalDepFinish` delivery, migrate every finish, delete
  `relocate_values`; (b) split into two items — context plus witnessed
  channel first, the value-copy finish migration second — if the diff
  outgrows one PR. Splitting is a scoping decision to surface, not take
  silently.
- *The catch channel — open.* `CatchOk.value` / `catch_continuation` relocate
  the watched value the same bare way; whether `CatchContinue` migrates in
  this item or trails as its own follow-up.

## Dependencies

**Requires:**

- [The opaque reach set](opaque-reach-set.md) — `alloc_with`'s reach fold
  mints and unions through the library set, infallibly.
- [The resolve-or-await protocol combinator](protocol-combinators.md) — the
  `AwaitContinue` signature change lands on combinator internals instead of
  per-builtin hand-rolled finishes.

**Unblocks:** none tracked — the remaining
[design/scheduler-library.md](../../design/scheduler-library.md) objectives
(regions wholesale, actual extraction) are not yet scoped as items.
