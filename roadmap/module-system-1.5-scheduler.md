# Module system stage 1.5 — Scheduler integration

**Problem.** Stage 1 shipped the module language as surface syntax — `MODULE`
and `SIG` declarators, `:|` / `:!` ascription, per-module abstract-type
identity via `KType::ModuleType { scope_id, name }` — but the type system
behind it remains the runtime `KType::matches_value` check that predates
modules. There is no compile-time inference pass and no implicit-search
machinery; the [scheduler](../src/execute/scheduler.rs) only runs `Dispatch`,
`Bind`, and `Aggregate` nodes. Stages 2-5 (functors, axioms,
modular implicits) all assume the
[`Infer` and `ImplicitSearch` scheduler nodes described in the design
doc](../design/module-system.md#compile-time-scheduling), and stage 5 in
particular cannot land without the type-checking phase boundary and the
multi-target unification story. Meanwhile the
[`dispatch::runtime::arena`](../src/dispatch/runtime/arena.rs) Miri slate
that signed off the previous memory model under `-Zmiri-tree-borrows` is now
out of date: stage 1 reshaped the runtime — `Module` and `Signature` use the
same `*const Scope<'static>` lifetime-erasure pattern as `KFunction`, the
new `RuntimeArena` slots (`modules`, `signatures`) feed into ATTR's
chained-attribute path, and opaque ascription re-binds source module entries
into a fresh child scope. Every new unsafe site, every new shape of arena
re-entry, every new lift path needs to face the same Miri evidence the
current set does, and the scheduler work below adds more.

**Impact.**

- *Type checking interleaves with implicit search.* `Infer` and
  `ImplicitSearch` scheduler nodes ship alongside `Dispatch`, with the
  scheduler's existing dependency tracking and cycle detection carrying both.
  Inference produces type refinements that search consumes; search produces
  module choices that refine types other inference tasks are waiting on, and
  cycles in either show up as the same kind of bug.
- *Multi-target unification lands.* A single inference task refines many type
  variables that downstream tasks are waiting on, either via a shared
  out-of-band substitution or by modeling type variables as their own
  scheduler nodes that get refined and woken up. Stage 5's modular implicits
  are what forces the question; this stage answers it.
- *Module values carry a known signature at use sites.* `Infer` resolves the
  signature of every module-typed value where it's used, so signature-bound
  dispatch (the stage-5 substrate piece) and any future use-site type-check
  of a module value can rely on a static signature being available rather
  than falling back to runtime inspection of the bound module.
- *Type-check-vs-evaluation phase boundary stabilizes.* Type checking
  completes for a compilation unit before evaluation begins. Whether one
  batch boundary or finer-grained per-definition phase tracking, the
  scheduler grows the machinery here rather than discovering it under stage 5.
- *Memory-model sign-off carries the new module surface.* The
  [audit slate](../design/memory-model.md#audit-and-sign-off) re-runs against
  the post-stage-1 runtime *and* the new scheduler nodes from this stage, so
  the closure-escape + per-call-arena story stays evidence-backed rather
  than carried on prior assertion. The `*const Scope<'static>` transmutes in
  `Module::child_scope` / `Signature::decl_scope`, any new raw-pointer sites
  picked up around per-module type identity, and any unsafe sites the
  scheduler grows for `Infer` / `ImplicitSearch` each get a targeted Miri
  test alongside the slate.
- *Static-typing-and-JIT has a stable target.* The checker's lifetime story
  and the JIT's codegen contract both want a memory model that's signed off
  against the post-stage-1 runtime *and* the scheduler nodes the type checker
  rides on.

**Directions.** None decided.

- *Node body shape.* `Infer(expr, ctx)` and `ImplicitSearch(sig, types,
  scope)` body code is distinct from `Dispatch`'s, but the scheduling,
  dependency tracking, and cycle detection are shared. The Rust signatures
  follow whatever shape `BodyResult` settles into after the monadic
  side-effect pass.
- *Substitution carrier.* Either a shared substitution map threaded
  out-of-band as a scheduler-level resource, or model type variables as
  their own scheduler nodes that get refined and woken up. The first is
  cheaper to land; the second slots more naturally into the existing
  dependency-tracking story.
- *Phase-boundary granularity.* Whole-compilation-unit batch boundary or
  finer-grained per-definition tracking. The first is simpler; the second
  permits incremental compilation more naturally.
- *Failure isolation.* When an inference or search fails, dependents fail
  too — but independent subtrees should still finish so the user sees
  multiple errors per compile rather than one-at-a-time. Falls out of the
  existing scheduler error-propagation rules; needs a deliberate decision
  on what counts as "independent."
- *Audit slate carry-forward.* Re-run the existing 16-test audit slate plus
  the `alloc_object_redirects_self_anchored_value_to_escape_arena` regression
  test added in the cycle-gate fix. Append new tests for the stage-1 unsafe
  sites (the `*const Scope<'static>` transmutes, the
  opaque-ascription path that re-binds source module entries into a fresh
  child scope, the `type_members` `RefCell<HashMap>` mutation) and any new
  raw-pointer sites picked up by the scheduler nodes themselves.

## Dependencies

**Requires:**

**Unblocks:**
- [Stage 2 — Functors](module-system-2-functors.md) — sharing constraints
  and generative-functor type identity ride on the type-checker substrate.
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  implicit resolution is the scheduler-node story this stage builds the
  substrate for.
- [Static type checking and JIT compilation](static-typing-and-jit.md) —
  both the checker's lifetime story and the JIT's codegen contract want a
  stable, signed-off memory model plus a settled scheduler-node shape to
  target.
