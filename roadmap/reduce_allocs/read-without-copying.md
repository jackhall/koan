# Read a name or a type node without copying it

**Problem.** Two readers on the execute path take an owned copy of something they only
inspect, and between them they are the largest attributed share of the `wide_step` term.

[`LabelInterner::resolve`](../../src/machine/model/labels.rs) hands back an owned `String`
per call, and its total form `render` is reached from paths a correct program never prints
from:

- [`resolve_name`](../../src/machine/execute/decide/resolve.rs) builds
  `Resolution::Unbound(String)` — which is not an error arm but the ordinary "this bare name
  is not a value" fall-through every keyworded dispatch takes. Its consumer in
  [`resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs) turns it into a
  `Lean::Dead(name)`, cloning the string a second time. `TypeChannel::Unbound` in the same
  file is the same shape on the type side.
- [`let_binding`](../../src/builtins/let_binding.rs)'s body renders the binder's spelling
  before it knows whether any diagnostic will quote it.
- [`render_label`](../../src/machine/model/types/ktype.rs) and
  [`walk_field_list`](../../src/machine/model/types/typed_field_list.rs) render on their
  ordinary path too.

A dhat difference of the wide pair puts these at 59 allocations per step — the largest
single share of the term. The lazy door already exists:
[`LabelInterner::display`](../../src/machine/model/labels.rs) renders straight into a
caller's formatter, so a message that names a label costs the message's own buffer alone.

[`TypeRegistry::node`](../../src/machine/model/types/registry.rs) is the second: it returns
the node a handle names *by clone*. The node is shallow — scalar payload plus child handles —
but a `Record`'s own clone allocates, so every predicate probe that only inspects a node's
shape allocates to do it: `accepts_part` and `accepts_carried`
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)) under
`slot_admits_strict`, and the construction lane's repr check. That is 15 allocations per step,
and it is the reason a dispatch that matches nothing still allocates per candidate.

**Acceptance criteria.**

- `Resolution::Unbound` and `TypeChannel::Unbound` carry the unbound name as a symbol; the
  spelling is rendered where a diagnostic is built, and no consumer of either clones a name.
- Registering, binding, and dispatching a name that resolves render no label text: a dhat
  difference of `audit/shapes/wide_n{10,100}.koan` attributes no allocation to
  `LabelInterner::resolve`.
- The type registry answers a shape or child-handle question through a borrowing read, and a
  dhat difference of the same pair attributes no allocation to `TypeRegistry::node`.
- The `wide_step` and `deep_frame` terms in [`observe/alloc.txt`](../../observe/alloc.txt)
  fall by the attributed share, and `tests/allocation_baseline.rs` holds the new figures.

**Directions.**

- *What an unbound outcome carries — decided.* The symbol the lookup already held. Rendering
  is the diagnostic's job, through `LabelInterner::display`, so the miss path costs nothing a
  correct program pays.
- *The registry's borrowing read — open.* A `with_node(handle, |node| …)` closure door, which
  keeps the `RefCell` borrow inside the registry and needs no lifetime on the caller; or a
  `Ref<'_, TypeNode>` handed out, which composes better but exports the borrow. Recommended:
  the closure door, with `node()` kept for the callers that genuinely need an owned node.
- *Whether `resolve` keeps its owning form — open.* Every remaining caller may turn out to be
  a diagnostic that `display` serves, in which case `resolve`'s `String` goes with them.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
