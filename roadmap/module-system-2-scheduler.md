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
and FN-def's [`ScopeResolver`](../src/dispatch/types/resolver.rs) does a
synchronous `scope.lookup(name)` and returns `None` rather than parking on
a dispatch-time placeholder, so a type identifier bound by an earlier
top-level `LET MyType = (LIST_OF Number)` doesn't resolve in a sibling FN
signature, and a signature-typed parameter naming a SIG only in scope at
the call site has no path to resolve. (Value-name forward references
already park on placeholders via the
[`Scope::placeholders`](../src/dispatch/runtime/scope.rs) sidecar and the
scheduler's `notify_list` / `pending_deps` machinery — the type-name path
is the gap.) Functors aren't dispatchable end-to-end at all: there is no
`KType::SignatureBound` slot kind, no `KType::TypeConstructor`, no
`TypeParams::Named` for sharing constraints, and no generative-application
semantics that mints fresh abstract types per call. Meanwhile the
[`dispatch::runtime::arena`](../src/dispatch/runtime/arena.rs) Miri slate
covers the post-reverse-DAG scheduler shape (push/notify edges with
`notify_list` / `pending_deps` sidecars and the `Lift { from: NodeId }`
work variant) and the stage-1 `*const Scope<'static>` lifetime-erasure
transmutes on `Module` / `Signature` plus the `type_members` `RefCell`,
but the opaque-ascription path that re-binds source module entries into
a fresh child scope still has no Miri test under tree borrows. Every new
unsafe site this stage introduces (functor lift, signature-bound dispatch,
type-op dispatch through the per-call arena) needs the same Miri evidence
the current set does.

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
  [audit slate](../design/memory-model.md#verification) re-runs against
  the new unsafe sites this stage introduces (functor lift, signature-bound
  dispatch, type-op dispatch through the per-call arena, the opaque-
  ascription re-bind path), so the closure-escape + per-call-arena story
  stays evidence-backed rather than carried on prior assertion.

**Directions.**

- *Inference and search as scheduler work — decided per [design/module-system.md § Inference and search](../design/module-system.md#inference-and-search-as-scheduler-work).*
  Inference and implicit search reduce to the existing `Dispatch` and
  `Bind` machinery — no `Infer` node kind, no `ImplicitSearch` node kind,
  no `KType::TypeVar`, no `Scope::types`. Type-returning builtins are
  ordinary builtins, type expressions in source position re-elaborate to
  a synthesized call, and refinement rides on `Bind` waiting for its
  sub-Dispatches.
- *Type resolution in FN signatures — decided.* No new top-level
  sequencing primitive, no parallel type-resolution pass. The
  push/notify scheduler with dispatch-time placeholders gives the same
  source-order semantics for types that it already gives for values, *if*
  FN-def's signature elaboration rides on the same machinery. Two
  changes:
  - **Parens-wrapped type expressions sub-dispatch.** A parameter
    position written `xs: (LIST_OF MyType)` schedules the parens-wrapped
    part as a sub-Dispatch; its `KObject::TypeExprValue` result splices
    in via the standard `Bind` path. An `elaborate_type_expr` helper in
    [`src/dispatch/types/resolver.rs`](../src/dispatch/types/resolver.rs)
    is the shared entry point.
  - **Bare type identifiers park on placeholders, then memoize.**
    FN-def's signature elaboration consults `ScopeResolver` (extended
    to check `Scope::placeholders` after `data`); a name whose binder
    has dispatched but not finalized parks the elaborating slot via
    the existing `notify_list` / `pending_deps` machinery, the same
    way a value-name forward reference parks today. Names not yet
    even dispatched (signature-typed parameters whose type comes from
    a SIG only in scope at the call site, mutually recursive type
    references) carry the original `TypeExpr` on the resulting
    `KFunction`; the first call re-runs resolution against the FN's
    captured scope and memoizes the result (one `OnceCell<KType>` per
    slot, sound because the captured scope is lexically fixed).
- *Functor declaration syntax — decided.* Functors are FNs whose
  parameters are signature-typed and whose body returns a `MODULE`
  expression. No `FUNCTOR` keyword.
- *Sharing constraints — decided per [design/module-system.md § Parameterized type expressions](../design/module-system.md#parameterized-type-expressions).*
  Pinning a functor's output abstract type to its input rides on
  named-slot syntax for parameterized type expressions (`<Type: E.Type>`),
  not a separate `with type` keyword.
- *Generative vs applicative semantics — decided.* Each functor
  application produces a fresh abstract type. `(MAKESET IntOrd)` called
  twice yields two distinct `Set` types whose values don't interoperate.
  Falls out of `:|`-per-call opaque ascription and stage 1's per-scope
  `KType::ModuleType { scope_id, .. }` identity carrier — no
  arguments-seen-before bookkeeping. Applicative semantics (same
  arguments → same output type) can be reconsidered later if the
  ergonomic cost of threading a single shared instance shows up in real
  use, but it's not in scope for v1.
- *Type identity through functor application — decided.* `(MAKESET IntOrd)`
  applied twice yields two distinct `Set` types. The implementation
  extends stage 1's module-type identity carrier to include the
  application context.
- *Higher-kinded abstract type slots — decided.* Signatures declare type
  constructors (a `Wrap` slot taking a type parameter) so monads and
  other parametric abstractions are expressible. Required by
  [monadic-side-effects](monadic-side-effects.md).
- *Audit slate carry-forward — decided.* The current 23-test slate
  ([TEST.md § Miri audit slate](../TEST.md#miri-audit-slate)) already
  passes against the post-reverse-DAG scheduler and the stage-1
  `*const Scope<'static>` transmute + `type_members` `RefCell` sites
  (covered by tests in
  [`dispatch::values::module`](../src/dispatch/values/module.rs)).
  Append three new tests for the sites this stage adds: the stage-1
  opaque-ascription re-bind path
  (`opaque_ascription_re_binds_do_not_alias_unsoundly`); the type-builtin
  dispatch path (`type_op_dispatch_does_not_dangle`); and the per-call
  functor module lift (`functor_per_call_module_lifts_correctly`). Slate
  re-runs zero-UB / zero-leak after each.

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
