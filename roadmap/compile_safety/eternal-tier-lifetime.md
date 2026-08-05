# Eternal storage as its own lifetime

**Problem.** Koan collapses the program-storage lifetime into the step lifetime
at every carrier that holds both. `SchedulerView<'step, 'view>`
([ctx.rs](../../src/machine/execute/dispatch/ctx.rs)) stores the run's
[`ProgramBrand`](../../src/machine/core/arena/frame.rs) shortened to `'step`;
`BodyCtx<'a, 'c>` ([action.rs](../../src/machine/core/kfunction/action.rs))
carries `ctx: StepAllocator<'a>` and `program: ProgramBrand<'a>` at one `'a`; the
`OP` plan threads the same shortened brand to its bridge body. Once collapsed, a
step-region `&'step str` and a program-region borrow are the same type, so the
mint doors in [program.rs](../../src/machine/model/ast/program.rs)
(`new_expression`, `build_expression`, `nested_node`) accept `parts` allocated at
the step brand, and their doc comment states the tier obligation as prose rather
than carrying it in the parameter types. Nothing mints such a node today —
`op_def`'s bridge body allocates its texts and operand nodes through
`program.region()` — and the direct case is caught by the borrow checker, since a
`ProgramExpression<'step>` cannot be stored in any holder the step outlives. The
path the lifetime does not close is the **step boundary**: a value leaves a step
through the workgraph open's `retype`, where the argument is the cell's reach
description rather than its lifetime, and an expression cell's verdict is `Owned`
with no member — so a cell over step-hosted parts would claim it pins nothing and
be re-anchored past its own producer. Library-side, the tier is a runtime `bool`
on [`RegionHost`](../../workgraph/src/witnessed/host.rs) read by
`pins_beyond_eternal`; no type says a borrow is eternal.

**Acceptance criteria.**

- The program-storage lifetime is a parameter distinct from the step lifetime on
  every carrier that holds both — `SchedulerView`, `BodyCtx`, the `OP` plan —
  related only by a `'program: 'step` bound.
- Handing a mint door a part allocated at the step brand is a compile error.
- [program.rs](../../src/machine/model/ast/program.rs)'s door doc states no
  allocation obligation in prose: the parameter types carry it.
- Eternal-tier data still reaches step code unchanged — a builtin body reads
  program-hosted AST at the step lifetime with no copy, re-host or widening exit.
- The Miri audit slate is green.

**Directions.**

- *Where the bound comes from — decided.* `Within<'b, 'outer: 'b>`
  ([dormant.rs](../../workgraph/src/witnessed/dormant.rs)), the `thread::scope`
  shape whose declared `'outer: 'b` the rank-2 step open's HRTB instantiation must
  discharge. It already gates every `SealedPinned::open`, so carrying a
  `'program`-branded value into a `'b`-scoped struct is legal today; what is
  missing is only that the carriers keep the two lifetimes apart instead of
  unifying them. The product still shortens where it should: a
  `ProgramExpression<'program>` coerces into a `KObject<'step>` cell by ordinary
  covariance, which is the borrow the shortening exists to allow.
- *Whether the marker newtypes survive — decided.* Yes, both. `KObject<'a>` has
  one lifetime parameter, so a node's `'program` collapses to `'step` the instant
  it enters a cell; only the newtype carries the mint past that coercion. The two
  proofs answer different questions — the type says *where this was minted*, the
  lifetime says *what its parts may borrow* — and neither subsumes the other.
- *Whether workgraph's eternal tier becomes a type — open.* Options: (a) leave
  `RegionHost::is_eternal` a runtime flag, since it answers a different question
  (which regions a pin bundle must cover) and the koan-side lifetime split closes
  the value-channel case on its own; (b) give the library a tier parameter so
  `pins_beyond_eternal` reads a type. Recommended: (a) — (b) is viral through
  every region-holding structure for a filter that is already exact.

## Dependencies

**Requires:** none — the `Within` bound and the `SchedulerView` / `BodyCtx` /
`OpPlan` program-brand plumbing this builds on are shipped.

**Unblocks:** none.
