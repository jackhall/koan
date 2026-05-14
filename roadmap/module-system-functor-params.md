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
- *Type-class parameter binding — open.* Two options. (a) Dual-write
  the per-call binding to both `bindings.data` (value lookups) and
  `bindings.types` (type-expression lookups via
  `elaborate_type_expr`), so the Type-class name resolves identically
  through both paths. (b) Keep the per-call binding value-only and
  teach `elaborate_type_expr` to consult value-bound modules when it
  sees a Type-class leaf that resolves to a `KModule` value.
  Recommended: (a), for symmetry with how `LET Ty = (LIST_OF Number)`
  already dual-writes through
  [`Scope::register_type`](../src/runtime/machine/core/scope.rs).
- *Templated return-type substitution — open.* Extend
  `ReturnTypeCapture::TypeExpr(t)` to also capture the FN's parameter
  list. At each dispatch boundary, walk the captured `TypeExpr` and
  rewrite any leaf matching a parameter name into a
  `Future(KObject::KTypeValue)` carrier whose value is the resolved
  type-of-arg, then schedule the substituted expression as a
  sub-Dispatch through the eager-sub-Dispatch rails. Open whether the
  walk reuses
  [`substitute_params`](../src/runtime/machine/kfunction/invoke.rs#L76)
  (operates on `KExpression`) or a parallel TypeExpr walk.
  Recommended: the parallel TypeExpr walk — `TypeExpr` is small,
  param-name-keyed substitution is a structural rewrite, and reusing
  the `KExpression`-shaped helper would force a round-trip through the
  expression carrier just to walk a type shape.
- *Arity scope — decided.* Arity-1 functor parameters only for the
  initial cut. Multi-parameter functors with cross-parameter references
  (`(MODULE_TYPE_OF E1 Type) -> (MODULE_TYPE_OF E2 Type)`) fall out for
  free once both mechanisms land (the substitution walk is
  param-name-keyed, not arity-keyed); test coverage focus stays on the
  canonical OCaml-Make shape.

## Dependencies

**Requires:**

**Unblocks:**

- [Standard library](standard-library.md) — collection functors like
  `Make` over `ORDERED` are the canonical use case for sharing
  constraints that reference the functor's input module.
