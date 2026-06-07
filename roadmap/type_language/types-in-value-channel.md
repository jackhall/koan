# Carry types in the value-flow channel

Widen the scheduler's value currency from `&KObject` to an `Object | Type` sum so a
type flows as a raw `&KType` — retiring the `KTypeValue` / `TypeNameRef` boxes — and add
a shallow `KKind` to classify a type at dispatch.

**Problem.** The scheduler's value currency is `&'a KObject<'a>`: `BodyResult::Value`
(`src/machine/core/kfunction/body.rs`), the node-store output, `ArgumentBundle` slots,
and lift all speak it. A type produced by a type-operator must ride that `&KObject`
channel, so it is boxed — `KObject::KTypeValue(KType)` for a resolved type (every
`LIST OF` / `MAP` / module / signature / `WITH` result `alloc_object`s a `KTypeValue` in
`src/builtins/type_constructors.rs`, `module_def.rs`, `using_scope.rs`, `ascribe.rs`,
`type_ops/`), or `KObject::TypeNameRef(TypeName)` for a bare user name that can't resolve
at the synchronous `resolve_for` seam (`src/machine/model/ast.rs`). Both ends already
speak `KType`: the producer *has* a `KType`, and the destination `bindings.types`
*stores* `&'a KType` (`alloc_ktype`) read through `Scope::resolve_type`
(`src/machine/core/scope.rs`), which the value resolver never consults. The boxes are a
transport artifact — a type round-trips into a `KObject` only to survive the channel,
then is unboxed at the binding seam. The LET boundary already rejects binding a type
under a value name, so the boxes carry no "type is a value" semantics; `ktype()` even
forks on them (`KTypeValue(Module/Signature)` reports itself, every other `KTypeValue`
reports the flat `TypeExprRef` marker — `src/machine/model/values/kobject.rs`). Those
type-meta markers — `TypeExprRef`, `Type`, `AnyModule`, `AnySignature` — are kinds living
as `KType` variants, unable to express a type constructor's arity.

**Acceptance criteria.**

- The value currency is `Object(&KObject) | Type(&KType)`: a type-operator returns a raw
  `&KType` and a type argument arrives as a `Type` arm.
- `KObject::KTypeValue` and `KObject::TypeNameRef` are removed, along with the box/unbox
  round-trip at the binding seam.
- "Types aren't values" is enforced structurally: a `Type`-arm result binds only a type
  name, and the `ktype()` Module/Signature special-case is gone.
- Modules and signatures travel in the `Type` arm, with `as_module` / `as_signature`
  projecting from it.
- A shallow `KKind` — `{ Proper, Module(sig), Signature, Any }` — classifies a `Type`-arm
  argument at dispatch, and the `TypeExprRef`, `Type`, `AnyModule`, and `AnySignature`
  markers live on `KKind` rather than `KType`.

**Directions.**

- *Value currency becomes a two-arm sum — decided.* `&'a KObject<'a>` widens to
  `Object(&'a KObject<'a>) | Type(&'a KType<'a>)`, threaded through `BodyResult::Value`,
  the node store, lift, and `ArgumentBundle` — arguments carry the `Type` arm too, since
  an argument is a sub-result. The existing channel is retyped in place rather than
  shadowed by a parallel type-result field.
- *Both carriers retire — decided.* `KObject::KTypeValue` and `KObject::TypeNameRef` are
  removed; the arena already stores `KType` (`alloc_ktype`, `src/machine/core/arena.rs`),
  so a resolved type needs no new storage.
- *`KType::Unresolved(TypeName)` transient — decided.* The deferred bare-leaf name becomes
  a `KType` transient, sibling to `RecursiveRef`: it never reaches the dispatch predicates
  and is consumed and replaced by the park-capable `Scope::resolve_type_expr`. The
  `resolve_for` seam mints it, so that seam gains arena access.
- *Modules and signatures are types — decided.* They move to the `Type` arm; `as_module`
  / `as_signature` project from it, and the `ktype()` Module/Signature special-case deletes.
- *Dispatch matching via a shallow `KKind` — decided.* A `Type`-arm argument is classified
  by a new `KKind` — `{ Proper, Module(sig), Signature, Any }` — matched against a
  type-accepting slot's expected kind. `TypeExprRef`, `Type`, `AnyModule`, and
  `AnySignature` move from `KType` into `KKind`. `SigiledTypeExpr` stays a `KType` slot —
  it marks an evaluation strategy (capture-raw vs eval), not a kind — and `NominalKind` /
  `AnyUserType` stay on the Object side, classifying value carriers.
- *Constructor-arity kinds — deferred.* The `* -> *` arity tower (`KKind::Constructor`) is
  deferred to the higher-kinded type-constructor work that exercises it; this item ships
  the shallow kind enum and its extension point only.
- *Scope — the type/value channel only, decided.* This item re-routes how types travel and
  dispatch; it does not touch the nominal-kind axis. The `NominalKind` collapse
  (the shipped struct → record-repr `NEWTYPE` collapse, [tagged-union
  variants](tagged-variant-types.md)) reduces kinds on the Object side and neither
  sequences nor is sequenced by this work.

## Dependencies

The shallow `KKind` leaves an extension point — `KKind::Constructor` and the `* -> *`
tower — for the higher-kinded type-constructor work; that is a soft downstream pull, not
a tracked build-order edge.

**Requires:**

- [Type-only nominal identities](../../design/typing/user-types.md) — the `bindings.types`
  type-side binding table, `alloc_ktype` storage, and `resolve_type` path this work routes
  the channel into.
- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md) — the
  dispatch substrate the `KKind` `Type`-arm matching extends.

**Unblocks:** none tracked yet.
