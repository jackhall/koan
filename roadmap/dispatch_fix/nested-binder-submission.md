# Nested-binder recursive submission

Submit nested binders' sub-`Dispatch` nodes at parent-submission time, so
their placeholders install before any sibling can dispatch — closing the
`LET f = (FN NAME [x] x)` race independent of FIFO ordering. Rename the
misnamed `pre_run` extractors to `binder` while the same code is open.

**Problem.** A nested binder's placeholder installs when the sub-Dispatch
is submitted, which today happens during the parent's Phase 4 — after a
sibling pops under FIFO. Under FIFO this is benign (every sibling in the
block is enqueued before any pops). Under strict-only admission, where
`Unbound` errors immediately, a sibling that dispatches before the
sub-Dispatch submits would hard-error on a name that should park. The
race exists today; strict-only admission would surface it.

The `pre_run` / `pre_run_bucket` names imply a body-execution hook that
doesn't exist — both extractors are pure structural projections over the
unresolved expression (the name, or the inner-call bucket key, that the
dispatch driver installs in `placeholders` / `pending_overloads`).

**Impact.**

- *Nested binders submit at the outermost submission point.*
  [`Scheduler::add`](../../src/machine/execute/scheduler/submit.rs)
  walks the picked function's `ClassifiedSlots` and recursively `add`s
  each eager Expression-slot part as a sub-Dispatch in the same
  submission step. Each recursive `add` runs its own
  `extract_binder_name`, so nested binders' placeholders all install at
  the outermost submission point.
- *Lazy slots are skipped.* `KType::KExpression` bodies of FN / FUNCTOR /
  MODULE are *not* recursively submitted — their parts dispatch in the
  callee's scope, not the parent's.
- *Phase 4 reuses pre-submitted children.* The W-collapse-era Phase 4
  successor reuses the pre-submitted child NodeIds rather than
  re-submitting — splice them into the parent's slots like today's
  wrap-slot mechanism.
- *Bounded by AST depth.* Recursion terminates at non-binder leaves and
  at lazy slots.
- *`pre_run` / `pre_run_bucket` become `binder_name` / `binder_bucket`.*
  The type aliases
  ([`PreRunFn`](../../src/machine/core/kfunction/body.rs),
  [`PreRunBucketFn`](../../src/machine/core/kfunction/body.rs)), the
  matching `KFunction` fields, and the
  `register_builtin_with_pre_run` helper rename together.

**Directions.**

- *Recursive submission at `add` — decided.* `Scheduler::add` walks
  `ClassifiedSlots` of the picked function and recursively submits each
  eager Expression-slot part.
- *Phase 4 splice path — open.* Exact reuse mechanism for pre-submitted
  child NodeIds — modeled on today's wrap-slot splice but applied
  uniformly to all eager Expression slots.
- *Rename `pre_run` / `pre_run_bucket` → `binder_name` /
  `binder_bucket` — decided.* Type aliases, `KFunction` fields, and the
  registration helper rename together.

## Dependencies

**Requires:**

- [Index-gated resolution](index-gated-resolution.md) — the race this
  closes is only observable once strict-only admission lands, but the
  structural `Placeholder` / `Unbound` split is the precondition that
  makes the recursive-submit fix correct (rather than ordering-dependent).

**Unblocks:**

- [Unified walk + strict-only admission](unified-walk.md) — strict-only
  admission needs nested binders' placeholders installed before any
  sibling dispatches.
