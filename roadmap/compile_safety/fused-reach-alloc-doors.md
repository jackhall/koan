# Fused reach-alloc doors

The evidence-tier alloc doors take a value and its reach as one inseparable pair, so a value
cannot be audited under a reach derived for a different value.

**Problem.** The evidence-tier doors — `Scope::alloc_object_reaching`,
`alloc_object_delivered`, `alloc_module_reaching`
([residence.rs](../../src/machine/core/arena/residence.rs)) — take the value and its
`StoredReach` as two independent parameters. Nothing ties the reach they audit against to the value
they store: any caller holding both can pair a value with a reach some other binding derived, and
the audit will happily check the value against evidence that was never about it. A reach that
over-covers the value passes an audit it should fail, which under-pins.

`StoredReach` can no longer be *forged* — [`StoredReach::empty`](../../src/machine/core/bindings.rs)
is visible only within `crate::machine::core` and the type carries no `Default` impl, so every reach
in a caller's hands is one *some* door derived for *some* value. What remains unenforced is the
pairing: the doors' two-parameter shape lets a caller mis-associate two legitimately-derived
reaches.

Only the value channel is in scope. A `KType` is a `Copy` registry handle
([`ktype.rs`](../../src/machine/model/types/ktype.rs)), so the type channel carries no reach at all
and needs no store door.

**Acceptance criteria.**

- No alloc door accepts a value and a `StoredReach` as separate parameters; each takes a single
  fused pair whose construction is the derivation site.
- The fused pair's constructor is confined to the derivation doors, so a caller cannot assemble one
  from a value and a reach it holds side by side.
- Pairing a value with a reach derived for a different value is a compile error, not a runtime
  audit that happens to refuse.
- Every read that yields a binding's reach yields it fused to the value it was derived for, never
  as a bare `StoredReach`.

**Directions.**

- *Shape of the fused pair — decided.* A single `Reached<T>` carrying any storable family, unifying
  the doors on one constructor to confine. The vehicle has shipped in
  [bindings.rs](../../src/machine/core/bindings.rs) — `Reached<T>` carrying `{ value, StoredReach,
  FramePins }`, landed on the binding entry first; this item extends the same type to the alloc-door
  audit signatures.
- *Test affordances — decided.* [`Reached::for_test`](../../src/machine/core/bindings.rs) has
  shipped alongside the vehicle: a `#[cfg(test)]`-gated `pub(crate)` constructor assembling a fused
  pair from explicit parts, mirroring `StoredReach::for_test` and confined so it is not a production
  back door around the derivation-site confinement (the route the `Default` impl became one). This
  item reuses it rather than adding its own.
- *Reach of the change — decided.* The doors are `pub(crate)` and their callers are countable
  (`scope/registry.rs`'s fused bind doors, `scope/reach.rs`, `ascribe.rs`, `dispatch/exec.rs`, plus
  test fixtures), so this is a contained signature change, not a cross-cutting refactor.

## Dependencies

The forgery route this item's residual sits behind — minting a `StoredReach` from nothing outside
`machine::core` — is already closed; see
[memory-model.md § Move-in residence audits](../../design/memory-model.md#move-in-residence-audits).

**Requires:** none — the `Reached<T>` vehicle it extends has shipped in
[bindings.rs](../../src/machine/core/bindings.rs).

**Unblocks:** none tracked yet.
