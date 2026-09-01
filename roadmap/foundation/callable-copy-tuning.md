# Callable copy tuning

The pricing and recursion levers the first callable-copy seam
([lazy-closures.md § Lazy close](../../design/lazy-closures.md)) deliberately
leaves unpulled.

**Problem.** The callable copy consolidates an escaping closure at exactly two
surfaces: a top-level `KFunction` crossing the escape seam out of its own
captured region, and an explicit `CLOSE OVER` capture. Everything else still
pins:

- A **foreign crossing** pins unconditionally, mirroring the substrate rule in
  [`copy_or_pin`](../../src/machine/model/values/kobject.rs). A callable that
  once downgraded to `Pin` is therefore never re-consolidated by a later
  escape crossing — its captured region is foreign to every later host — so
  frame-death evacuation is the only backstop for a pin that could have been
  priced away earlier.
- A **callable cell inside a copied container** rides verbatim: a record of
  closures crossing the seam severs nothing for its cells, and the container's
  memoized copy cost counts no environment, so even a would-be-cheap
  consolidation is never offered.
- A captured chain holding a **`USING … SCOPE` window** (`Borrowed` bindings)
  reads as not-ready and pins, though the module scope it aliases may be
  closed and cheap.
- A **module value** never consolidates. Its child scope is `MODULE`-kinded and
  the readiness gate declines that kind, so an escaping module pins its
  producer chain however small and closed the body is.

**Acceptance criteria.**

- A ready callable crossing a priced seam consolidates under the same cost
  comparison whether its captured region is the crossing's host or foreign to
  it, and the copy releases only regions the product no longer reaches.
- Deep-copying a container consolidates its ready callable cells, and the
  cost the pricing chooser reads for that container accounts for those cells'
  environments.
- The chooser's tuning constants are justified by a measured workload
  (smallest `n` that shows the trend), recorded where the constant is defined.
- An escaping module value whose body scope is closed and claim-free
  consolidates on the same terms a closure does.

**Directions.**

- *Foreign-crossing pricing — open.* Reuse the home-crossing α against the
  chain regions' allocated totals, or give foreign crossings their own
  constant (the retention profiles differ: a foreign pin is often already
  shared with other holders, so a copy may release nothing).
- *Environment cost in container memos — open.* Fold a callable cell's chain
  cost into the substrate's memoized copy weight at section time (stale once
  the scope grows — monotone under-count), or read it live at the seam for
  callable-bearing containers only.
- *`Borrowed` window scopes — open.* Copy the façade through the module
  scope's memo entry (a window and its module value share one copy), or keep
  the not-ready pin.
- *Module scopes — open.* Teach the engine to rebuild an `AnnouncedWindow` so a
  `MODULE`-kinded scope passes the readiness gate, or keep the gate and
  consolidate a module value by rebuilding only what its members reach. A module
  scope also carries a group record, which
  [Operator registrations in a copied environment](operator-registry-copy.md)
  owns.

## Dependencies

Region evacuation owns the frame-death backstop for pins these levers never
reach; this item covers the per-crossing decisions. The operator-registry
decline is carved out into
[Operator registrations in a copied environment](operator-registry-copy.md).

**Requires:**


**Unblocks:** none.
