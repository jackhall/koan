# Slot kinds and function signatures

Type-position slot kinds, the `UnresolvedType` surface-survives-bind
carrier, and function signature types. Part of the [`KType` reference](README.md).

## Type-position slot kinds

`OfKind(Proper)` is the meta-type for argument slots that capture a parsed type-name
token (`ExpressionPart::Type(_)`). The slot resolves to a `KType` handle flowing raw in the value
channel's `Type` arm, carrying the elaborated type — name, nested
parameters, and (for recursive types) the member handle of a sealed nominal —
so parameterized types like `:(LIST OF Number)` and recursive types like `Tree`
survive the parser → dispatch boundary as a single canonical value. Used by
FN's return-type slot, by NEWTYPE and UNION's name slots, and by `type_call`'s
verb slot. Slots that want only a bare name (NEWTYPE/UNION) check the elaborated
shape on the inner type; the validation lives at the consuming builtin rather
than at the slot kind.

### `UnresolvedType` — surface form survives bind

A type-position value whose surface `TypeName` doesn't resolve at
`ExpressionPart::resolve_for` time — a bare-leaf name outside
[`KType::from_name`](../../../src/machine/model/types/ktype_resolution.rs)'s
builtin table (`Point`, `Ordered`, `MyList`, or an unknown name like
`SomeWeirdName`) — rides through bind on a dedicated
[`Carried::UnresolvedType` / `Held::UnresolvedType`](../../../src/machine/model/values/carried.rs)
arm carrying the surface `TypeIdentifier` verbatim, rather than as a resolved
`KType` handle in the `Type` arm — so no type handle ever denotes an unresolved
name. See
[elaboration.md § Layers](../elaboration.md#layers) § Layer 5 for where this
carrier sits in the pipeline and the eventual scope-aware elaboration
hop.

The guarantee this gives consumers: diagnostics can quote the user's
identifier exactly as written, not the elaborated canonical form. A FN
declared `FN (DOIT) -> SomeWeirdName = (1)` whose return-type name never
binds surfaces a `ShapeError` mentioning `SomeWeirdName` verbatim, not a
synthesized rewrite. The same applies to user-bound aliases like `MyT` —
the carrier remembers `MyT` as written, and only at the resolution boundary
does it elaborate to the underlying type. Pinned by
`fn_return_type_surface_name_preserved_in_error` in
[`src/builtins/fn_def/tests/return_type.rs`](../../../src/builtins/fn_def/tests/return_type.rs).

## Function signatures

`FN` syntax requires both per-parameter types and a return type:

```
FN (sig) -> ReturnType = (body)
```

Each parameter slot in `<sig>` is written as `name: Type`. A bare identifier
without `: Type` is a parse error — there is no implicit `Any` default. Use
`: Any` to opt a slot out of type-checking. Parameter types are checked at
dispatch via the same `Argument::matches` path as builtins, so a call whose
arguments don't satisfy the signature surfaces as
[`KErrorKind::DispatchFailed`](../../../src/machine/core/kerror.rs); the same call shape
with different parameter types routes to a different overload by
slot-specificity (see below).

The return type is non-optional and runtime-enforced. The scheduler injects a
check at user-fn slot finalization that surfaces
[`KErrorKind::TypeMismatch`](../../../src/machine/core/kerror.rs) (with a `<return>` arg
name and a frame naming the called function) on mismatch. `Any` is the
no-enforcement fast path for sites that genuinely don't care. `MATCH` and `TRY`
arms share this check: their mandatory `-> :T` rides the same slot carrier (a
[`ReturnContract`](../../../src/machine/core/kfunction/body.rs) — `Function` for a
call, `Arm` for a function-less arm) and the same Done-arm check, so every arm
agrees on `T` and the expression's value carries `T` for downstream dispatch (see
[execution/calls-and-values.md § Arms as own blocks](../../execution/calls-and-values.md#arms-as-own-blocks)).

FN itself registers with a return type of `Any` — there's no "any function"
KType to declare, since a function with no signature has nothing to dispatch
on; the constructed function's projected `ktype()` carries the real shape at
runtime.

