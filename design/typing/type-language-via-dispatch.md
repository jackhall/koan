# Type language via dispatch

The type language and the value language share dispatch machinery. A
sigiled expression `:(...)` is a parse-context marker: the parser tags
the inner expression as evaluating to a type rather than a value, but
the inner expression itself routes through the same classifier,
candidate-bucket lookup, and binder admission as any value-side call.
Builtin parameterized types (`LIST`, `MAP`, `FN`, `FUNCTOR`) register as
keyworded overloads that produce `KTypeValue` (or paired-carrier)
results. User-defined functors slot in identically — they're
`KFunction` carriers bound to Type-shape names, dispatched through
their declared keyword skeletons.

## Sigil surface

```
:(LIST OF Number)
:(MAP Str -> Number)
:(FN (x :Number, y :Str) -> Bool)
:(FUNCTOR (T :SomeSig) -> Module)
:(MyFunctor T = IntOrd)
```

The sigil contributes no syntactic structure beyond the marker — the
parser does not fold the inner expression's args into `TypeParams::List`
or any positional collapse. Dispatch sees the raw multi-part expression
through the AST wrapper described below, runs the normal candidate walk
against a registered overload, and the picked overload's body returns a
`KObject::KTypeValue(...)` (for structural types) or the paired carrier
(for nominal `UserType` / `Module` / `Signature` identities).

## AST representation

The sigil rides on the slot it occupies via
`ExpressionPart::SigiledTypeExpr(Box<KExpression>)`. The variant wraps
the inner expression as a first-class `KExpression`, so splicing,
lifting, and dispatch-time transformations preserve the type-context
without per-site flag propagation. Pattern-matching against the variant
in the classifier and elsewhere is exhaustive-match-checked by the
compiler — a missed handler is a build error, not a silent fall-through
to the value-side path.

## Fully-uppercase head keywords

`LIST`, `MAP`, `FN`, `FUNCTOR` keep parameterized-type construction in
its own candidate bucket, distinct from any user-defined value-side
overload on short connector words. Routing each parameterized type
through its own uppercase head — `[Keyword("LIST"), Keyword("OF"),
Slot]`, `[Keyword("MAP"), Slot, Keyword("->"), Slot]`, etc. — keeps the
buckets narrow even when user-defined functors overload `OF` or `->`
heavily.

`MAP` is the surface keyword for the dict carrier. The underlying type
identity remains `KType::Dict(K, V)`; only the construction surface
changes.

## Function-type sigil

`:(FN (x :Number, y :Str) -> Bool)` declares parameter names at the
sigil surface, symmetric with the FN declaration form and the
value-side rule that function-value calls are named (no positional
`f 1 2` shape). Lowering drops the names: `KType::KFunction { args,
ret }` stores args positionally, and `:(FN (a :Number) -> Bool)` is
identity-equal to `:(FN (b :Number) -> Bool)`. Until names load into
identity, a function-typed slot can't mechanically enforce that the
call site uses the declared parameter names — see open work.

## Functor-type sigil

Symmetric with the function-type rule:
`:(FUNCTOR (T :SomeSig) -> Module)`. Parameter names appear at the
sigil surface; `KType::KFunctor { params, ret }` stores params
positionally for now.

## User-functor application

`FUNCTOR MyFunctor (T :SomeSig) = ...` binds `MyFunctor` to a
`KFunction` carrier under both the value-side name and the keyword
skeleton declared at `FUNCTOR` time. Applying the functor at any
surface — value-side `(MyFunctor (T = IntOrd))`, sigiled
`:(MyFunctor (T = IntOrd))` — uses one nested-parens kwarg group
inheriting the parameter names from the declaration. Symmetric with
the value-side function-value call shape, which admits one
nested-parens part holding the kwargs.

## Classifier

`classify_dispatch_shape`
([dispatch.rs](../../src/machine/execute/scheduler/dispatch.rs))
doesn't grow a `SigiledTypeExpr` shape. Sigils unwrap at
part-evaluation time: when a part holds
`ExpressionPart::SigiledTypeExpr(inner)`, the dispatch driver
recursively dispatches `inner` through the standard classifier. The
inner expression's parts decide its shape — there's no separate
type-context table.

The sigil boundary asserts the returned `KObject` is a type-side
carrier (`KTypeValue`, `Module`, `Signature`, `UserType`,
`KFunctor`). A value-side carrier in sigil position (number, instance
struct, plain function) is an error surfaced at the boundary. This
covers `TypeConstructorCall` shapes reached through a sigil — they
construct value-side instances, so the boundary rejects them.

The inner classifier walks unchanged. Keyworded inputs
(`:(LIST OF Number)` → `[Keyword(LIST), Keyword(OF), Type(Number)]`)
route through `Keyworded` to the registered `LIST OF` overload.
Positional inputs the parser no longer folds (`:(List Number)` →
`[Type(List), Type(Number)]`) route through `TypeCall` to
`resolve_type_expr`, which produces the same `KType::List(Number)`
carrier. Source annotations work unchanged through this fallback.

## Open work

- [Type-language via dispatch (rollout)](../../roadmap/dispatch_fix/type-language-via-dispatch.md) —
  parser change to emit `SigiledTypeExpr` uniformly (no shape
  inspection), part-evaluation unwrap + type-carrier boundary check,
  registration of `LIST` / `MAP` / `FN` / `FUNCTOR` keyworded
  overloads, parallel tests for the new shapes. The existing
  `TypeCall` arm serves as the positional fallback inside the
  wrapper; migrating annotations to the keyworded form is an optional
  follow-up.
- [FN/FUNCTOR named identity](../../roadmap/type_language/fn-named-identity.md) —
  load parameter names from the `:(FN ...)` / `:(FUNCTOR ...)` sigil
  surface into `KType::KFunction` / `KType::KFunctor` identity so a
  function-typed slot can enforce that callers use the declared
  parameter names.
