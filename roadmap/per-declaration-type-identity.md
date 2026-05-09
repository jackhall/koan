# Per-declaration type identity for structs and tagged unions

**Problem.** [`KType`](../src/dispatch/types/ktype.rs) carries opaquely-ascribed
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
