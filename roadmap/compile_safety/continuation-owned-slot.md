# Droppable families off the Copy tier

**Problem.** Under the
[dormant union slot](../../workgraph/design/witnessed-memory.md), the Copy
tier — `Erased` / `Sealed` / `SealedExtern`, and the `Witnessed` / `Delivered`
carriers built over `Erased` — bounds `T: DropFree`, and the default
`reattachable!` arm's const backstop rejects any family whose `At<'static>`
carries drop glue. Koan's droppable families do not compile, in three shapes:

- **Resting dormant:** `ContinuationFamily`
  ([outcome.rs](../../src/machine/execute/outcome.rs)) — `NodeContinuation<'r>`
  is a `Box<dyn FnOnce …>` resting erased on lifetime-free node slots as
  `SealedExtern<ContinuationFamily>` / `Erased<ContinuationFamily>`.
- **Transient zip operands:** `DepResultsFamily`
  ([run_loop.rs](../../src/machine/execute/run_loop.rs)), whose
  `Vec<Result<DepTerminal, KError>>` rides a transient `SealedExtern` zipped
  into the step open, and `ExpressionSignatureFamily`
  ([signature.rs](../../src/machine/model/types/signature.rs)), whose
  `Vec<SignatureElement>` rides a zip operand at the `KFunction` birth door.
- **Fold accumulators:** `AggBuildFamily`
  ([literal.rs](../../src/machine/execute/dispatch/literal.rs)) and
  `RecordFieldsFamily`
  ([constructors.rs](../../src/machine/execute/dispatch/constructors.rs)),
  `Vec`-carrying accumulators riding `Witnessed` / `Delivered` folds.

The droppy arena-`Stored` families
([arena.rs](../../src/machine/core/arena.rs) — `KFunction<'static>`,
`Scope<'static>`, `Module<'static>`, …) never rest in a carrier — region
arenas keep their own drop discipline — but are declared through the default
arm, so the backstop rejects them too.

Two library-side surfaces ride the same debt. The scheduler's node slot stores
`Erased<W::Continuation>`
([nodes.rs](../../workgraph/src/scheduler/nodes.rs)) under a `DropFree` bound on
`Workload::Continuation`
([workload.rs](../../workgraph/src/scheduler/workload.rs)), so a continuation
resting on `SealedPinned` means retyping that slot and dropping that bound.
Separately, `StepContext::alloc_with` hands its build closure a region-bumped
`&'b [V::At<'b>]` rather than an owned `Vec`
([step_ctx.rs](../../workgraph/src/witnessed/step_ctx.rs)), which koan's
`alloc_carried_with` wrapper
([step_allocator.rs](../../src/machine/core/arena/step_allocator.rs)) follows.

**Acceptance criteria.**

- koan compiles with the continuation slot on `SealedPinned`: the erase is
  co-located with the anchor pin at the scheduler's install seam, and the
  once-per-step open — the owned-tier verb, with the scope operand riding the
  `SealedExtern` side — consumes the value before the pins drop. Dep results
  cross the step as lifetime-free delivery envelopes read per-envelope
  (`Delivered::open_at`), riding no carrier.
- Every other carrier family meets `DropFree`; `ContinuationFamily` is the
  only carrier family declared through the `droppable` arm.
- Arena-`Stored`-only droppy families are declared through the `droppable`
  arm; no `DropFree` exemption, suppression, or bare-`Erased` workaround
  remains; `SealedExtern` serves only `DropFree` families.
- Behavior unchanged: the verify slate and both Miri slates are green.

**Directions.**

- *Continuation migration — decided.* The family is declared through the
  `droppable` arm, the resting slot retyped to `SealedPinned`, and the pin
  co-located at the scheduler's install seam from the node's anchor `Rc` — the
  same liveness witness the open is bounded by today, now bundled instead of
  external. `NodeWork` carries the continuation live until install, so no
  koan call site can mispair a pin.
- *Vec-carrying families — decided, shapes chosen at planning (see
  `scratch/continuation-owned-slot-plan.md`).*
  DropFree-ification over owned-tier adoption: `DepTerminal` goes
  lifetime-free (values read per-envelope; `DepResultsFamily` deleted); the
  signature's lifetime-free `elements` move-capture into the born closure with
  only the `Copy` `ReturnType` crossing as an operand
  (`ExpressionSignatureFamily` deleted); the fold accumulators become
  re-bumped `Copy` slices on the `fold_dep_view` precedent, record field names
  leaving the carrier.

## Dependencies

**Requires:** none.

**Unblocks:** none.
