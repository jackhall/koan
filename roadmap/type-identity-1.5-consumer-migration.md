# Type identity stage 1.5 — Consumer migration and fallback removal

Migrates every type-name lookup site to `Scope::resolve_type` and **removes
the temporary `Scope::resolve` fallback** that synthesizes
`KObject::KTypeValue` on demand at
[`scope.rs`](../src/runtime/machine/core/scope.rs). This sub-item must land
before stage 1 can be considered shipped — leaving the fallback in place
defeats the migration's purpose.

**Problem.** Builtin types live in `bindings.types` (post-stage-1.4 storage
flip), but unmigrated consumers still find them through the
[`Scope::resolve`](../src/runtime/machine/core/scope.rs) fallback's
on-the-fly `KObject::KTypeValue` synthesis. Production reads pay the
synthesis cost on every type-name lookup, and the fallback blurs the
binding-home invariant.

**Impact.**

- *Token-kind discrimination flows through to the lookup site.* Type
  tokens resolve through `types`; identifier tokens through `data`. No
  fallback chain inside one body.
- *Fallback synthesis deleted.* `Scope::resolve` returns to its
  pre-fallback shape: `data` then `placeholders` then outer.

**Directions.**

- *`value_lookup.rs` `TypeExprRef` overload — decided.* Splits into two
  bodies (`body_identifier` keeps `scope.lookup`; new `body_type_expr`
  consults `scope.resolve_type`). The existing parameterized-type
  rejection (`List(_) | Dict(_, _) | KFunction { .. } | Mu | RecursiveRef`)
  runs on the incoming bundle slot *before* the new lookup, so structural
  rejections fire identically.

- *`elaborate_type_expr` bare-leaf arm — decided.* At
  [`resolver.rs:88`](../src/runtime/model/types/resolver.rs): try
  `el.scope.resolve_type(name)` first; on hit, `ElabResult::Done(kt.clone())`.
  Fall back to `el.scope.resolve(name)` for the `KSignature` /
  `StructType` / `TaggedUnionType` arms (those carriers stay in `data`
  until stage 3 dual-writes). The `KObject::KTypeValue` arm deletes.

- *Value-carrier consumers stay on `Scope::lookup` — decided.* The
  `attr.rs` `body_type_lhs` overload, `type_call.rs`, and `attr.rs`'s
  `access_module_member` look up the *value* side of a nominal binding
  (`KModule` / `StructType` / `TaggedUnionType`), not the `KType`
  identity. Until stage 3 dual-writes those bindings, the value carriers
  live in `data` alone — `scope.lookup` is the right API. Doc-comment
  each call site noting why it stays. (Stage 3 introduces `KType::UserType`
  and may revisit; out of scope here.)

- *Fallback removal — decided.* `Scope::resolve` reverts to its
  pre-stage-1.4 shape. The transitional comment block deletes with it.

- *Tests — decided.* `run_tests.rs` and `fn_def/tests/module_stage2.rs`
  test assertions over `scope.lookup("ty")` for `LET ty = Number`-style
  bindings stay on `lookup` for now — those bindings still write to
  `data` (Identifier-class LHS path). The
  [stage 1.7](type-identity-1.7-let-type-value-writes-types.md) routing
  change is what flips those tests.

- *Bindings-routes invariant — decided.* All migrated lookups go through
  `Bindings::types` (via `Scope::resolve_type`'s outer-chain walk). No
  call site reaches into `bindings.types().get(...)` directly outside
  the façade.

## Dependencies

**Requires:**


**Unblocks:**

- [Type identity stage 2 — `KObject::TypeNameRef` carrier](type-identity-2-typename-ref-carrier.md)
  — operates against a fallback-free lookup surface.
- [Type identity stage 3 — `KType::UserType` and per-declaration identity](type-identity-3-user-type-and-per-decl.md)
  — `KType::UserType` resolution rides on the migrated `Scope::resolve_type`
  path.
