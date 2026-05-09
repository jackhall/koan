# Module system stage 2 — Module values and functors through the scheduler

**Problem.** Stage 1 shipped the module language as surface syntax —
`MODULE` and `SIG` declarators, `:|` / `:!` ascription, per-module abstract-type
identity via `KType::ModuleType { scope_id, name }` — and the type-expression
substrate now lands too: scheduled type-constructor builtins (`LIST_OF`,
`DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF` in
[`builtins/type_ops.rs`](../src/dispatch/builtins/type_ops.rs)) produce
`KObject::TypeExprValue` through the same `Dispatch` / `Bind` machinery values
use, and a [`ScopeResolver`](../src/dispatch/types/resolver.rs) lowers
`TypeExprValue` bindings in `Scope::data` so a `LET MyList = (LIST_OF Number)`
binding makes `MyList` available as a type name in subsequent FN signatures.
What this substrate does not yet support: parens-wrapped type expressions
in FN parameter positions (`xs: (LIST_OF Number)`) aren't sub-dispatched,
and FN-def's `ScopeResolver` lookup gives up rather than deferring when a
type identifier isn't yet bound — so a non-LET-wrapped FN that references
an earlier `LET MyType = (LIST_OF Number)` fails, and a signature-typed
parameter naming a SIG that's only in scope at the call site has no path
to resolve. Functors aren't dispatchable end-to-end at all: there is no
`KType::SignatureBound` slot kind, no `KType::TypeConstructor`, no
`TypeParams::Named` for sharing constraints, and no generative-application
semantics that mints fresh abstract types per call. Meanwhile the
[`dispatch::runtime::arena`](../src/dispatch/runtime/arena.rs) Miri slate that
signed off the previous memory model under `-Zmiri-tree-borrows` is out of
date: stage 1 reshaped the runtime — `Module` and `Signature` use the same
`*const Scope<'static>` lifetime-erasure pattern as `KFunction`, new
`RuntimeArena` slots feed into ATTR's chained-attribute path, opaque
ascription re-binds source module entries into a fresh child scope. Every new
unsafe site, every new shape of arena re-entry, every new lift path needs to
face the same Miri evidence the current set does.

**Impact.**

- *Module expressions dispatch and execute.* Module values flow through
  the scheduler the same way ordinary values do — dispatched, executed,
  bound, aggregated. Any feature that treats modules as first-class values
  (signature-bound dispatch, modular implicits, functor application
  results) has a working substrate.
- *Type expressions assemble end-to-end inside FN signatures.* Top-level
  type bindings (`LET MyType = (LIST_OF Number)`) and parameterized type
  expressions inside FN parameter lists compose freely: a `FN (USE xs:
  MyType)` waking the binding behaves the same as `FN (USE xs: (LIST_OF
  Number))`, and either form can be tightened as inference proceeds with
  dependents waking on the refinement. The type-builtin substrate is in
  place; closing this requires FN-def to dispatch its parameter-list type
  expressions as scheduler work rather than parsing them as `Type` tokens
  up front.
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

**Directions.**

- *Inference and search as scheduler work — decided per [design/module-system.md § Inference and search](../design/module-system.md#inference-and-search-as-scheduler-work).*
  Inference and implicit search reduce to the existing `Dispatch` and
  `Bind` machinery — no `Infer` node kind, no `ImplicitSearch` node kind,
  no `KType::TypeVar`, no `Scope::types`. Type-returning builtins are
  ordinary builtins, type expressions in source position re-elaborate to
  a synthesized call, and refinement rides on `Bind` waiting for its
  sub-Dispatches.
- *Type resolution in FN signatures — decided.* No new top-level
  sequencing primitive, no parallel type-resolution pass. The existing
  FIFO + `Bind` discipline that gives `LET x = ...; PRINT x` source-order
  semantics for values gives the same for types, *if* FN-def's signature
  elaboration rides on the same machinery. Two changes:
  - **Parens-wrapped type expressions sub-dispatch.** A parameter
    position written `xs: (LIST_OF MyType)` schedules the parens-wrapped
    part as a sub-Dispatch; its `KObject::TypeExprValue` result splices
    in via the standard `Bind` path. An `elaborate_type_expr` helper in
    `src/dispatch/types/resolver.rs` is the shared entry point.
  - **Bare type identifiers resolve hybrid.** FN-def attempts synchronous
    resolution at body time via the existing `ScopeResolver`. Resolved
    slots land in the resulting `KFunction` as ordinary `KType`s.
    Unresolved slots carry the original `TypeExpr` on the KFunction; the
    first call re-runs resolution against the FN's captured scope and
    memoizes the result (one `OnceCell<KType>` per slot, sound because
    the captured scope is lexically fixed). This covers the realistic
    LET-wrapped FN cases (resolution succeeds at body time via FIFO) and
    the late-binding cases — signature-typed parameters, mutually
    recursive type references — without a new scheduler primitive.
- *Functor declaration syntax — decided.* Functors are FNs whose
  parameters are signature-typed and whose body returns a `MODULE`
  expression. No `FUNCTOR` keyword.
- *Sharing constraints — decided per [design/module-system.md § Parameterized type expressions](../design/module-system.md#parameterized-type-expressions).*
  Pinning a functor's output abstract type to its input rides on
  named-slot syntax for parameterized type expressions (`<Type: E.Type>`),
  not a separate `with type` keyword.
- *Generative vs applicative semantics — open.* Generative — each
  application produces a fresh abstract type — is simpler to specify and
  provides the per-type identity property the design relies on, and falls
  out of `:|`-per-call evaluation. Applicative — same arguments yield the
  same output type — is more ergonomic when functors are re-applied.
  Recommended: generative for v1, revisit later.
- *Type identity through functor application — decided.* `(MAKESET IntOrd)`
  applied twice yields two distinct `Set` types. The implementation
  extends stage 1's module-type identity carrier to include the
  application context.
- *Higher-kinded abstract type slots — decided.* Signatures declare type
  constructors (a `Wrap` slot taking a type parameter) so monads and
  other parametric abstractions are expressible. Required by
  [monadic-side-effects](monadic-side-effects.md).
- *Audit slate carry-forward — decided.* Re-run the existing 16-test audit
  slate plus the `alloc_object_redirects_self_anchored_value_to_escape_arena`
  regression test added in the cycle-gate fix. Append new tests for the
  stage-1 unsafe sites (the `*const Scope<'static>` transmutes in
  `Module::child_scope` / `Signature::decl_scope`, the opaque-ascription
  path that re-binds source module entries into a fresh child scope, the
  `type_members` `RefCell<HashMap>` mutation), the type-builtin dispatch
  path (`type_op_dispatch_does_not_dangle`), and the per-call functor
  module lift (`functor_per_call_module_lifts_correctly`). The named
  slate from the implementation plan
  (`module_child_scope_transmute_does_not_dangle`,
  `signature_decl_scope_transmute_does_not_dangle`,
  `opaque_ascription_re_binds_do_not_alias_unsoundly`,
  `type_members_refcell_mutation_does_not_corrupt_under_concurrent_borrow`,
  plus the two named above) is the deliverable. Today the test-helper
  leaks in `dispatch::runtime::scope::tests` (the `Box::leak` markers in
  the specificity tests) are the only Miri findings — replace those with
  arena-anchored allocations as part of this work so the slate runs
  zero-leak.

## Dependencies

**Requires:**

**Unblocks:**
- [Standard library](standard-library.md) — collections and other
  parametric abstractions ship as Koan-source functor FNs once functor
  dispatch and execution work end-to-end.
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  implicit resolution rides on the dispatch and execution of module values
  this stage lands, layered as a `SEARCH_IMPLICIT` builtin per the
  reduction in [design/module-system.md § Inference and search](../design/module-system.md#inference-and-search-as-scheduler-work).
- [Error handling](error-handling.md) — `Result<T, E>` is the
  functor-produced carrier for user-typed errors.
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the in-language `Monad` signature's `Wrap` slot is higher-kinded,
  expressible only with functor support.
- [Static type checking and JIT compilation](static-typing-and-jit.md) —
  both the checker's lifetime story and the JIT's codegen contract want a
  stable, signed-off memory model plus a settled answer to the
  inference-as-scheduler-work question.
