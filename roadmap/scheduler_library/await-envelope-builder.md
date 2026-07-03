# The `Await` envelope builder

One constructor for "park on deps, then run a finish", with the dep-error
short-circuit built in — Layer 2 of the consumer API in
[design/scheduler-library.md](../../design/scheduler-library.md).

**Problem.** `Outcome::ParkThenContinue` envelopes are hand-assembled at every
consumer site:

- The first-errored-dep short-circuit loop exists twice: `short_circuit`
  (`src/machine/execute/outcome.rs:224-242`) vs `short_circuit_witnessed`
  (:267-281) differ only in the Ok-arm payload; both return the identical
  `propagate_dep_error(e, dep_error_frame)` short-circuit.
- Every site threads `dep_error_frame` and `park_count` by hand.
- The three aggregate-literal builders (`dispatch/literal.rs`
  `schedule_list` / `schedule_dict` / `schedule_record`, :114-213) share an
  identical classify-prelude and
  `submit_dep_finish_witnessed_in_own_scope` tail, and the list and record
  finish closures are near-verbatim.
- `dispatch/field_list.rs` duplicates its entire ~30-line finish body across
  the `Outcome` (:86-118) and `Action` (:150-178) currencies, with matching
  10-parameter signatures.

**Acceptance criteria.**

- A builder (working name `Await`, per the design doc) is the sole production
  constructor of `Outcome::ParkThenContinue` carrying a `Finish` /
  `FinishWitnessed` continuation; direct construction of those envelope shapes
  remains only inside the builder and the apply side.
- The dep-error short-circuit loop is implemented once; `short_circuit` and
  `short_circuit_witnessed` are one core with two Ok-arm projections (or two
  thin wrappers over it).
- A finish body never observes an errored dep.
- `field_list.rs` has one finish implementation, lifted into its two
  currencies at two thin entry points.
- `literal.rs`'s three `schedule_*` functions share one generic scheduling
  path parameterized by (classify, assemble); the duplicated finish closures
  are gone.
- Behavior unchanged; existing tests green.

**Directions.**

- *Builder API — decided* per
  [design/scheduler-library.md](../../design/scheduler-library.md):
  `Await::on(deps).error_frame(f).finish(...)`; names are working names.
- *Both finish channels supported — decided.* The builder wraps the value-copy
  and witnessed finishes over the one short-circuit core. Retiring the bare
  value-copy channel is a separate later item — do not migrate call sites
  between channels here.
- *`Resume` / `Catch` continuations — decided:* out of scope. The builder
  covers the `Finish` / `FinishWitnessed` shapes; `Resume` and `Catch`
  construction stays where it is.
- *Builder home — open.* `outcome.rs` (it assembles an `Outcome`) vs a new
  dispatch-facing module. Recommended: `outcome.rs`.

## Dependencies

**Requires:**

- [One producer-disposition primitive and the `Deps` builder](disposition-and-deps-builder.md)
  — the builder consumes `Deps` as its input currency.

**Unblocks:**

- [`Action::Tail` covers every dispatch tail](action-tail-single-lowering.md)
- [One dep-finish delivery currency](witnessed-only-dep-finish.md)
