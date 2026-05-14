# Module system stage 2 — Module values and functors through the scheduler

**Problem.** Two stage-2 unsafe sites still have no targeted Miri test
under tree borrows: the opaque-ascription path that re-binds source
module entries into a fresh child scope, and the type-op dispatch
through the per-call arena. The rest of the stage-2 module surface —
scheduled type-constructor builtins producing typed values, the
scope-aware `elaborate_type_expr` walking both type-value and
`KSignature` bindings, MODULE / SIG body statements planning onto the
outer scheduler with `BodyResult::DeferTo`, end-to-end functor dispatch
with per-call generative semantics, the `SIG_WITH` builtin with
concrete-typed sharing-constraint pins, and higher-kinded
type-constructor slots via `(TYPE_CONSTRUCTOR <param>)` and
`KType::ConstructorApply` — has shipped; see
[design/module-system.md](../design/module-system.md) for the shape.

**Impact.**

- *Memory-model sign-off carries the full stage-2 module surface.* The
  [audit slate](../design/memory-model.md#verification) covers every
  unsafe site this stage touches — including the opaque-ascription
  re-bind path and the type-op dispatch through the per-call arena — so
  the closure-escape + per-call-arena story stays evidence-backed.

**Directions.**

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
  — module-language substrate including the higher-kinded `Wrap` slot
  surface. The HKT pieces have shipped; only the audit slate
  carry-forward remains here.
- [Static type checking and JIT compilation](static-typing-and-jit.md) —
  both the checker's lifetime story and the JIT's codegen contract want a
  stable, signed-off memory model plus a settled answer to the
  inference-as-scheduler-work question.
