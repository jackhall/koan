# Type identity stage 1.3 — `Bindings::try_register_nominal` dual-write primitive

Lands the transactional dual-write helper that STRUCT / UNION / MODULE
declarations will use in
[stage 3](type-identity-3-user-type-and-per-decl.md) — shipped now so the
scaffolding is in place when stage 3 migrates nominal declarations onto it.

**Problem.** Stage 3's `KType::UserType { kind, scope_id, name }` requires
nominal declarations to write *both* maps atomically: identity into `types`,
runtime carrier into `data`. The transactional pre-check (reject if either
side collides) needs to live in `Bindings` since it owns both maps.

**Impact.**

- *Stage 3's nominal-declaration migration is a wire-up, not a design.*
  STRUCT / UNION / MODULE finalize paths call the helper this sub-item
  ships; the transactional contract is already enforced.

**Directions.**

- *API — decided.* `Bindings::try_register_nominal(name: &str, kt: &'a
  KType, obj: &'a KObject<'a>) -> Result<ApplyOutcome, KError>`.
- *Transactional contract — decided.* Pre-check both `types[name]` and
  `data[name]` vacant; on either collision, return `Err(Rebind)` with no
  partial write. Acquire `types` then `data` borrows in that order (no
  `functions` involvement — nominal carriers are not callable verbs).
  Best-effort placeholder clear on success.

## Dependencies

**Requires:** none — foundation. Builds on the shipped `Bindings::types`
map and `try_apply_type` write path in
[`bindings.rs`](../src/runtime/machine/core/bindings.rs); the dual-write
primitive layered here borrows `types` then `data` atomically.

**Unblocks:**

- [Type identity stage 3 — `KType::UserType` and per-declaration identity](type-identity-3-user-type-and-per-decl.md)
  — STRUCT / UNION / MODULE finalize paths migrate onto this primitive.
