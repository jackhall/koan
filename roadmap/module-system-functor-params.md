# Functor parameters — Type-class names and templated return types

**Problem.** Two runtime gaps under the design surface from
[design/module-system.md § Functors](../design/module-system.md#functors):

1. **Type-class FN parameters for module values park.** The design names
   a signature-typed FN parameter as a Type-class binder — "a
   signature-typed FN parameter (`Er: OrderedSig`) is a type-language
   binder, like an OCaml functor's parameter." Declaring
   `FN (LIFT Er: OrderedSig) -> Module` and calling `LIFT some_module`
   parks the dispatch — the Type-class arg name routes through
   type-resolution rather than per-call value binding. Every shipped
   functor test
   ([fn_def/tests/module_stage2.rs:59](../src/runtime/builtins/fn_def/tests/module_stage2.rs#L59),
   [fn_def/tests/module_stage2.rs:209](../src/runtime/builtins/fn_def/tests/module_stage2.rs#L209),
   [ascribe.rs:441](../src/runtime/builtins/ascribe.rs#L441), and
   neighbours) works around this with a lowercase identifier parameter
   (`elem`, `p`).

2. **FN return-type expressions referencing a per-call parameter fail at
   FN-construction.** Declaring
   `FN (LIFT Er: OrderedSig) -> (MODULE_TYPE_OF Er Type) = ...` errors
   with "unbound name `Er`". The
   [`ReturnTypeCapture::TypeExpr`](../src/runtime/builtins/fn_def.rs#L239)
   arm re-elaborates the captured expression against the FN's outer
   scope at Combine-finish;
   [`substitute_params`](../src/runtime/machine/kfunction/invoke.rs#L76)
   exists for FN bodies but is not wired into return-type elaboration.
   [ROADMAP.md](../ROADMAP.md) and
   [design/module-system.md § Functors](../design/module-system.md#functors)
   both describe (2) as shipped; those statements are stale.

**Impact.**

- *OCaml-style functor signatures become writable.* The canonical
  `module Make (E : ORDERED) : SET with type elt = E.t` shape — a
  functor whose result type pins to the input module's abstract type —
  is expressible end-to-end at the koan surface.
- *Standard-library collection functors generalize naturally.* `Make`
  over `ORDERED` and similar shapes carry sharing constraints between
  input and output abstract types at the FN signature.
- *Design-doc / runtime drift closes.* The design surface and the
  runtime agree on the same form; the lowercase-identifier workaround
  in the test suite migrates to the documented surface.
- *Audit-slate pin tightens.* The slate test
  [`type_op_dispatch_does_not_dangle`](../src/runtime/builtins/type_ops.rs)
  currently exercises per-call-arena type-op dispatch via a
  lowercase-identifier parameter; once Type-class params land, the same
  shape exercises the dispatch-boundary substitution path.

**Directions.**

- *Surface form — decided per
  [design/module-system.md § Functors](../design/module-system.md#functors).*
  `Er: OrderedSig` for the parameter; `(MODULE_TYPE_OF Er Type)` for
  the return-type expression that references it. Single-letter
  parameter names follow koan's existing token-classification rules
  (`E` is reserved; `Er`, `Elem` work).

- *Type-class parameter binding — decided.* At call time, parameters
  whose declared `KType` is **type-denoting** dual-write into
  `bindings.types` (via the existing
  [`Scope::register_type`](../src/runtime/machine/core/scope.rs))
  alongside the value-side bind in `bindings.data`. The predicate
  covers `KType::SignatureBound { .. }` (parameter is a module
  ascribed to a signature; registers the module's nominal type
  identity), `KType::Signature` (parameter is a signature value;
  registers the signature itself), `KType::Type` (parameter is a
  `KTypeValue`; registers it directly), and `KType::TypeExprRef`
  (parameter carries a `TypeExpr`; registers the elaborated type). A
  small `is_type_denoting` helper on `KType` keeps the per-call
  [`invoke.rs`](../src/runtime/machine/kfunction/invoke.rs) site
  declarative.

- *Templated return-type substitution — decided.* No separate
  substitution walk. Widen `ExpressionSignature::return_type` from
  `KType` to a two-variant
  `ReturnType { Resolved(KType), Deferred(TypeExpr) }`. Selection is
  decidable at FN-definition by scanning the captured `TypeExpr` for
  any leaf matching a parameter name: present → `Deferred`; absent →
  `Resolved`. Per call, `Deferred` re-runs
  [`elaborate_type_expr`](../src/runtime/model/types/resolver.rs)
  against the per-call scope where dual-write has installed parameter
  names; parameter-name leaves resolve naturally through
  `bindings.types`. Return-type checking on the body's value runs
  against the per-call resolution. Replaces the existing
  [`ReturnTypeCapture::{Resolved, Unresolved, TypeExpr}`](../src/runtime/builtins/fn_def.rs#L239)
  split — Combine-finish at FN-definition either lands
  `Resolved(KType)` (no parameter refs and outer-scope elaboration
  succeeds) or `Deferred(TypeExpr)`. Parameter-type parking machinery
  ([`defer_via_combine`](../src/runtime/builtins/fn_def.rs)) is
  unchanged — parameter types remain definition-time-elaborated for
  dispatch keying.

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
