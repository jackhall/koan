# Consolidate the arena store-side erasure

Route the six per-type `T<'a> → T<'static>` store transmutes through one audited
method instead of repeating the erasure per type.

**Problem.** [`RuntimeArena`](../../src/machine/core/arena.rs)'s store side is six
near-identical `transmute` pairs — `alloc_function`, `alloc_scope`, `alloc_module`,
`alloc_signature`, and the `KObject` / `KType` `alloc_local` impls — each doing the
same `T<'a> → T<'static>` erasure on the way in and `&mut T<'static> → &T<'a>` on the
way out, each carrying its own SAFETY comment. The `CycleGated` trait already owns
that erasure for `KObject` and `KType`; the other four allocate inline, so the same
transmute argument is written and audited in six places rather than one.

**Impact.**

- The `T<'a> → T<'static>` store erasure lives behind one audited method, the way the
  read-side re-attach already lives behind one `ScopePtr` — a single transmute to
  reason about instead of six.
- A new arena-stored type joins by implementing one trait method rather than copying a
  transmute pair, so the erasure cannot drift between the per-type sites.

**Directions.**

- *Consolidation shape — open.* Either extend `CycleGated` (already the owner for
  `KObject` / `KType`) to `KFunction` / `Scope` / `Module` / `Signature`, or add a
  sibling `ArenaStored` trait. The obstacle is transmuting a lifetime-parameterized
  type in a generic context: `mem::transmute` needs concrete equal-sized types, so the
  helper needs a `type Static` associated type plus `transmute_copy` (with a `size_of`
  equality assertion) or an equivalent encoding.
- *Worth-it gate — open.* The win is one audited erasure, not fewer lines. Decide
  whether the generic body (with its `transmute_copy` / `size_of` plumbing) is genuinely
  more auditable than six explicit, monomorphic pairs before committing — if it is not,
  this item closes as "keep the explicit pairs with their SAFETY comments."
- *Validation — decided.* Re-run the full Miri slate before and after under tree
  borrows via the `miri` skill.

## Dependencies

**Requires:** none — builds on the shipped arena lifetime-erasure core, but adds no new
prerequisite.

**Unblocks:** none tracked yet.
