# Tree and view memos

The load's content trees and views, each read once per binder and per view.

**Problem.** The load reads a binder's content tree and a view's types
afresh at every site that asks, and lays what it reads into program storage
each time:

- **An open view chain.** `view_of`
  ([`views.rs`](../../src/dispatch/walk/views.rs)) memoizes only a view whose
  print is `Print::Closed`; over an open tree it calls `view_read`, which
  calls `carried_type` on the source, which calls `view_of` on a source view.
  A chain of k open views costs O(k²), though the file's top-of-file comment
  claims linear time for a chain of views.
- **One `:|` over a closed tree.** The fit is computed three times — `fit_of`
  in `view_tree` ([`trees.rs`](../../src/dispatch/walk/trees.rs)),
  `opaque_view_types` in the walk's opaque-ascription arm
  ([`walk.rs`](../../src/dispatch/walk.rs)), and `opaque_view_types` again in
  `view_read` on the first `view_of` — and `carried_type(tree)` twice, in
  `view_tree` and in the ascription arm.
- **A member chain through a leaf.** `read_member` over `Leaf::Bound` copies
  the leaf's chain plus one label into a fresh slice laid through `lay` per
  step. `resolve_entry` ([`paths.rs`](../../src/dispatch/walk/paths.rs)) and
  `substituted_leaf` loop it over an n-step chain, laying O(n²) bytes into
  program storage per read, though the path entry's `chain` is already a
  `&'o` slice whose prefixes are the chains wanted.
- **A binder read per site.** `slot_tree` guards against cycles but memoizes
  nothing, so `binder_tree` rebuilds a binder's tree at every read site; for a
  module binder it re-lays the body's captures through `captures_of` →
  `composed_captures` each time, so k reads of one module lay k identical
  capture slices.
- **One-element slices.** `modules()` lays a fresh one-element slice into
  program storage for every carried value that is not a rebuilt union.
- **Repeated scans.** `sides()` runs in `lay_nested` and again in `across` per
  container level; `inside()` (`trees.rs`) scans a callee's whole subtree of
  nested shapes where `Shapes::parent` walks up in O(depth); `last_binder`
  scans every slot of a shape for the one its last statement binds.

**Acceptance criteria.**

- Reading a chain of k views, open or closed, costs the load O(k) view reads,
  and the top-of-file comment of `views.rs` states the bound that holds.
- A `:|` over a closed tree computes its fit and its source's carried type
  once.
- Reading an n-step member chain through a leaf lays O(n) bytes, or none
  where the chain is a prefix of a slice the load already holds.
- A binder's tree is built at most once per binding of its body, outside the
  site-keyed fallbacks a read site needs, and a module body's captures are
  laid once per body and binding.
- `modules()` lays nothing for a single carried value.
- `sides()` runs once per container level, `inside()` walks parents, and
  `last_binder` reads the slot its last statement binds without scanning.
- A load test reading a chain of open views, a long member chain through a
  parameter, and one module binder from many sites counts view reads, bytes
  laid and capture slices laid, and each is linear in the input.

**Directions.**

- *The view memo — open.* Memoize `Print::Open` beside `Print::Closed` in the
  pass's `views` table, tagged with the computing unit as closed entries are;
  or memoize by tree identity, as `steps` and `rebuilds` do; or both, where an
  open print cannot tell two trees apart that the load must. The first needs
  an open print to name one view within one walk under one binding.
  Recommended: the open print, if that holds.
- *One fit per `:|` — open.* The ascription arm computes the carried type and
  the fit once and hands both to `view_tree` and the lay; or `view_tree`
  records them on the view tree for `view_read` to read. Recommended: the
  first, which keeps `Tree` the size it is.
- *The binder memo — open.* Memoize `slot_tree` by segment and slot for
  `Scope::Chain`, storing only a tree holding no `Leaf::Unnamed` keyed on the
  read site; or memoize only `captures_of` by body and scope, leaving the
  tree rebuilt but its captures shared. Recommended: the first, falling back
  per site where the tree names one.
- *A leaf's chain — open.* `read_member` takes the chain prefix it extends to
  and reuses it when it is a prefix of the caller's slice; or a `read_chain`
  door reads a whole chain through a leaf in one step, laying the chain once.
  Recommended: `read_chain`, which both loops call.

## Dependencies

**Requires:** none.

**Unblocks:** none.
