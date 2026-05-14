# Type identity stage 3.2 — SCC discovery and anonymous-UNION removal

Third and final sub-stage of the type-identity-3 arc. Stages
[3.0](type-identity-3.0-scaffolding.md) and
[3.1](type-identity-3.1-variant-collapse.md) shipped per-declaration
identity; 3.2 closes the two remaining gaps the same surface admits:
mutually recursive STRUCT / UNION pairs deadlock during elaboration, and
the anonymous `UNION (...)` form mints values whose sentinel identity
doesn't fit the per-declaration contract.

**Problem.** A mutually recursive STRUCT pair (`STRUCT TreeA = (b: TreeB)`
/ `STRUCT TreeB = (a: TreeA)`) deadlocks: each member's field elaboration
parks on the other's placeholder via the Combine path in
[`struct_def.rs`](../src/runtime/builtins/struct_def.rs), neither
producer terminalizes, and neither finalize runs. The `#[ignore]`d
`mutually_recursive_struct_pair` test at
[`struct_def.rs:319-334`](../src/runtime/builtins/struct_def.rs) pins
the gap.

The anonymous `UNION (...)` overload at
[`union.rs:167-179`](../src/runtime/builtins/union.rs) produces a
`KObject::TaggedUnionType` carrying sentinel identity `("", 0)`. Stage 3.1
tolerates this — `ktype()` on a tagged value from an anonymous union
reports `KType::UserType { kind: Tagged, scope_id: 0, name: "" }`, which
cannot collide with any named UNION but breaks the per-declaration
contract two anonymous unions are supposed to satisfy.

**Impact.**

- *Mutually recursive STRUCT / UNION pairs elaborate as a unit.* `STRUCT
  TreeA` / `STRUCT TreeB` cross-references resolve through the
  `Bindings::pending_types` registry's cycle detector. The `#[ignore]`d
  `mutually_recursive_struct_pair` test passes.
- *N-way mutual recursion works at any nesting depth.* SCC discovery
  fires whenever an in-flight binder parks on another in-flight binder,
  whether they live at run-root, inside a `MODULE` body, or inside a
  `FN` body.
- *Anonymous `UNION (...)` rejected at dispatch.* `UNION (some: Number)`
  with no binder name fails the existing signature match and surfaces as
  `DispatchFailed`. Every tagged value in the language carries a real
  per-declaration identity.
- *Idempotent finalize, exercised by a unit test.* The SCC cycle-close
  path runs each member's finalize directly; the original parked
  Combine fires later and observes both `bindings.types[name]` and
  `bindings.data[name]` already populated, short-circuiting cleanly.

**Directions.**

- *SCC mechanism — decided.* `Bindings.pending_types` (added empty in
  3.0) is populated by the
  [elaborator](../src/runtime/model/types/resolver.rs) at park time: an
  in-flight binder whose schema parks on an unresolved type name records
  the edge `(current_decl, unbound_name)`. Each new edge runs a DFS
  cycle check from the just-added source. The
  [`Elaborator`](../src/runtime/model/types/resolver.rs) gains
  `current_decl_name: Option<String>` and `current_decl_kind:
  Option<UserTypeKind>` fields that `struct_def` and the named-form
  `union` body seed before elaboration.

- *Cycle-close action — decided.* When a cycle closes, the detector
  iterates members and runs each member's finalize directly. Per-entry
  state on `pending_types` (the `schema_expr` `KExpression` plus the
  binder's name and kind) is captured when the binder first parks, so
  the cycle-close has everything it needs without re-entering the
  scheduler. Each finalize call writes through the existing
  `try_register_nominal` path. Because every cycle member's identity is
  in `bindings.types` before the next member finalizes, the parked
  bodies re-elaborate against `Scope::resolve_type` directly — no
  `RecursiveRef` wrap inside SCC members, only between true self-references.

- *Idempotent finalize — decided.* `finalize_struct`, `finalize_union`,
  and the MODULE-finalize Combine closure check
  `bindings.types[name].is_some() && bindings.data[name].is_some()` at
  entry. If both are populated (the cycle-close already ran the
  finalize), the function returns `BodyResult::Value(<existing data
  carrier>)` immediately. A unit test per finalize site exercises the
  double-invocation path directly:
  - `finalize_struct_is_idempotent_when_both_maps_populated` in
    [`struct_def.rs`](../src/runtime/builtins/struct_def.rs).
  - `finalize_union_is_idempotent_when_both_maps_populated` in
    [`union.rs`](../src/runtime/builtins/union.rs).
  - `module_finalize_is_idempotent_when_both_maps_populated` in
    [`module_def.rs`](../src/runtime/builtins/module_def.rs).
  Each test calls finalize twice on the same `(name, scope)` pair and
  asserts the second call returns the same `&'a KObject<'a>` pointer
  the first call wrote. Pins the short-circuit so a future SCC-path
  refactor cannot silently lose the idempotency contract.

- *Anonymous UNION removal — decided.* The second
  `register_builtin_with_pre_run` call at
  [`union.rs:167-179`](../src/runtime/builtins/union.rs) deletes. The
  tests at `union.rs:219-274` and
  [`tagged_union.rs:233-275`](../src/runtime/model/values/tagged_union.rs)
  that exercise the anonymous form either delete (the dedicated
  anonymous-form test) or rewrite to use a named binder. A new test
  `anonymous_union_fails_dispatch` asserts the bare-parens form
  produces `KErrorKind::DispatchFailed`.

- *MODULE participates in `pending_types`? — decided: no.* MODULE
  bodies park on the outer scheduler's dispatch deps for sibling
  member references, not on type-name resolution inside elaboration.
  Two MODULEs referencing each other's abstract types (`MODULE A = (LET
  T = B.Type)`, `MODULE B = (LET T = A.Type)`) is a separate problem
  that does not surface today and is not addressed here.

## Dependencies

**Requires:**

- [Type identity stage 3.1 — atomic variant collapse and dual-write](type-identity-3.1-variant-collapse.md)
  — needs `KType::UserType` to mint per-cycle-member identities and
  `try_register_nominal` as the finalize write path.

**Unblocks:** none. Tail of the type-identity-3 arc.
