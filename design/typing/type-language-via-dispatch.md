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
carries a `SigiledTypeExpr` variant whose handler
(`fast_lane_sigiled_type_expr`) tail-replaces the slot with a
`Dispatch` of the wrapped `KExpression`. The inner dispatch sees the
same classifier — there is no separate type-context table — so the
inner expression's parts decide its shape:

- `Keyworded` for the keyworded surface (`:(LIST OF Number)`,
  `:(MAP Str -> Number)`, `:(FN (x :Number) -> Bool)`,
  `:(FUNCTOR (T :S) -> M)`) served by the registered `LIST OF` /
  `MAP _ -> _` / `FN` / `FUNCTOR` overloads in
  [`builtins/type_constructors.rs`](../../src/builtins/type_constructors.rs).
- `BareTypeLeaf` / `BareIdentifier` for single-name sigils
  (`:(Number)`, `:(MyType)`).
- `ConstructorCall` for a leaf-Type head with non-empty rest
  (`:(MyStruct 1 2 3)`) — routes Struct / Tagged / Newtype heads
  through their construction primitives.
- `FunctionValueCall` for user-functor application
  (`:(MyFunctor (T = IntOrd))`).

The sigil boundary — "the returned carrier must be type-side
(`KTypeValue`, `Module`, `Signature`, `UserType`, `KFunctor`)" — is
enforced implicitly by the consuming slot's KType machinery rather
than by a dedicated tail at the sigil. A value-side carrier (number,
instance struct, plain function value) flowing out of `:(...)`
reaches a `TypeExprRef` / `Type` / `AnyModule` / `AnySignature` slot
and surfaces a standard `TypeMismatch`. The sigil handler itself does
no extra check; the slot-type rails are the single source of truth.

The legacy positional sigil shape (`:(List Number)` →
`[Type(List), Type(Number)]`) now classifies as `ConstructorCall`
inside the wrapper. Standalone parameterized-type elaboration is
served by the keyworded overloads in every freshly-written
annotation; the field-walker inside `typed_field_list` retains an
inline `try_synth_legacy` path for legacy positional shapes embedded
in `STRUCT` / `UNION` field schemas, because the elaborator there
carries SCC threading context (current declaration name + threaded
set) that the standalone dispatcher does not yet plumb (see
[Open work](#open-work)).

## Binder install: name-keyed vs bucket-keyed

`LET`, `STRUCT`, `UNION`, `SIG`, and `MODULE` register a single name
binding via a `binder_name` extractor and ride the name-keyed
placeholder channel. `FN` and `FUNCTOR` register an *overload* in a
function bucket via a `binder_bucket` extractor — and crucially,
*not* a `binder_name`. The two channels are reflected at the
submission walk as `BinderKey::Name(String)` and
`BinderKey::Bucket(UntypedKey)` (see
[`scheduler/submit.rs`](../../src/machine/execute/scheduler/submit.rs)),
mutually exclusive per binder.

The bucket-keyed channel admits *sibling* overloads under one head
keyword. Two `FN (PICK xs :A) ...` / `FN (PICK xs :B) ...`
declarations each install a distinct entry into the same
`pending_overloads[bucket]` per-bucket vec; the earlier-index entry
is the wake target for a consumer parking on the bucket, and the
later-index siblings remain pending until their own finalize. On
each producer's finalize, only its own entry is removed; if a parked
consumer's first wake doesn't deliver an admitting overload, the
consumer re-dispatches and either picks from the now-live
`functions[bucket]` or re-parks on the next-earliest pending sibling
(see [execution-model.md § Dispatch-time name
placeholders](../execution-model.md#dispatch-time-name-placeholders)).
A name-keyed install would collide on the second sibling — both
`PICK` binders trying to claim `placeholders[PICK]` — which is why
FN / FUNCTOR do not install on the name channel.

## Open work

- [FN/FUNCTOR named identity](../../roadmap/type_language/fn-named-identity.md) —
  load parameter names from the `:(FN ...)` / `:(FUNCTOR ...)` sigil
  surface into `KType::KFunction` / `KType::KFunctor` identity so a
  function-typed slot can enforce that callers use the declared
  parameter names.
- [SCC-aware dispatcher for parameterized self-recursive
  types](../../roadmap/dispatch_fix/scc-aware-dispatcher-for-self-recursive-types.md) —
  plumb the elaborator's threaded set + current-declaration context
  into the dispatcher's bare-Type-leaf and sub-Dispatch paths so a
  self-reference inside `:(LIST OF Tree)` inside `STRUCT Tree`'s body
  short-circuits `Tree` to `RecursiveRef` rather than `UnboundName`.
  Closes the field-walker / dispatcher split and retires the
  `try_synth_legacy` inline path.
- [User-defined TypeConstructor keyworded
  application](../../roadmap/dispatch_fix/user-defined-typeconstructor-keyworded-application.md) —
  give a user `LET Wrap = (TYPE_CONSTRUCTOR T)` a keyworded
  application surface so `:(Wrap Number)` routes through dispatch
  the same way `:(LIST OF Number)` does. Today only the four builtin
  parameterized types (`LIST`, `MAP`, `FN`, `FUNCTOR`) have
  keyworded overloads.
