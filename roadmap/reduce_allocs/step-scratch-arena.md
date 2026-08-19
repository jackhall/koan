# Step-scoped scratch arena

A bump arena reset at the top of every slot step, whose confinement is the step brand,
plus its first consumer: the dispatch bucket walk.

**Problem.** The execute path has no place to put a staging buffer. A buffer that is
built, read, and dropped inside one step currently goes to the global heap, because the
only arena in reach is a *frame region* — reclaimed when the frame dies, so step-transient
staging put there grows without bound across an `Inherit` cart's steps. `StepAllocator`
([src/machine/core/arena/step_allocator.rs](../../src/machine/core/arena/step_allocator.rs))
is region-destined construction, not scratch.

The cost shows up hardest on the bucket walk, which `resolve_dispatch`'s own comment
([src/machine/execute/decide/resolve_dispatch.rs](../../src/machine/execute/decide/resolve_dispatch.rs))
calls the hottest read in the machine. Per ancestor scope, per dispatch, it collects the
same overload set three times: `lookup_function_probe` copies the bucket's visible
finalized overloads into `FunctionLookup { overloads: Vec<SealedFunction> }`
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)); `decide_scope`
re-collects that vector into `candidates: Vec<OpenedFunction>`; `pick_strict` then builds
`survivors: Vec<usize>` and `sigs: Vec<&ExpressionSignature>` off the candidates. Four
heap allocations per scope walked, plus the same four again for the root builtin probe,
and `build_resolved` adds one to two more through `ClassifiedSlots`
([src/machine/core/kfunction/pick.rs](../../src/machine/core/kfunction/pick.rs)). None of
the four escapes the call — only the pick does, through `Scope::open_function`.

The recorded baselines put a figure on that traffic: the 128-operand chain in
[audit/README.md](../../audit/README.md) costs ≈63 allocations per dispatch, against 226
per machine step for the tail loop, and `tests/allocation_baseline.rs` holds both shapes
to a bound. A buffer removed here therefore shows up as a measured drop rather than as a
static read of the call graph.

Absent a scratch arena the only way to remove them is to rework the signature of
`ExpressionSignature::most_specific`
([src/machine/model/types/signature.rs](../../src/machine/model/types/signature.rs)) to
take indexed pairs, contorting a type-system door to solve an allocation problem.

**Acceptance criteria.**

- `Host` owns a scratch `Bump` reset once per slot step, before the step's rank-2 brand
  is opened, and the reset retains the first chunk so steady state performs no allocator
  syscall.
- A scratch handle reaches decide code through `DecideCtx`
  ([src/machine/execute/decide/ctx.rs](../../src/machine/execute/decide/ctx.rs)) at the
  step's own `'b`, so a scratch-allocated buffer that outlived the step is a borrow-check
  error rather than a convention.
- Nothing scratch-allocated escapes the step: no scratch buffer is reachable from an
  `Outcome`, a park's `Deps`, or a stored continuation.
- The bucket walk's `overloads`, `candidates`, `survivors` and `sigs` are scratch-hosted,
  and `ExpressionSignature::most_specific` keeps the contiguous-slice signature it has today.
- Every scratch buffer whose final length is known at construction is built with that
  capacity, so no scratch buffer reallocates and abandons bump bytes within a step.
- The recorded per-dispatch allocation count for a call resolved through an n-deep scope
  walk is independent of n.

**Directions.**

- *Confinement mechanism — decided.* The step brand, the same mechanism `StepCarried`
  ([src/machine/execute/step_carried.rs](../../src/machine/execute/step_carried.rs))
  already uses: `'b` is unnameable outside the step's `open`, so a `Vec<T, &'b Bump>`
  cannot outlive it. `BumpAllocator` already implements `allocator_api2::Allocator`
  ([workgraph/src/witnessed/bump.rs](../../workgraph/src/witnessed/bump.rs)) and
  `allocator_api2` is already in the graph, so no new dependency is needed.
- *Reset point — decided.* The top of `Host::step`
  ([src/machine/execute/harness.rs](../../src/machine/execute/harness.rs)), which has a
  single caller — the drain closure — and takes `&mut self`, so `&mut Bump` is reachable
  exactly where no `'b` is live. `apply` recurses into itself but never re-enters `step`,
  and mid-step submissions do not either; the reset point is backed by a debug assert
  that no step is already in flight.
- *Growth discipline — decided.* Bump deallocation is a no-op and `grow` extends in place
  only for the newest allocation, so a growing scratch `Vec` abandons its old buffer as
  dead bump bytes. Every scratch buffer here has a known bound — `survivors` and `sigs`
  by `candidates.len()`, `candidates` by `overloads.len()` — so all are built with capacity.
- *`FunctionLookup`'s lifetime — open.* Give the struct the scratch brand so its
  `overloads` is scratch-hosted, versus keeping `FunctionLookup` heap-backed and hosting
  only the two walk buffers. Recommended: the former — `lookup_function_stored` has two
  call sites, both inside `decide/`, so the parameter does not travel far.
- *Buffers that escape as park data — deferred.* A park's `Deps<R>`
  ([workgraph/src/scheduler/deps.rs](../../workgraph/src/scheduler/deps.rs)) is workgraph
  API over a global-allocator `Vec`, so anything leaving the decide as park data stays on
  the heap; see [Park-wiring buffer recycling](park-wiring-buffer-recycling.md).

## Dependencies

**Requires:** none — the allocation counts that decide whether this plumbing earns its keep are recorded.

**Unblocks:**

- [Step-local staging on the scratch arena](scratch-hosted-step-staging.md) — that item moves the remaining step-local buffers onto this arena.
