# Type identity stage 1.4 — `Scope::resolve_type` + `register_type` rewire (with temporary fallback)

Flips builtin type storage from `data` to `types` and adds the type-side
lookup API. Ships with a **temporary fallback in `Scope::resolve`** so
unmigrated consumers continue to find type names through their current
`data`-side code path — the fallback is removed in
[stage 1.5](type-identity-1.5-consumer-migration.md).

**Problem.** Without the fallback, flipping `register_type`'s storage
target breaks every consumer that today reads `scope.lookup("Number")` and
unwraps a `KObject::KTypeValue`. Migrating those consumers atomically with
the storage flip produces a single oversized PR. The fallback splits the
work: this sub-item moves the storage; the next migrates consumers.

**Impact.**

- *Builtin types live in `types`.* `default_scope`'s 13 `register_type`
  calls populate `bindings.types`, not `bindings.data`.
- *`Scope::resolve_type` available.* Type-class consumers can opt in to
  the new path; old consumers keep working via the fallback until the
  next sub-item.
- *Fallback path scheduled for deletion.* This sub-item lands a transient
  shim. [Stage 1.5](type-identity-1.5-consumer-migration.md) **must**
  delete it.

**Directions.**

- *`Scope::resolve_type` — decided.* `pub fn resolve_type(&self, name:
  &str) -> Option<&'a KType>`. Mirrors `Scope::resolve`'s outer-chain
  walk against `bindings.types()`. Drops the `Ref` before recursing into
  `outer` (same NLL-safe discipline as `resolve_dispatch`). No
  `Placeholder` variant — that lane is reserved for stage 3's
  `pending_types` registry.

- *`Scope::register_type` rewire — decided.* At
  [`scope.rs:170`](../src/runtime/machine/core/scope.rs): arena-allocate
  via `self.arena.alloc_ktype(ktype)`, call
  `bindings.try_register_type(&name, kt_ref)`. The `KObject::KTypeValue`
  wrap is dropped from this call path. Stays infallible (builtin
  registration name-collision is a programming error).

- *`PendingQueue::defer_type` — decided.* New `PendingWrite::Type {
  name, kt: &'a KType }` variant + `defer_type` constructor + drain arm
  delegating to `bindings.try_register_type`. Same `Conflict`-then-queue
  shape as `defer_value` / `defer_function`. Production hot path: the
  drain rarely fires — `register_type` doesn't borrow `data` or
  `functions`, so contention requires a live `types` reader.

- *Temporary fallback — decided.* `Scope::resolve` (and `Scope::lookup`
  via it) gain a final arm after the existing `data` / `placeholders`
  check: if neither map holds `name`, also check `bindings.types()`. If
  found, arena-allocate a fresh `KObject::KTypeValue(kt.clone())` and
  return it as `Resolution::Value`. **This synthesis is transitional**;
  comment block at the fallback site cites [stage 1.5](type-identity-1.5-consumer-migration.md)
  as the removal owner. The synthesis allocates one `KObject` per lookup;
  acceptable cost during the short bridge window.

## Dependencies

**Requires:** none — foundation. Builds on the shipped `Bindings::types`
map plus the `try_register_type` write primitive in
[`bindings.rs`](../src/runtime/machine/core/bindings.rs); the rewire here
calls that primitive from `Scope::register_type` and the new `defer_type`
queue drain.

**Unblocks:**

- [Stage 1.5 — consumer migration + fallback removal](type-identity-1.5-consumer-migration.md)
- [Stage 1.7 — `LET Ty = Number` routes through `register_type`](type-identity-1.7-let-type-value-writes-types.md)
