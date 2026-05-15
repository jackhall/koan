# Functor parameters — Type-class names and templated return types

**Problem.** FN return-type expressions referencing a per-call
parameter fail at FN-construction. Declaring
`FN (LIFT Er: OrderedSig) -> (MODULE_TYPE_OF Er Type) = ...` errors with
"unbound name `Er`". The
[`ReturnTypeCapture::TypeExpr`](../src/runtime/builtins/fn_def.rs#L239)
arm re-elaborates the captured expression against the FN's outer scope
at Combine-finish;
[`substitute_params`](../src/runtime/machine/kfunction/invoke.rs)
exists for FN bodies but is not wired into return-type elaboration.
Several shipped functor tests
([fn_def/tests/module_stage2.rs](../src/runtime/builtins/fn_def/tests/module_stage2.rs)
return-type-position cases, [ascribe.rs:441](../src/runtime/builtins/ascribe.rs#L441),
and neighbours) work around this with a lowercase identifier parameter
(`elem`, `p`) in the return-type slot rather than the documented
Type-class form.

(Parameter-position references — a Type-class FN parameter `Er:
OrderedSig` carrying a type-language binding for body-position lookups
like `(MODULE_TYPE_OF Er Type)` — already resolve through per-call
dual-write into `bindings.types`; see
[design/module-system.md § Functors](../design/module-system.md#functors).
This roadmap item is the return-type-position residual.)

**Impact.**

- *OCaml-style functor signatures become writable.* The canonical
  `module Make (E : ORDERED) : SET with type elt = E.t` shape — a
  functor whose result type pins to the input module's abstract type —
  is expressible end-to-end at the koan surface.
- *Standard-library collection functors generalize naturally.* `Make`
  over `ORDERED` and similar shapes carry sharing constraints between
  input and output abstract types at the FN signature.
- *Design-doc / runtime drift closes for the return-type slot.* The
  remaining lowercase-identifier workaround in return-type-position
  tests migrates to the documented Type-class form.
- *Substrate for dependent parameter annotations.* The `Deferred(_)`
  return-type carrier and per-call re-elaboration plumbing established
  here is the same machinery
  [Dependent parameter annotations](module-system-dependent-param-annotations.md)
  reuses for earlier-parameter references in later parameter types.

**Directions.**

- *Surface form — decided per
  [design/module-system.md § Functors](../design/module-system.md#functors).*
  `(MODULE_TYPE_OF Er Type)` for the return-type expression that
  references the parameter; `(SIG_WITH Set ((Elt: (MODULE_TYPE_OF Er Type))))`
  for sharing-constraint pins that reference it.

- *Templated return-type substitution — decided.* No separate
  substitution walk. Widen `ExpressionSignature::return_type` from
  `KType` to a two-variant
  `ReturnType { Resolved(KType), Deferred(TypeExpr) }`. Selection is
  decidable at FN-definition by scanning the captured `TypeExpr` for
  any leaf matching a parameter name: present → `Deferred`; absent →
  `Resolved`. Per call, `Deferred` re-runs
  [`elaborate_type_expr`](../src/runtime/model/types/resolver.rs)
  against the per-call scope where Stage A's dual-write has installed
  parameter names; parameter-name leaves resolve naturally through
  `bindings.types`. Return-type checking on the body's value runs
  against the per-call resolution. Replaces the existing
  [`ReturnTypeCapture::{Resolved, Unresolved, TypeExpr}`](../src/runtime/builtins/fn_def.rs#L239)
  split — Combine-finish at FN-definition either lands
  `Resolved(KType)` (no parameter refs and outer-scope elaboration
  succeeds) or `Deferred(TypeExpr)`. Parameter-type parking machinery
  ([`defer_via_combine`](../src/runtime/builtins/fn_def.rs)) is
  unchanged — parameter types remain definition-time-elaborated for
  dispatch keying.

- *Parens-form return-type carrier — decided.* Raw parens-form return
  types (`(MODULE_TYPE_OF Er Type)`, `(SIG_WITH Set ((Elt: Er)))`)
  land via a second FN overload whose return-type slot is
  `KType::KExpression` rather than `KType::TypeExprRef`, so the
  expression survives FN-def without sub-dispatching against the
  outer scope. The body branch on `bundle.get("return_type")`'s shape
  decides between today's eager-elaborate path and the deferred
  `Expression`-carrier path.

- *Per-call elaboration at dispatch boundary — decided.* The deferred
  return-type elaboration runs as a sibling of the body in a Combine
  joined for the lift-time slot check, rather than as a separate
  substitution pass. The `BodyResult::tail_with_frame` shape today's
  invoke uses widens to a `DeferTo(combine_id)` form for the
  `Deferred(_)` path; the slot check moves into the Combine's finish
  closure.

- *Arity scope — decided.* Arity-1 functor parameters only for the
  initial cut. Multi-parameter functors with cross-parameter
  references in the *return* type
  (`(MODULE_TYPE_OF E1 Type) -> (MODULE_TYPE_OF E2 Type)`) fall out
  for free once the mechanisms above land (the per-call re-elaboration
  is param-name-keyed, not arity-keyed); test coverage focus stays on
  the canonical OCaml-Make shape.

- *Dependent parameter annotations — deferred to
  [Dependent parameter annotations](module-system-dependent-param-annotations.md).*
  Allowing a parameter's type to reference an *earlier* parameter
  (e.g. `(MAKE T: Type elt: T)`, OCaml's
  `module Make (E : ORDERED) (S : SET with type elt = E.t)`) requires
  staged left-to-right dispatch, which is independent of the
  return-type machinery decided here.

## Dependencies

**Requires:**

**Unblocks:**

- [Standard library](standard-library.md) — collection functors like
  `Make` over `ORDERED` are the canonical use case for sharing
  constraints that reference the functor's input module.
- [Dependent parameter annotations](module-system-dependent-param-annotations.md)
  — reuses the `Deferred(TypeExpr)` carrier and per-call re-elaboration
  plumbing established by this item.
