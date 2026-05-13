# Type identity stage 3 — `KType::UserType` and per-declaration identity

Stage 3 of the four-stage type-identity arc. Collapses `KType::Struct`,
`KType::Tagged`, and `KType::ModuleType` into a unified `KType::UserType {
kind, scope_id, name }` carrier; threads identity onto every STRUCT / UNION
/ MODULE value via the shipped
[`Bindings::try_register_nominal`](../src/runtime/machine/core/bindings.rs)
dual-write primitive; ships SCC
discovery via
lazy dependency tracking so mutually recursive STRUCT / UNION pairs
elaborate without deadlocking; adds the bare-`Struct` / `Tagged` wildcard
slot mechanism. Subsumes
[per-declaration type identity for structs and tagged unions](type-identity-3-user-type-and-per-decl.md)
entirely (that file is removed when this stage ships).

**Problem.** Flat user-defined STRUCT and UNION declarations report
singleton types ([`KType::Struct`](../src/runtime/model/types/ktype.rs) and
`KType::Tagged`), so two distinct `STRUCT Foo = (a: Number)` and `STRUCT
Bar = (a: Number)` produce values that cannot be distinguished by dispatch
on type. Opaquely-ascribed module types live in their own
`KType::ModuleType { scope_id, name }` variant, so the precedent for
scope-tagged identity exists but does not extend to flat declarations.
Dispatch on `FN (PICK x: Foo)` and `FN (PICK x: Bar)` selects the same
overload bucket when both slot types collapse to `KType::Struct`.

The same surface carries a recursion gap: a self-recursive STRUCT
elaborates via the threaded-set self-reference recognition shipped with
[eager type elaboration](eager-type-elaboration.md), but a mutually
recursive pair (`STRUCT TreeA = (b: TreeB)` / `STRUCT TreeB = (a: TreeA)`)
deadlocks: each member parks on the other's placeholder via the Combine
path in [`struct_def.rs`](../src/runtime/builtins/struct_def.rs) and
neither finalizes. The `#[ignore]`d `mutually_recursive_struct_pair` test
pins the gap.

Three ancillary gaps the same surface admits:

- Builtin signatures that want "any struct" today write `KType::Struct` in
  the slot type — ATTR's `body_struct` overload at
  [`attr.rs:213-225`](../src/runtime/builtins/attr.rs) is the canonical
  example. Collapsing `KType::Struct` into `KType::UserType` removes this
  wildcard meaning unless an explicit wildcard variant lands alongside.
- Anonymous `UNION (...)` declarations (no binder name) produce values
  with nowhere to anchor a declaration-site identity tag.
- `KObject::Struct` and `KObject::Tagged` value carriers report
  `ktype() = KType::Struct` / `Tagged` today by introspecting the value
  shape only; they don't carry the declaring STRUCT's identity.

**Impact.**

- *Per-declaration nominal identity for STRUCT, UNION, and (after [stage
  4](type-identity-4-newtype.md)) NEWTYPE.* `Foo` and `Bar` declared as
  distinct STRUCTs dispatch to different overloads.
- *Mutual recursion elaborates as a unit.* `STRUCT TreeA` /
  `STRUCT TreeB` cross-references resolve via the
  [`Bindings::pending_types`](../src/runtime/machine/core/bindings.rs)
  registry's lazy cycle detection; the `#[ignore]`d
  `mutually_recursive_struct_pair` test moves to passing.
- *Better dispatch-failure errors.* `FN (PICK x: Foo)` rejecting a `Bar`-typed
  value names both declared types, not "expected Struct, got Struct".
- *Wildcard slots keep working.* `KType::AnyUserType { kind: Struct }`
  matches any struct value regardless of `(scope_id, name)`; ATTR's
  `body_struct` slot migrates onto it.
- *Anonymous UNION rejected at dispatch.* `UNION (...)` without a binder
  name fails the existing signature match — no special-case parser logic.

**Directions.**

- *Carrier shape — decided.* `KType::UserType { kind: UserTypeKind,
  scope_id: usize, name: String }` with `enum UserTypeKind { Struct,
  Tagged, Module }`. (`UserTypeKind::Newtype { repr: Box<KType> }` is
  added in [stage 4](type-identity-4-newtype.md).) `KType::Struct`,
  `KType::Tagged`, and `KType::ModuleType` all delete inside this stage —
  the "no broken mid-flight state" constraint forces the variant collapse
  to complete inside one stage.

- *`scope_id` representation — decided.* `scope_id: usize` is `&Scope<'a>
  as *const _ as usize` — the same scheme `KType::ModuleType::scope_id`
  uses today. Captured at `finalize_struct` / `finalize_union` /
  `finalize_module` against the declaring scope (run-root for top-level
  decls, module child-scope for in-module decls).

- *Identity comparison — decided.* Field-wise across `(kind, scope_id,
  name)`. `is_more_specific_than` and `matches_value` (in
  [`ktype_predicates.rs`](../src/runtime/model/types/ktype_predicates.rs))
  compare on these three fields. `AnyUserType { kind }` ranks strictly
  below concrete `UserType { kind, .. }` of the same kind.

- *Value-carrier identity reporting — decided.* `KObject::Struct` and
  `KObject::Tagged` carry their declaration identity as `{ kind, scope_id,
  name }` fields directly on the carrier (not as a `&'a KType` pointer);
  `ktype()` reconstructs the `KType::UserType` on each call. Matches the
  `KObject::KModule(&Module)` precedent at
  [`module.rs:80-82`](../src/runtime/model/values/module.rs) — `Module`
  carries `scope_id` / `name` and reconstructs `KType::ModuleType` for
  `ktype()`. Identity comparison stays field-wise, not pointer-equality;
  the `String::clone` cost is the same cost `ModuleType.name()` pays today.

- *Dual-write — decided.* STRUCT / UNION / MODULE declarations write both
  maps via the shipped
  [`Bindings::try_register_nominal`](../src/runtime/machine/core/bindings.rs):
  identity into
  `types["Foo"] = &KType::UserType{..}`, runtime payload into `data["Foo"]`
  as the existing carrier (`KObject::StructType` / `TaggedUnionType` /
  `KModule`). Transactional — pre-check both maps before either write
  begins; on collision, neither commits.

- *Wildcard variant — decided.* `KType::AnyUserType { kind: UserTypeKind }`
  matches any `KObject` whose `ktype()` reports a `KType::UserType` of the
  same kind. Surface names `Struct`, `Tagged`, `Module` parse to
  `KType::AnyUserType { kind: ... }` via `KType::from_name`. Existing
  builtin signatures that use bare `Struct` / `Tagged` / `Module` migrate
  onto the wildcard with no source-text change.

- *Rendering — decided.* `KType::UserType.name()` returns the bare declared
  name (matching `KType::ModuleType.name()` today).
  `KType::AnyUserType.name()` returns the kind keyword (`"Struct"`,
  `"Tagged"`, `"Module"`) to preserve today's error-message text.

- *`extract_bare_type_name` allowlist — decided.* The helper at
  [`argument_bundle.rs:82-124`](../src/runtime/machine/kfunction/argument_bundle.rs)
  accepts any `KType::UserType { name, .. }` (every kind).

- *SCC discovery via lazy dependency tracking — decided.* `Bindings`
  gains a `pending_types: RefCell<HashMap<String, Vec<String>>>` registry
  that records, for each in-flight type declaration, the type names it has
  parked on. When `elaborate_type_expr` would park a STRUCT / UNION body
  on an unbound type name, it records the edge `(current_decl,
  unbound_name)` in `pending_types`. Each new edge runs a cycle check
  (Tarjan or simpler DFS). When a cycle closes, the cycle detector mints
  `KType::UserType` for every cycle member, installs them into the
  `types` map directly, and wakes all parked bodies; cross-references
  inside the SCC resolve via `Scope::resolve_type` and the elaborator's
  threaded-set short-circuit produces `KType::RecursiveRef` for true
  self-references during `Mu` wrapping. Non-cycle forward refs continue
  to park on `Bindings::placeholders` until the producer finalizes (same
  path value forward references use). This approach rides the existing
  `notify_list` / `pending_deps` machinery — no new scheduler entry point
  — and naturally handles SCC discovery at any nesting depth (top-level,
  inside `MODULE` bodies, inside FN bodies).

- *Anonymous UNION rejection — decided.* The UNION builtin no longer
  registers an anonymous-name overload. A parenthesized-only `UNION (...)`
  form fails the existing signature match and surfaces as
  `DispatchFailed`. Parser-level rejection would require teaching the
  parser about UNION's signature shape and is out of scope; the
  dispatch-level rejection matches every other removed-overload error
  shape.

- *Migration ordering — decided.* Inside stage 3 the cargo-test green
  invariant requires `KType::Struct` / `Tagged` / `ModuleType` migrations
  to complete in one pass. Every construction site (`builtins.rs:74-89`,
  `kobject.rs:85-86`, `struct_value.rs:179`, `tagged_union.rs:156`,
  `ktype_resolution.rs:24-25`, `ascribe.rs:50`, etc.) and every match
  arm (`ktype_predicates.rs:155-162`, `resolver.rs:101-102`) migrates
  before the stage's PR can land green.

## Dependencies

**Requires:**

- [Type identity stage 1.5 — consumer migration and fallback removal](type-identity-1.5-consumer-migration.md)
  — `KType::UserType` resolution rides on `Scope::resolve_type` with the
  fallback gone.
- [Type identity stage 2 — `KObject::TypeNameRef` carrier and `KType::Unresolved` deletion](type-identity-2-typename-ref-carrier.md)
  — collapsing `KType::Struct` / `Tagged` / `ModuleType` into
  `KType::UserType` is cleaner once `KType::Unresolved` is already gone.

**Unblocks:**

- [Type identity stage 4 — `NEWTYPE` keyword and `KObject::Wrapped` carrier](type-identity-4-newtype.md)
  — extends `UserTypeKind` with the `Newtype` variant.
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — `KType::TypeConstructor` extends the same carrier with a
  `UserTypeKind::Constructor` variant (or a sibling variant carrying
  type-parameter shape); the same `(kind, scope_id, name)` identity
  contract applies.
