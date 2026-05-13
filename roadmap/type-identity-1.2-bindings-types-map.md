# Type identity stage 1.2 — `Bindings::types` map and `try_register_type`

Adds the dedicated type-binding storage that the rest of the stage 1
sub-items and the entire type-identity arc plug into. Unused at land time
— wiring happens in
[stage 1.4](type-identity-1.4-scope-resolve-type-and-rewire.md).

**Problem.** Type bindings and value bindings share one `Bindings::data`
map. The
[parser already discriminates token kind](../src/parse/tokens.rs)
(`ExpressionPart::Type` vs `ExpressionPart::Identifier`) but the binding
home does not. Stage 1 separates the homes; this sub-item lands the type
half.

**Impact.**

- *Dedicated type-binding storage.* `types: RefCell<HashMap<String, &'a
  KType>>` alongside `data` and `functions` in
  [`bindings.rs`](../src/runtime/machine/core/bindings.rs). All later type
  lookups go through `Bindings` (consistent with how value lookups go
  through `Bindings::data()` today — Scope-level lookup helpers delegate
  to the façade, never bypassing it).
- *Stage 1.4 wires `Scope::register_type` into the new home* via the
  `try_register_type` helper this sub-item ships.

**Directions.**

- *Field shape — decided.* `types: RefCell<HashMap<String, &'a KType>>`,
  plus `pub fn types(&self) -> Ref<'_, HashMap<String, &'a KType>>` for
  reads.
- *`try_register_type` — decided.* `Bindings::try_register_type(name: &str,
  kt: &'a KType) -> Result<ApplyOutcome, KError>`. Returns `Err(Rebind)`
  on collision; `Ok(Conflict)` on `RefCell` contention. Clears any
  matching placeholder on success.
- *Helper structure — decided.* Separate private `try_apply_type` rather
  than extending the existing `try_apply` with `Option<&KObject>` skip
  logic. Cleaner; the type-side path has zero overlap with the dual-map
  invariant. May collapse later if a refactor warrants it.
- *Borrow order across three maps — decided.* `types → functions → data`.
  Each acquired conditionally (`types` only when writing types). Verified
  deadlock-free against existing read sites: `resolve_dispatch` holds
  `functions` only (drops before outer-chain recursion);
  [`Scope::resolve`](../src/runtime/machine/core/scope.rs) reads `data`
  non-mutably; the future `resolve_type` reads `types` non-mutably with
  the same drop discipline.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Stage 1.3 — `try_register_nominal` dual-write primitive](type-identity-1.3-try-register-nominal.md)
- [Stage 1.4 — `Scope::resolve_type` and `register_type` rewire](type-identity-1.4-scope-resolve-type-and-rewire.md)
