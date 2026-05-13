# Type identity stage 1.1 — `RuntimeArena::alloc_ktype`

Foundation step for stage 1 of the four-stage type-identity arc. Adds an
arena slot so the `Bindings::types` map (landed in
[stage 1.2](type-identity-1.2-bindings-types-map.md)) can store `&'a KType`
directly without coupling to the `KObject` sub-arena.

**Problem.** The forthcoming `Bindings::types: RefCell<HashMap<String, &'a
KType>>` needs `&'a KType` references with the arena's lifetime. The
[`RuntimeArena`](../src/runtime/machine/core/arena.rs) only allocates
`KObject` today; routing `KType` storage through a sentinel
`KObject::KTypeValue` wrap couples the type map's lifetime to the object
arena and forces a `.as_ktype()` unwrap on every read.

**Impact.**

- *Stage 1.2's type-binding storage can carry `&'a KType` references
  directly* without coupling to the object arena or routing through a
  sentinel wrap.
- *Stage 3's `KType::UserType` lands without an extra indirection.* The
  wrap-free storage matches the shape stage 3 wants for nominal identity.

**Directions.**

- *API — decided.* `pub fn alloc_ktype<'a>(&'a self, t: KType) -> &'a KType`
  on `RuntimeArena`, backed by a new `ktypes: typed_arena::Arena<KType>`
  sub-arena. `KType` has no lifetime parameter today — direct typed-arena
  alloc, no transmute, no `with_escape` involvement.
- *`alloc_count` test helper — decided.* Extends to include `ktypes.len()`
  so existing leak audits cover the new slot.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Stage 1.2 — `Bindings::types` map and `try_register_type`](type-identity-1.2-bindings-types-map.md)
- [Stage 1.4 — `Scope::resolve_type` and `register_type` rewire](type-identity-1.4-scope-resolve-type-and-rewire.md)
