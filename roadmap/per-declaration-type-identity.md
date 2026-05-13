# Per-declaration type identity for structs and tagged unions

**Problem.** [`KType`](../src/runtime/model/types/ktype.rs) carries opaquely-ascribed
module abstract types as `KType::ModuleType { scope_id, name }`, so two
opaque ascriptions of the same source module mint observably distinct types.
Flat user-defined struct and tagged-union types do not get the same
treatment: every `STRUCT` value reports the same singleton `KType::Struct`,
and every `UNION` variant value reports the same singleton `KType::Tagged`,
regardless of which declaration produced them. Two distinct user struct
declarations — `STRUCT Foo = (a: Number)` and `STRUCT Bar = (a: Number)` —
produce values that report the same `KType` and so cannot be distinguished
by dispatch on type, even though they are nominally separate. The
discriminator the singletons rely on for runtime construction (`KObject::StructType`
+ schema match, `KObject::TaggedUnionType` + variant tag) lives one level
below `KType`, so dispatch cannot select on it. The `Tagged` and `Struct`
variants in `KType` document this gap with prose comments rather than
encoding the identity.

The same STRUCT/UNION declaration surface also carries a recursion gap.
A self-recursive STRUCT (`STRUCT Tree = (children: List<Tree>)`) elaborates
cleanly via the threaded-set self-reference recognition shipped with
[eager type elaboration](eager-type-elaboration.md), but a mutually
recursive pair (`STRUCT TreeA = (b: TreeB)` /
`STRUCT TreeB = (a: TreeA)`) deadlocks: each STRUCT parks on the other's
placeholder via the Combine path in
[`struct_def.rs`](../src/runtime/builtins/struct_def.rs) and neither ever
finalizes. The
[`mutually_recursive_struct_pair`](../src/runtime/builtins/struct_def.rs)
test is `#[ignore]`d until batch SCC pre-registration lands. Self-recursive
UNION uses the same threaded-set mechanism today (the binder seeds its own
name) but inherits the same gap for the mutually recursive case.

**Impact.**

- *Per-declaration nominal identity for structs and tagged unions.* `Foo`
  and `Bar` declared as separate `STRUCT`s become distinct types at the
  `KType` level, so `FN (PICK x: Foo) -> ...` and `FN (PICK x: Bar) -> ...`
  dispatch separately even when their schemas coincide.
- *Better type-mismatch errors.* Today a dispatch failure on a struct
  argument can only report "expected `Struct`, got `Struct`" because the
  singleton tag carries no declaration identity. With per-declaration
  identity the error names the declared type by name.
- *Substrate for per-type method dispatch.* Future work that wants
  declaration-keyed registration of operations (struct-specific methods,
  union-specific destructors, type-class-style dispatch outside the module
  system) has a stable identity to key on.
- *Mutually recursive STRUCT/UNION declarations elaborate as a unit.*
  `STRUCT TreeA = (b: TreeB)` / `STRUCT TreeB = (a: TreeA)` elaborates
  without deadlocking; cross-references become `KType::RecursiveRef` at
  the binder boundary the same way the self-recursive case already does.
  The currently `#[ignore]`d
  [`mutually_recursive_struct_pair`](../src/runtime/builtins/struct_def.rs)
  test moves to passing without special-casing.

**Directions.**

- *Carrier shape — open.* The `KType::ModuleType { scope_id, name }`
  design — a declaration-site address plus a name — is the natural analog.
  A `KType::Tagged { scope_id, name }` and `KType::Struct { scope_id, name }`
  pair would mirror it directly: the declaring scope address gives stable
  identity for the run, the name handles textual disambiguation. Open
  question whether to share one carrier (`KType::UserType { kind: TaggedKind |
  StructKind, scope_id, name }`) or keep two parallel variants.
- *Construction-site capture — open.* `STRUCT Foo = (...)` and
  `UNION Bar = ...` need to record the scope address at declaration time
  and thread it onto every value produced. The construction primitives
  that currently mint `KObject::StructType` / `KObject::TaggedUnionType`
  are the single capture point; the question is what slot on those values
  carries the identity forward to `KObject::ktype()`.
- *Dispatch consequences — open.* `KType::matches_value` and
  `is_more_specific_than` need to compare on the new identity, mirroring
  what `KType::ModuleType` already does. Any builtin or user-fn slot
  declared as `Struct` (today: matches any struct) needs a migration story
  — either widen to a wildcard slot that accepts any declared struct, or
  treat the bare `Struct` shape as a parse error in slot position.
- *Recursive declarations — decided.* Schemas with self-references
  elaborate to `KType::RecursiveRef(name)` per [eager type elaboration
  with placeholder-based recursion](eager-type-elaboration.md); this
  work's `{ scope_id, name }` carrier inherits the binder name, and
  `RecursiveRef` resolution finds the concrete identity by walking the
  enclosing schema-binder context. The recursion encoding does not
  change shape when this work lands.
- *Mutual recursion via SCC pre-registration — decided.* At top-level,
  batch-register every binder name in a strongly-connected STRUCT/UNION
  declaration group as a scheduler placeholder before any body
  elaborates, and seed the elaborator's threaded set with all SCC member
  names. Any back-reference from any SCC member's body to any other
  member returns `RecursiveRef(name)` directly. SCC discovery rides on
  the existing scheduler — each binding's body elaboration is scheduler
  work; mutual references inside the SCC short-circuit, mutual
  references outside the SCC park on each other's placeholders the same
  way value forward references park. Today's per-binder threaded-set
  seeding (each STRUCT/UNION seeds only its own name in
  [`struct_def.rs`](../src/runtime/builtins/struct_def.rs) /
  [`union.rs`](../src/runtime/builtins/union.rs)) was a deliberate
  narrowing — batch-wide seeding without SCC discovery would mis-mark
  non-recursive cross-references as `RecursiveRef`. SCC discovery closes
  the gap without that hazard. The work ships under this item rather
  than under [eager type elaboration](eager-type-elaboration.md) because
  the STRUCT/UNION declaration surface that records the new identity
  carrier is the same surface that needs to batch-register the SCC.
- *Module-system relationship — decided.* This is not part of the
  module-system staged work — opaque-ascription types and user-defined
  types are conceptually distinct (one is an abstraction barrier, the
  other is a nominal declaration), and the design doesn't require them to
  share an implementation. The `KType::ModuleType` carrier may be the
  right model to extend, but the work itself is type-system upkeep that
  ships independently of any module-system stage.

## Dependencies

**Requires:**

**Unblocks:**

No hard prerequisites and no roadmap items downstream. Module-system stage 1
shipped the `KType::ModuleType { scope_id, name }` carrier that is the
analog and possible model to extend, but is not a hard prerequisite — the
work could land first against `STRUCT` / `UNION` and inform the carrier
shape rather than the other way around.
