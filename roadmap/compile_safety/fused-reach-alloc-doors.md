# Fused reach-alloc doors

The evidence-tier alloc doors take a value and its reach as one inseparable pair, so a value
cannot be audited under a reach derived for a different value.

**Problem.** The evidence-tier doors — `Scope::alloc_ktype_reaching`, `alloc_object_reaching`,
`alloc_module_reaching` ([arena.rs](../../src/machine/core/arena.rs)) — take the value and its
`StoredReach` as two independent parameters. Nothing ties the reach they audit against to the value
they store: any caller holding both can pair a `KType` with a reach some other binding derived, and
the audit will happily check the value against evidence that was never about it. A reach that
over-covers the value passes an audit it should fail, which under-pins.

`StoredReach` can no longer be *forged* — [`StoredReach::empty`](../../src/machine/core/bindings.rs)
is visible only within `crate::machine::core`, the type carries no `Default` impl, and the reads
outside `core` go through `Scope::type_reach` / `Scope::reach_for_resolved_type`, which collapse the
miss and pick the channel behind the wall. So every reach in a caller's hands is one *some* door
derived for *some* value. What remains unenforced is the pairing: the doors' two-parameter shape
lets a caller mis-associate two legitimately-derived reaches. `home_resolved_return_type`
([exec.rs](../../src/machine/core/kfunction/exec.rs)) shows the fused shape already — it takes the
resolver's `TypeHit`, so the reach it audits under can only be the one derived for that very type —
but it is one consumer, not the door, and the doors underneath it still assemble from parts.

**Acceptance criteria.**

- No alloc door accepts a value and a `StoredReach` as separate parameters; each takes a single
  fused pair whose construction is the derivation site.
- The fused pair's constructor is confined to the derivation doors, so a caller cannot assemble one
  from a value and a reach it holds side by side.
- Pairing a value with a reach derived for a different value is a compile error, not a runtime
  audit that happens to refuse.
- `Scope::type_reach` / `Scope::reach_for_resolved_type` yield the fused pair rather than a bare
  `StoredReach`; `TypeHit` either is that pair or is expressed in terms of it.

**Directions.**

- *Shape of the fused pair — open.* Either generalize the existing `TypeHit` (today a `KType` + its
  reach) into a `Reached<T>` carrying any storable family, or give each family its own pair type.
  `Reached<T>` unifies the three doors on one constructor to confine; per-family types keep each
  door's audit signature concrete. Recommended: `Reached<T>`, since the three doors already differ
  only in the family they store.
- *Test affordances — open.* `StoredReach::for_test` is `#[cfg(test)]`-gated and assembles a token
  from explicit parts; the fused pair needs an equivalent, and it must not become a production
  back door around the confinement (which is how the `Default` impl became one).
- *Reach of the change — decided.* The doors are `pub(crate)` and their callers are countable
  (`scope.rs`'s fused bind/register doors, `ascribe.rs`, `resolve_type_identifier.rs`,
  `kfunction/exec.rs`, plus test fixtures), so this is a contained signature change, not a
  cross-cutting refactor.

## Dependencies

The forgery route this item's residual sits behind — minting a `StoredReach` from nothing outside
`machine::core` — is already closed; see
[memory-model.md § Move-in residence audits](../../design/memory-model.md#move-in-residence-audits).

**Requires:** none — the reach-token confinement it builds on has shipped.

**Unblocks:** none tracked yet.
