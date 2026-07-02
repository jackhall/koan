# Fold the USING-window write forwarding

**Problem.** Every write-side `Scope` method opens with the same transparent-USING-window
guard — `if self.bindings.is_borrowed() { return self.write_target().method(...); }` —
repeated ten times in `src/machine/core/scope.rs`: `bind_value`, `register_function`,
`register_type`, `register_type_upsert`, `preinstall_identity`, `drain_pending`,
`install_placeholder`, `clear_placeholders_for_producer`, `install_pending_overload`,
`register_operator_group` (plus the `is_borrowed` no-op pair in the type-identifier memo
accessors). The forwarding semantics — writes through a `Borrowed` window land at the
call site — are re-asserted per method, so a new write method can silently skip the
guard, and one method (`bind_value`) interleaves its member-shadowing check with it.

**Acceptance criteria.**

- The forward-to-call-site decision is expressed in one place; no write-side `Scope`
  method carries its own `is_borrowed` / `write_target` guard.
- `bind_value`'s USING-specific rejection (a local bind colliding with a surfaced module
  member) still errors, exercised by a test.
- Existing USING semantics are otherwise unchanged (block-local binds persist at the
  call site; the pending queue drains at the call site) — existing tests green.

**Directions.**

- *Fold shape — open.* (a) A single resolver (`fn write_scope(&self) -> &Scope`) that
  walks to the effective write target once, with every write method operating on its
  result; (b) move the dispatch into `ScopeBindings` so the `Borrowed` arm forwards
  structurally. Recommended: (a) — smallest diff; (b) also has to re-home the pending
  queue, which `drain_pending` forwards today.
- *Member-shadowing check placement — open.* Keep it inline in `bind_value` ahead of the
  shared resolver, or lift it into the resolver as a per-operation hook. Recommended:
  inline — it is the only channel with window-specific semantics.

## Dependencies

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
