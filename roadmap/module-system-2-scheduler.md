# Module system stage 2 — Module values and functors through the scheduler

**Problem.** Stage 1 shipped the module language as surface syntax —
`MODULE` and `SIG` declarators, `:|` / `:!` ascription, per-module abstract-type
identity via `KType::ModuleType { scope_id, name }` — but module expressions
are not yet first-class participants in the scheduler's free-execution
model. The narrow paths stage 1 wired don't generalize: type expressions
have no persistent home in `Scope` and no incremental-refinement story,
functors aren't dispatchable end-to-end, and the
[design doc](../design/module-system.md#inference-and-search-as-scheduler-work)
leaves open whether inference and implicit search ship as new node kinds
or reduce to existing `Execute` / `Dispatch`. Meanwhile the
[`dispatch::runtime::arena`](../src/dispatch/runtime/arena.rs) Miri slate
that signed off the previous memory model under `-Zmiri-tree-borrows` is
out of date: stage 1 reshaped the runtime — `Module` and `Signature` use
the same `*const Scope<'static>` lifetime-erasure pattern as `KFunction`,
new `RuntimeArena` slots feed into ATTR's chained-attribute path, opaque
ascription re-binds source module entries into a fresh child scope. Every
new unsafe site, every new shape of arena re-entry, every new lift path
needs to face the same Miri evidence the current set does.

**Impact.**

- *Module expressions dispatch and execute.* Module values flow through
  the scheduler the same way ordinary values do — dispatched, executed,
  bound, aggregated. Any feature that treats modules as first-class values
  (signature-bound dispatch, modular implicits, functor application
  results) has a working substrate.
- *Type expressions are first-class in `Scope`.* Type expressions persist
  in `Scope` with an incremental-refinement story, so partially-known
  types can be tightened as inference proceeds and dependents wake on the
  refinement.
- *Functors are defined, dispatched, and executed.* Functors are FNs whose
  parameters are signature-typed and whose body returns a `MODULE`
  expression
  ([design/module-system.md § Functors](../design/module-system.md#functors));
  their definition, dispatch, and execution work end-to-end. This is what
  lets a generic data structure — `(MAKESET Element)`, `(MAKEMAP Key
  Value)` — be written once and instantiated against any element type
  with the required operations, with no per-concrete-type duplication.
- *Tests cover the module system end to end.* Coverage extends to the
  dispatch and execution paths above and to the functor cases, so the
  module system is exercised through the scheduler rather than only at
  the surface forms shipped in stage 1.
- *Memory-model sign-off carries the new module surface.* The
  [audit slate](../design/memory-model.md#audit-and-sign-off) re-runs
  against the post-stage-1 runtime and any new unsafe sites this stage
  introduces, so the closure-escape + per-call-arena story stays
  evidence-backed rather than carried on prior assertion.

**Directions.** The central architectural question — whether inference
and implicit search ship as new node kinds or reduce to existing `Execute`
/ `Dispatch` — is open per
[design/module-system.md § Inference and search](../design/module-system.md#inference-and-search-as-scheduler-work);
the implementation plan answers that and sequences the agenda above
accordingly. Functor surface and sharing-constraint syntax are decided in
the design doc; the remaining functor implementation choices are below.

- *Functor declaration syntax — decided.* Functors are FNs whose
  parameters are signature-typed and whose body returns a `MODULE`
  expression. No `FUNCTOR` keyword.
- *Sharing constraints — decided.* Pinning a functor's output abstract
  type to its input rides on named-slot syntax for parameterized type
  expressions (`<Type: E.Type>`), not a separate `with type` keyword. See
  [design/module-system.md § Parameterized type expressions](../design/module-system.md#parameterized-type-expressions).
- *Generative vs applicative semantics.* Generative — each application
  produces a fresh abstract type — is simpler to specify and provides the
  per-type identity property the design relies on, and falls out of
  `:|`-per-call evaluation. Applicative — same arguments yield the same
  output type — is more ergonomic when functors are re-applied.
  Recommended: generative for v1, revisit later.
- *Type identity through functor application.* `(MAKESET IntOrd)` applied
  twice yields two distinct `Set` types. The implementation extends stage
  1's module-type identity carrier to include the application context.
- *Higher-kinded abstract type slots.* Signatures need to declare type
  constructors (a `Wrap` slot taking a type parameter) so monads and
  other parametric abstractions are expressible. Required by
  [monadic-side-effects](monadic-side-effects.md).
- *Audit slate carry-forward.* Re-run the existing 16-test audit slate
  plus the `alloc_object_redirects_self_anchored_value_to_escape_arena`
  regression test added in the cycle-gate fix. Append new tests for the
  stage-1 unsafe sites (the `*const Scope<'static>` transmutes in
  `Module::child_scope` / `Signature::decl_scope`, the opaque-ascription
  path that re-binds source module entries into a fresh child scope, the
  `type_members` `RefCell<HashMap>` mutation) and any new raw-pointer
  sites picked up by this stage.

## Dependencies

**Requires:**

**Unblocks:**
- [Standard library](standard-library.md) — collections and other
  parametric abstractions ship as Koan-source functor FNs once functor
  dispatch and execution work end-to-end.
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  implicit resolution rides on the dispatch and execution of module values
  this stage lands; resolves the new-node-kinds-vs-reduction question.
- [Error handling](error-handling.md) — `Result<T, E>` is the
  functor-produced carrier for user-typed errors.
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the in-language `Monad` signature's `Wrap` slot is higher-kinded,
  expressible only with functor support.
- [Static type checking and JIT compilation](static-typing-and-jit.md) —
  both the checker's lifetime story and the JIT's codegen contract want a
  stable, signed-off memory model plus a settled answer to the
  inference-as-scheduler-work question.
