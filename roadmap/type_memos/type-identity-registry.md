# Content-addressed type identity

Identity is a wide content-hash digest of the type — equal digest means equal type in the
common case, with collision detection and a repair path so distinct types that hash alike
stay distinct, and thread-local digests that merge without a shared lock.

**Problem.** Nominal-type identity is the only koan type identity keyed on a raw pointer:
[`KType::SetRef`](../../src/machine/model/types/ktype.rs) / `Variant` / `RecursiveGroup`
and [`KObject::Tagged.set`](../../src/machine/model/values/kobject.rs) carry
`Rc<RecursiveSet<'a>>` and compare by `Rc::ptr_eq` / hash by `Rc::as_ptr`, where `Module`
/ `Signature` / `AbstractType` already key on minted ids (`scope_id()` / `sig_id()`). Two
costs follow. `KType` equality and hashing recurse the type (the manual `PartialEq` /
`Hash` in `ktype.rs`), an O(size) walk on every dispatch compare. And raw-pointer identity
is fragile: it is unstable if a set is ever relocated, and it carries an ABA hole — a freed
region address can be reused by a later allocation and alias a different type.

**Acceptance criteria.**

- Type identity is a wide (≥128-bit) content-hash digest, computed bottom-up at mint (at
  seal for recursive sets) as a pure function of type content — independent of interning
  order.
- `KType::SetRef` / `Variant` / `RecursiveGroup` and `KObject::Tagged.set` carry a `Copy`
  identity tag `Unique(u128) | Collided(u128)`; no `Rc<RecursiveSet>` remains in `KType` /
  `KObject` / `Scope`.
- `KType` equality compares the `u128` when both operands are `Unique`, and falls back to a
  structural walk when either is `Collided`; hashing keys on the `u128` for both tags.
- A `digest → type` table detects the rare distinct-types-same-digest collision (structural
  compare on a digest match) and tags the newcomer `Collided(u128)` with the same hash —
  never re-keying it — so structurally-identical types still compare equal everywhere.
- The digest is content-only, so two independently-built digest tables agree on every
  non-colliding type with no reconciliation.
- The detection table is per-frame; a type's entry is reclaimed with its declaring frame
  unless a lifted result carries it, in which case it merges into the parent frame's table —
  only types that escape to the run frame persist run-long.
- A nominal type declared inside an FN body (including ≥2 frames deep) stays bindable and
  constructible after the declaring frame returns; existing dispatch/identity tests pass.

**Directions.**

- *Wide content-hash digest, computed bottom-up — decided.* `u128`-or-wider; shallow
  hash-cons over children's digests; recursive sets interned at seal over the finite SCC
  presentation (`SetLocal(i)` as the index literal), member identity `(set_digest, index)`,
  self-recursion the degenerate single-member case. The digest *is* the identity, so width
  sets how often a collision (and thus a repair) occurs — wide enough that it is negligible.
- *Content-only, order-independent — decided.* The digest depends only on type content, never
  on interning order, so independently-built tables agree without reconciliation. This is the
  enabling decision for a future parallel runtime: thread-local digest tables merge without a
  shared hot-path lock (no GIL).
- *Collision detection via a `digest → type` table — decided.* On a digest match the table
  structurally compares; equal → same type, distinct → a collision, disambiguated at intern
  before any result carries the digest.
- *Per-frame table, merged on lift — decided.* The `digest → type` table lives per call
  frame, like the call region; on return, the types carried by lifted results merge into the
  parent frame's table (a digest-keyed union that carries any `Collided` marks), reaching the
  run-frame table only by escaping every frame — so a frame's transient types are reclaimed
  with it. Identity stays the content-hash digest (`Copy`, no `Rc`, no region pointer); this is
  the table's lifecycle, not the identity. The same digest-keyed union is the primitive a
  future thread-join reuses.
- *Collision handling via a `Collided` tag, not a rewrite — decided.* A `KType`'s identity is
  `Unique(u128) | Collided(u128)`. On a digest match the per-frame table structurally
  verifies; equal → same type; a true collision tags the newcomer `Collided(h)` with the
  *same* hash (the lift walk bringing it in flips the tag). A comparison falls back to a
  structural walk whenever either operand is `Collided`, and is a `u128` equality otherwise.
  Re-keying the hash is rejected: it would diverge from a fresh computation of the same type
  elsewhere and need a persistent rewrite record plus an region-wide rewrite; tagging touches
  only the lifted type and keeps the hash content-derived.
- *Phasing — decided.* Foundation phase (carries the risk): digest mint, the per-frame
  `digest → type` table, and merge-on-lift — run in shadow mode, where digests are computed
  and checked for agreement while pointer identity remains the identity in use. Mechanical
  phases, each leaving the verify-koan slate green: compiler-guided swap of equality,
  hashing, and carriers to the `Copy` tag, then removal of the `Rc` fields.
- *Cross-thread collision reconciliation — deferred.* Per-frame `Collided` marks merge up on
  lift; joining independent run-frame tables under future parallelization (so a hash contested
  in one thread is `Collided` in the join) generalizes this and is deferred.
- *`Box<KType>` interning — deferred.* `KType` is built by value at hundreds of sites with no
  region in scope; out of scope here.

## Dependencies

Builds on the shipped region / lift substrate
([design/per-call-region/README.md](../../design/per-call-region/README.md),
[design/memory-model.md](../../design/memory-model.md)); the identity rules it changes are
described in [design/typing/ktype/README.md](../../design/typing/ktype/README.md), which should be updated
when it ships.

**Requires:** none — builds on shipped substrate.

**Unblocks:**

- [Memoized subtype matching](memoized-subtype-matching.md) — its match cache is keyed on
  these digests and homed in this registry's per-type entries.
