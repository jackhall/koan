# Type identity stage 3.1 — atomic variant collapse and dual-write

Second of three sub-stages. Stage 3.0 landed the scaffolding (new
variants, identity fields, predicate arms, all coexisting with the old
singletons); stage 3.2 wires SCC discovery and removes the anonymous
`UNION` overload. Stage 3.1 is the atomic collapse — one commit deletes
the old singletons, flips every consumer onto `KType::UserType` and
`KType::AnyUserType`, and routes nominal binders through the
[`Bindings::try_register_nominal`](../src/runtime/machine/core/bindings.rs)
dual-write primitive.

**Problem.** Flat user-defined STRUCT and UNION declarations report
singleton types ([`KType::Struct`](../src/runtime/model/types/ktype.rs)
and `KType::Tagged`), so two distinct `STRUCT Foo = (a: Number)` and
`STRUCT Bar = (a: Number)` produce values that cannot be distinguished
by dispatch on type. Opaquely-ascribed module types live in their own
`KType::ModuleType { scope_id, name }` variant, so the precedent for
scope-tagged identity exists but does not extend to flat declarations.
Dispatch on `FN (PICK x: Foo)` and `FN (PICK x: Bar)` selects the same
overload bucket when both slot types collapse to `KType::Struct`.

A separate gap on the same surface: SIG declarations write `KObject::
KSignature` into `bindings.data` only, never into `bindings.types`.
[`body_type_expr`](../src/runtime/builtins/value_lookup.rs)'s
`scope.lookup` fall-through is what catches SIG names today. The
roadmap's single-home invariant ("type names resolve via `resolve_type`
only") cannot land while SIG sits on the value-side map.

**Impact.**

- *Per-declaration nominal identity for STRUCT, UNION, and MODULE.*
  `Foo` and `Bar` declared as distinct STRUCTs dispatch to different
  overloads. After [stage 4](type-identity-4-newtype.md) the same
  carrier covers `NEWTYPE` too.
- *Better dispatch-failure errors.* `FN (PICK x: Foo)` rejecting a
  `Bar`-typed value names both declared types, not "expected Struct,
  got Struct".
- *Wildcard slots keep working.* `KType::AnyUserType { kind: Struct }`
  matches any struct value regardless of identity; ATTR's `body_struct`
  slot rides the wildcard.
- *Single-home invariant for type names.* Type-classed name lookups go
  through [`Scope::resolve_type`](../src/runtime/machine/core/scope.rs)
  exclusively. The `body_type_expr` `scope.lookup` fall-through deletes;
  the `resolver.rs` value-side `KObject::StructType` / `TaggedUnionType`
  fallback deletes.

**Directions.**

- *Carrier shape — decided.* `KType::UserType { kind: UserTypeKind, scope_id:
  usize, name: String }` with `enum UserTypeKind { Struct, Tagged, Module }`,
  both already on
  [`KType`](../src/runtime/model/types/ktype.rs) from the stage-3.0 scaffolding
  (see [design/type-system.md § Open work](../design/type-system.md#open-work)).
  3.1 deletes `KType::Struct`, `KType::Tagged`, `KType::Module`, and
  `KType::ModuleType` in the same commit.

- *Identity comparison — decided.* Field-wise across
  `(kind, scope_id, name)`. `is_more_specific_than` ranks `UserType
  { kind, .. }` strictly below `AnyUserType { kind }` of the same kind,
  which ranks strictly below `Any`. `matches_value` falls through to
  the derived `PartialEq` for the field-wise check.

- *Dual-write through `try_register_nominal` — decided.* STRUCT
  finalize ([`struct_def.rs:80-99`](../src/runtime/builtins/struct_def.rs)),
  UNION finalize for the named form
  ([`union.rs:79-102`](../src/runtime/builtins/union.rs)), and MODULE
  finalize's Combine closure
  ([`module_def.rs:71-87`](../src/runtime/builtins/module_def.rs)) route
  through `try_register_nominal` so the type-side `&KType::UserType`
  and the data-side carrier write transactionally.

- *SIG dual-write — decided.* SIG finalize
  ([`sig_def.rs:52-67`](../src/runtime/builtins/sig_def.rs)) writes
  `bindings.types[name] = &KType::SignatureBound { sig_id, sig_path }`
  alongside `bindings.data[name] = KObject::KSignature(...)` via
  `try_register_nominal`. Without this write, deleting the
  `body_type_expr` `scope.lookup` fall-through would break every
  SIG-typed name lookup. SIG is not a `UserTypeKind`; its identity
  carrier (`KType::SignatureBound`) is unchanged.

- *Wildcard variant — decided.* `KType::AnyUserType { kind }` (added in
  3.0) is the migration target for every site that used bare
  `KType::Struct` / `KType::Tagged` / `KType::Module` as a slot type —
  the construction-primitive return types in
  [`struct_value.rs`](../src/runtime/model/values/struct_value.rs) and
  [`tagged_union.rs`](../src/runtime/model/values/tagged_union.rs),
  ATTR's `body_struct` slot at
  [`attr.rs:241`](../src/runtime/builtins/attr.rs), MODULE / ascription
  / `MODULE_TYPE_OF` `m: Module` slots, and the
  `(SignatureBound, Module)` specificity arm in `is_more_specific_than`.

- *Value-carrier identity reporting — decided.* `KObject::Struct` /
  `Tagged` / `StructType` / `TaggedUnionType` already carry
  `(scope_id, name)` from 3.0; their `ktype()` arms flip in 3.1 to
  reconstruct `KType::UserType { kind, scope_id, name.clone() }`.
  `KObject::KModule(m, _).ktype()` reconstructs from `m.scope_id()` and
  `m.path`. Schema carriers (`StructType` / `TaggedUnionType`) keep
  reporting `KType::Type` (they are values *of the meta-type*, not
  user-typed values).

- *Anonymous UNION sentinel — deferred.* 3.1 tolerates the sentinel
  `("", 0)` identity that the anonymous `UNION (...)` form constructs;
  [stage 3.2](type-identity-3.2-scc-and-anon-union.md) deletes the
  overload entirely.

- *Mutual recursion — deferred.* The `#[ignore]`d
  `mutually_recursive_struct_pair` test
  ([`struct_def.rs:319-334`](../src/runtime/builtins/struct_def.rs))
  stays ignored after 3.1; SCC discovery is
  [stage 3.2](type-identity-3.2-scc-and-anon-union.md). The variant
  collapse alone does not change the deadlock mechanism.

- *Atomicity — decided.* Rust's exhaustiveness check enforces single-
  commit landing. Deleting `KType::Struct` forces every `match` arm on
  it to migrate: `KObject::Struct.ktype()`, `from_name`, every
  predicate, `extract_bare_type_name`'s allowlist, ATTR's slot type,
  the construction primitive's return type, and the resolver fallback
  all change together. Same for `KType::Tagged`, `KType::Module`, and
  `KType::ModuleType`.

## Dependencies

**Requires:** none. (Stage 3.0 scaffolding has shipped — see
[design/type-system.md § Open work](../design/type-system.md#open-work).)

**Unblocks:**

- [Type identity stage 3.2 — SCC discovery and anonymous-UNION removal](type-identity-3.2-scc-and-anon-union.md)
  — needs the `KType::UserType` variant to mint per-cycle-member identities.
- [Type identity stage 4 — `NEWTYPE` keyword and `KObject::Wrapped` carrier](type-identity-4-newtype.md)
  — extends `UserTypeKind` with the `Newtype` variant.
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — `KType::TypeConstructor` extends the same carrier with a
  `UserTypeKind::Constructor` variant (or a sibling variant carrying
  type-parameter shape); the same `(kind, scope_id, name)` identity
  contract applies.
