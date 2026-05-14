# Module system stage 2 — Module values and functors through the scheduler

**Problem.** Higher-kinded type slots aren't expressible: there is no
`KType::TypeConstructor` and no `TypeParams::Named` for sharing
constraints, so a signature can't declare a `Wrap` slot taking a type
parameter and a functor's output abstract type can't be pinned to its
input via `<Type: Er.Type>` named-slot syntax. Two stage-2 unsafe sites
still have no targeted Miri test under tree borrows: the
opaque-ascription path that re-binds source module entries into a fresh
child scope, and the type-op dispatch through the per-call arena. The
substrate the rest of stage 2 rides on — scheduled type-constructor
builtins producing typed values, the scope-aware `elaborate_type_expr`
walking both type-value and `KSignature` bindings, MODULE / SIG body
statements planning onto the outer scheduler with `BodyResult::DeferTo`,
and end-to-end functor dispatch with per-call generative semantics — has
landed; see [design/module-system.md](../design/module-system.md) for the
shipped shape. Scheduler-driven type elaboration with placeholder-based
recursion is tracked in [eager-type-elaboration](eager-type-elaboration.md).

**Impact.**

- *Higher-kinded type slots and sharing constraints become expressible.*
  Signatures declare type constructors (a `Wrap` slot taking a type
  parameter); functor applications pin their output abstract type to an
  input via `<Type: Er.Type>` named-slot syntax. Unblocks the in-language
  `Monad` signature's `Wrap` slot for
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
- *Sharing constraints — decided per [design/module-system.md § Parameterized type expressions](../design/module-system.md#parameterized-type-expressions).*
  Pinning a functor's output abstract type to its input rides on
  named-slot syntax for parameterized type expressions (`<Type: Er.Type>`),
  not a separate `with type` keyword. Implementation needs
  `TypeParams::Named` in the parser and a `KType::TypeConstructor`
  slot kind. Resolution of the named slot's right-hand side rides on the
  scheduler-driven elaborator landed by
  [eager-type-elaboration](eager-type-elaboration.md).
- *Higher-kinded abstract type slots — decided.* Signatures declare
  type constructors (a `Wrap` slot taking a type parameter) so monads
  and other parametric abstractions are expressible. Required by
  [monadic-side-effects](monadic-side-effects.md). Implementation
  needs `KType::TypeConstructor` and the surface syntax to declare and
  apply it.
- *Audit slate carry-forward — decided.* Two unsafe sites remain to
  pin: the opaque-ascription re-bind path
  (`opaque_ascription_re_binds_do_not_alias_unsoundly`) and the
  type-op dispatch path (`type_op_dispatch_does_not_dangle`). Slate
  re-runs zero-UB / zero-leak after each. The current slate
  ([TEST.md § Miri audit slate](../TEST.md#miri-audit-slate)) already
  covers the rest of stage 2's unsafe sites.

## Dependencies

**Requires:**
- [Eager type elaboration with placeholder-based recursion](eager-type-elaboration.md) —
  HKT slot elaboration (`KType::TypeConstructor`) and `<Type: Er.Type>`
  sharing-constraint resolution ride on the scheduler-driven elaborator
  that work lands. Doing stage 2's surface ergonomics first would force
  re-doing them once eager elaboration replaces the synchronous
  resolver.

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
