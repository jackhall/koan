# Type identity stage 3.0 — scaffolding for `KType::UserType`

First of three sub-stages that together replace the original "type
identity stage 3" entry. Stage 3.1 performs the atomic variant collapse;
stage 3.2 adds SCC discovery and removes the anonymous `UNION` overload.
Stage 3.0 is pure scaffolding — every step independently green — so the
3.1 collapse can flip in a single commit without simultaneously inventing
the replacement variants and the predicate behavior.

**Problem.** The variant collapse in stage 3.1 is non-decomposable: every
`match` arm on the deleted variants (`KType::Struct`, `KType::Tagged`,
`KType::ModuleType`, the bare `KType::Module`) has to migrate together
because Rust's exhaustiveness check refuses a half-migration. Landing the
collapse in one commit without scaffolding forces the same commit to
introduce four new variants, predicate arms, identity fields on every
value carrier, and a new dual-write call shape. The diff is large enough
that a single mistake snowballs. Stage 3.0 lands the new shapes alongside
the old ones — coexisting compile-clean — so 3.1 is purely a delete +
rewire pass.

**Impact.**

- *Variants exist but are unconstructed except by `KType::from_name`.*
  The new types live in the `KType` enum and in match arms that already
  exhaustively cover the enum (`name()`); every other consumer keeps
  matching the old singletons.
- *Value carriers carry identity but report old singletons.* The
  `(scope_id, name)` fields on `KObject::Struct` / `Tagged` /
  `StructType` / `TaggedUnionType` exist and are populated at finalize
  time; `ktype()` still returns `KType::Struct` / `KType::Tagged` /
  `KType::Type`. Identity is dormant until 3.1 flips the arms.
- *Predicate behavior for the wildcard is wired but unreferenced.*
  `KType::AnyUserType { kind }` is admissible at dispatch (matches any
  `KObject::Struct` / `Tagged` / `KModule` of the right kind), strictly
  more specific than `Any`, and incomparable with everything else.
- *`Bindings.pending_types` field exists.* No writer in 3.0; stage 3.2
  populates it.

**Directions.**

- *Enum addition — decided.* `enum UserTypeKind { Struct, Tagged,
  Module }` lives next to `KType` with `Clone, PartialEq, Eq, Debug`
  derives plus a `surface_keyword(&self) -> &'static str` method
  returning `"Struct"` / `"Tagged"` / `"Module"`.

- *New `KType` variants — decided.* `KType::UserType { kind:
  UserTypeKind, scope_id: usize, name: String }` and `KType::AnyUserType
  { kind: UserTypeKind }`. `name()` returns the bare name for `UserType`
  and the kind keyword for `AnyUserType`.

- *`from_name` rewire — decided.* The surface names `"Struct"`,
  `"Tagged"`, `"Module"` map to `KType::AnyUserType { kind: ... }` in
  [`from_name`](../src/runtime/model/types/ktype_resolution.rs). The old
  singletons still exist for every other site that constructs them
  directly; 3.1 deletes them.

- *Predicate arms — decided.* `is_more_specific_than`, `matches_value`,
  `accepts_part` (in
  [`ktype_predicates.rs`](../src/runtime/model/types/ktype_predicates.rs))
  gain arms for `AnyUserType { kind }`. The `(SignatureBound, Module)`
  arm migrates in 3.1, not 3.0.

- *Value-carrier identity fields — decided.* `KObject::Struct`,
  `KObject::Tagged`, `KObject::StructType`, `KObject::TaggedUnionType`
  grow `(scope_id: usize, name: String)` fields (or rename `type_name`
  to `name` where it exists). `KObject::TaggedUnionType` becomes
  named-field form `{ schema, name, scope_id }`. Every construction
  site populates the identity from the declaring scope's `as *const _
  as usize`. `ktype()` is *not* updated in 3.0 — old arms continue to
  return `KType::Struct` / `Tagged` / `Type`.

- *`Bindings.pending_types` field shape — decided.* `pending_types:
  RefCell<HashMap<String, PendingTypeEntry>>` on the
  [`Bindings`](../src/runtime/machine/core/bindings.rs) façade with a
  read handle. `PendingTypeEntry` minimal in 3.0 (the SCC machinery
  in 3.2 expands it). No write path in 3.0.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Type identity stage 3.1 — atomic variant collapse and dual-write](type-identity-3.1-variant-collapse.md)
  — consumes every new variant, every new identity field, and every new
  predicate arm in a single atomic migration.
