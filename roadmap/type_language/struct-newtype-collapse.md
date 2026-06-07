# Collapse `STRUCT` into a record-repr `NEWTYPE`

Phase one of shrinking `NominalKind` toward a single `Newtype` primitive: a struct
becomes a nominal identity over a `KType::Record` repr, retiring the separate `Struct`
carrier, schema, and kind.

**Problem.** `STRUCT` and `NEWTYPE` are two nominal declarators encoding one concept —
a nominal identity over a structural shape. A struct's schema is *already* a record
(`NominalSchema::Struct(Record<KType>)`, `src/machine/model/types/recursive_set.rs`),
and a newtype is a nominal identity over a transparent repr
(`NominalSchema::Newtype(Box<KType>)`). The only things separating a struct from a
newtype whose repr is a record are the value carrier
(`KObject::Struct { set, index, fields }` vs `KObject::Wrapped { inner, type_id }`,
`src/machine/model/values/kobject.rs`), the construction surface
(`(Point {x = 1, y = 2})` named record-literal body vs `(Distance 3.0)` positional
wrap), and a redundant `NominalKind::Struct` / `NominalSchema::Struct` /
`ProjectedSchema::Struct` triple plus the struct arm of `apply_constructor`
(`src/machine/execute/dispatch/apply_callable.rs`) and `dispatch_construct_struct`.

**Acceptance criteria.**

- A struct is a `NominalKind::Newtype` member whose repr is a `KType::Record`, carried
  as `Wrapped { inner: Rc<KObject::Record>, type_id }`.
- `Wrapped.inner` rides an `Rc` (not an arena `&'a` reference), so a record-newtype returned
  across a frame boundary survives `lift_kobject`'s `Rc::clone` the way `KObject::Struct`'s
  `Rc<IndexMap>` fields did.
- The `KObject::Struct` carrier, `NominalSchema::Struct` schema variant,
  `ProjectedSchema::Struct` projected-schema variant, `NominalKind::Struct` kind, and the
  `dispatch_construct_struct` constructor path are removed.
- `(Point {x = 1, y = 2})` builds the record and wraps it, and `(Point r)` wraps an
  existing record value, both routed through the one newtype construction path —
  `newtype_construct` evaluates the value expression, type-checks it against the repr, and
  wraps it.
- `.x` field access on an ex-struct resolves through ATTR's `Wrapped` fall-through over a
  record repr.

**Directions.**

- *`STRUCT` declarator retired — decided.* `STRUCT Name = (fields)` goes away; the
  spelling is `NEWTYPE Name = :{fields}`. koan has no users, so existing declarations
  migrate freely.
- *Construction surface — decided.* Positional construction consumes the trailing parts
  `expr.parts[1..]` directly as the value expression, so `(Point r)` is the canonical
  positional form and the mandated nested-paren `Point (r)` is dropped (redundant parens
  still parse as ordinary grouping). A record-repr newtype constructs with either the named
  record-literal body `(Point {x = 1, y = 2})` (build + wrap) or a positional value
  `(Point r)` (wrap an existing record); both evaluate the value expression and wrap it, so
  there is no separate named-vs-positional construction path. Scalar reprs keep
  `(Distance 3.0)`. The surface change is newtype-only — tagged-union construction stays
  `(Outcome (err "x"))` (product-side only).
- *Value carrier — decided (full collapse, heap-`Rc` inner).* `KObject::Struct` is removed;
  ex-structs are `Wrapped { inner: Rc<KObject::Record>, type_id }`. `inner` becomes an `Rc`
  rather than the arena `&'a` reference the scalar-newtype `Wrapped` used, so the carrier
  lifts by `Rc::clone` — keeping a returned record-newtype alive without the frame-reanchor
  write boundary (which is not yet landed and not phaseable into a thinner slice; see
  [Scheduler run/frame lifetime split](../refactor/scheduler-lifetime-split.md) /
  [Type-enforced frame re-anchor](../refactor/type-enforced-frame-reanchor.md)). `type_id`
  stays `&'a` (declaration-stable). Field access pays one peel indirection through the
  wrapper — accepted for the single-carrier simplification.
- *`NominalKind::Struct` and the `:Struct` wildcard — decided (drop both).* The kind is
  removed, so all record/scalar nominals are `Newtype`. The `:Struct` wildcard is dropped
  rather than re-pointed: "record-shaped vs scalar-wrapper" is a *repr-shape* distinction
  (`Newtype(Record)` vs `Newtype(scalar)`), which the eagerly-recorded `NominalKind` axis
  cannot encode. If "match any record-repr nominal" is ever wanted, it belongs on a future
  repr-shape wildcard, not the kind axis.
- *Recursion through a record repr — decided (needs declarator wiring).* The sealing
  helpers `seal_recursive_refs` / `resolve_set_locals` already descend into `KType::Record`
  field types (`src/machine/model/types/recursive_set.rs`), so a sealed recursive record
  repr is navigable. But the NEWTYPE declarator (`src/builtins/newtype_def.rs`) currently
  fills its repr without threading its binder name or sealing — so `NEWTYPE Node =
  :{value :Number, next :Node}` does *not* thread the `next` back-edge today. The declarator
  is reworked to elaborate + seal its repr through the shared `finalize_nominal_member` /
  `seal_recursive_refs` path the STRUCT declarator uses, so a self-reference seals to a
  `SetLocal` exactly as a self-recursive struct did.
- *Scope — product side only, decided.* This item dissolves only `Struct` into `Newtype`.
  The sum-side `Tagged` collapse is a separate sequel, downstream of [anonymous structural
  unions](anonymous-unions.md) and [tagged-union variants as dispatchable
  types](tagged-variant-types.md): a tagged union decomposes into the anonymous-union
  *join* of per-variant `Newtype`s, each variant's nominal identity replacing its tag
  string. That sequel is a different mechanism than this one — an untagged union can't
  carry the tag discriminant — and recursive unions there still need the union name to
  thread a `SetLocal` back-edge, the same nominal anchor a record-repr `Newtype` keeps.

## Dependencies

**Requires:**

- [Type-only nominal identities](../../design/typing/user-types.md) — the shipped
  `NominalKind` / `NominalSchema` / `Wrapped` substrate and the record-sigil `:{…}` →
  `KType::Record` resolution this work collapses.

**Unblocks:**

- [Tagged-union variants as dispatchable types](tagged-variant-types.md) — the sum-side
  sequel that builds each variant as a `Newtype` over its payload, on the product-side
  primitive this item establishes.
- [Constructors as first-class function values](constructor-as-first-class-function.md) —
  reifies the single `Newtype` construction path this item establishes into a callable
  `KObject::KFunction`.
