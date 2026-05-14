# Module system stage 2 — Module values and functors through the scheduler

**Problem.** Higher-kinded type slots aren't expressible: there is no
higher-kinded slot carrier in `KType`, so a signature can't declare a
`Wrap` slot taking a type parameter. Functor return-type pins resolve
only against the FN's outer scope, so a per-call FN parameter's
attribute (e.g. the parameter's abstract type) can't be threaded through
to a return-type pin. Two stage-2 unsafe sites still have no targeted
Miri test under tree borrows: the opaque-ascription path that re-binds
source module entries into a fresh child scope, and the type-op dispatch
through the per-call arena. The substrate the rest of stage 2 rides on —
scheduled type-constructor builtins producing typed values, the
scope-aware `elaborate_type_expr` walking both type-value and
`KSignature` bindings, MODULE / SIG body statements planning onto the
outer scheduler with `BodyResult::DeferTo`, end-to-end functor dispatch
with per-call generative semantics, and the `SIG_WITH` builtin with
concrete-typed sharing-constraint pins — has landed; see
[design/module-system.md](../design/module-system.md) for the shipped
shape.

**Impact.**

- *Higher-kinded type slots become expressible.* Signatures declare
  type constructors (a `Wrap` slot taking a type parameter); functor
  applications then thread that constructor through to their output via
  the shipped `SIG_WITH` pin path. Unblocks the in-language `Monad`
  signature's `Wrap` slot for
  [monadic-side-effects](monadic-side-effects.md).
- *Memory-model sign-off carries the full stage-2 module surface.* The
  [audit slate](../design/memory-model.md#verification) covers every
  unsafe site this stage touches — including the opaque-ascription
  re-bind path and the type-op dispatch through the per-call arena — so
  the closure-escape + per-call-arena story stays evidence-backed.

**Directions.**

- *Inference and search as scheduler work — decided per [design/module-system.md § Inference and search](../design/module-system.md#inference-and-search-as-scheduler-work).*
  Inference and implicit search reduce to the existing `Dispatch` and
  `Bind` machinery — no `Infer` node kind, no `ImplicitSearch` node kind,
  no `KType::TypeVar`, no `Scope::types`. Type-returning builtins are
  ordinary builtins, type expressions in source position re-elaborate to
  a synthesized call, and refinement rides on `Bind` waiting for its
  sub-Dispatches.
- *Sharing constraints — decided per [design/module-system.md § Type expressions and constraints](../design/module-system.md#type-expressions-and-constraints).*
  Pinning a functor's output abstract type to a concrete type rides on
  the `SIG_WITH` parens-form builtin reusing the shipped `name: value`
  triple shape, not a `<>` named-slot extension. Resolution of the
  slot's right-hand side rides on the shipped scheduler-driven type
  elaborator.
- *Per-call FN-parameter references in pin values — open.* The shipped
  pin path elaborates values at FN-construction time against the FN's
  outer scope, so `(SIG_WITH SetSig ((Elt: (MODULE_TYPE_OF Er Type))))`
  for a per-call parameter `Er` parks on a name not yet bound. Closing
  this requires either templated return types (storing the unresolved
  `TypeExpr` on the function and substituting parameter values at call
  time before lifting) or extending the `substitute_params` walk to
  cover the return-type slot. Recommended: the templated-return path,
  symmetric with how parameter-typed slots already flow.
- *Higher-kinded abstract type slots — decided.* Signatures declare
  type constructors (a `Wrap` slot taking a type parameter) so monads
  and other parametric abstractions are expressible. Required by
  [monadic-side-effects](monadic-side-effects.md). Implementation needs
  a higher-kinded slot carrier in `KType` plus the surface syntax to
  declare and apply it.
- *Audit slate carry-forward — decided.* Two unsafe sites remain to
  pin: the opaque-ascription re-bind path
  (`opaque_ascription_re_binds_do_not_alias_unsoundly`) and the
  type-op dispatch path (`type_op_dispatch_does_not_dangle`). Slate
  re-runs zero-UB / zero-leak after each. The current slate
  ([TEST.md § Miri audit slate](../TEST.md#miri-audit-slate)) already
  covers the rest of stage 2's unsafe sites.

## Dependencies

**Requires:** none.

**Unblocks:**
- [Standard library](standard-library.md) — collections and other
  parametric abstractions ship as Koan-source functor FNs once functor
  dispatch and execution work end-to-end.
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  implicit resolution rides on the dispatch and execution of module values
  this stage lands, layered as a `SEARCH_IMPLICIT` builtin per the
  reduction in [design/module-system.md § Inference and search](../design/module-system.md#inference-and-search-as-scheduler-work).
- [Error handling](error-handling.md) — `Result<Ty, Er>` is the
  functor-produced carrier for user-typed errors.
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the in-language `Monad` signature's `Wrap` slot is higher-kinded,
  expressible only with functor support.
- [Static type checking and JIT compilation](static-typing-and-jit.md) —
  both the checker's lifetime story and the JIT's codegen contract want a
  stable, signed-off memory model plus a settled answer to the
  inference-as-scheduler-work question.
