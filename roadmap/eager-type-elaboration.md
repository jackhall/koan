# Eager type elaboration with placeholder-based recursion

Phases 1–2 (one canonical runtime type representation) and a meaningful slice
of phase 3 (scheduler-aware FN / STRUCT / UNION elaboration with self-recursive
STRUCT support and `LET T = T` cycle detection) have landed. The remaining
work — parens-wrapped FN parameter types and the phase-5 cleanup that deletes
`NoopResolver` and `KType::Unresolved` — is captured below. Mutual STRUCT /
UNION recursion (SCC pre-registration) was originally part of this item; it
now ships with [per-declaration type identity for structs and tagged
unions](per-declaration-type-identity.md), since both touch the same
STRUCT/UNION declaration surface.

**Problem.** Two concrete gaps remain after the phase-1–3 landing:

- *Parens-wrapped FN parameter types.* `parse_fn_param_list` in
  [`fn_def/signature.rs`](../src/runtime/builtins/fn_def/signature.rs) still
  only accepts `ExpressionPart::Type(t)` triples; a parameter written
  `xs: (LIST_OF Number)` raises `ShapeError` because the parens-wrapped
  form isn't sub-dispatched. The `OnceCell<KType>`-backed late binding
  for signature-typed parameters whose type only resolves at functor
  application time is also unstarted.
- *`NoopResolver` / `KType::Unresolved` cleanup.* Phase-3 wiring lifted
  bind-time elaboration off `ScopeResolver` (bindings now store
  `KObject::KTypeValue(KType)` directly, and the elaborator returns the
  stored value rather than re-elaborating), but
  [`NoopResolver`](../src/runtime/model/types/resolver.rs) and
  [`KType::from_type_expr`](../src/runtime/model/types/ktype_resolution.rs)
  still exist as a transitional seam:
  [`ExpressionPart::resolve_for`](../src/ast.rs) calls them to lower bare
  `Type(_)` parts in `TypeExprRef` slots, and an unresolved leaf falls
  through as a phase-2 transitional
  [`KType::Unresolved(name)`](../src/runtime/model/types/ktype.rs) that the
  scheduler-driven elaborator later replaces. Removing that fallback path
  (so bare-leaf user-bound type names route entirely through the
  scheduler-aware elaborator at FN-def / LET / STRUCT body time) lets
  `NoopResolver`, the `TypeResolver` trait's recursion arm, and
  `KType::Unresolved` all go.

**Impact.**

- *Type expressions assemble end-to-end inside FN signatures.* A FN
  parameter typed `xs: (LIST_OF MyType)` schedules the parens-wrapped
  part as a sub-Dispatch and splices the resulting `KType` in via the
  Combine path FN-def already uses for bare-name parking. A
  signature-typed parameter whose type comes from a SIG in scope only at
  functor application time carries the original `TypeExpr` on the
  resulting `KFunction`; the first call re-runs resolution against the
  FN's captured scope and memoizes the result.
- *One elaboration entry point, no transitional carriers.* The phase-5
  cleanup deletes `NoopResolver`, `KType::Unresolved`, and the
  `TypeResolver`-shaped recursion arm of `KType::from_type_expr`. The
  scheduler-driven elaborator is the only path bare-leaf type names take;
  `resolve_for` becomes a thin builtin-table lookup for `Type(_)` parts
  that are genuinely builtin scalar names. `KType::from_type_expr`
  remains for the parens-wrapped sub-dispatch lowering only.

**Directions.**

- *Parens-wrapped type expressions sub-dispatch — decided.* A parameter
  position written `xs: (LIST_OF MyType)` schedules the parens-wrapped
  part as a sub-Dispatch; its `KObject::KTypeValue` result splices in via
  the standard `Bind` path. The
  [`elaborate_type_expr`](../src/runtime/model/types/resolver.rs) helper
  added in phase 3 is the shared entry point.
- *Signature-typed parameter late binding via `OnceCell<KType>` — decided.*
  Names not yet even dispatched at FN-definition time (signature-typed
  parameters whose type comes from a SIG only in scope at functor
  application time) carry the original `TypeExpr` on the resulting
  `KFunction`; the first call re-runs resolution against the FN's captured
  scope and memoizes the result (one `OnceCell<KType>` per slot, sound
  because the captured scope is lexically fixed). The OnceCell fallback
  narrows to genuine functor late-binding cases; top-level and
  lexical-scope cases are handled at bind time and the OnceCell never
  fires there.
- *`NoopResolver` and `KType::Unresolved` removal — decided.* With bindings
  storing elaborated `KType` directly and the scheduler-aware elaborator
  handling bare-leaf user-bound names at FN-def / LET / STRUCT body time,
  `ScopeResolver::resolve` no longer re-elaborates anything and
  `ExpressionPart::resolve_for`'s `from_type_expr(&NoopResolver)` fallback
  has no live user-bound case to cover. `NoopResolver`, the `TypeResolver`
  trait's recursion arm, and `KType::Unresolved` go in one pass.
- *Module-qualified type names — open.* `TypeExpr` carries a name string
  that can naturally hold a path like `MyMod.Number`; `KType` has no
  path-aware variant today. If module-qualified type references ever need
  to flow as type values, either `KType::ModuleType` covers the case
  (already path-shaped) or a new `KType::Qualified(Path)` variant is
  needed. Decision deferred until a use case forces it; the current
  module-system stages don't.
- *Forward references and partial definitions — open.* Eager elaboration
  means a type alias's RHS must resolve at bind time. Mutual STRUCT/UNION
  recursion ships with [per-declaration type identity for structs and
  tagged unions](per-declaration-type-identity.md), but a binding whose
  RHS references a name not yet introduced (e.g. a top-level `LET T = U`
  where `U` is declared later in source) still fails. Whether to extend
  `Scope::placeholders` to typed names beyond an SCC group, or to require
  source-order declaration for non-mutually-recursive aliases, is left
  open until a real use case appears.

## Dependencies

**Unblocks:**
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md) —
  higher-kinded slot elaboration (`KType::TypeConstructor`), sharing
  constraints (`<Type: E.Type>`), and the remaining stage-2 audit slate
  ride on the scheduler-driven elaborator this work lands.
