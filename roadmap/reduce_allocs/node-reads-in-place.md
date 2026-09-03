# Node reads in place past the read/intern split

Clone-the-whole-node reads that exist only to keep an intern out of a
`with_node` closure, now that a read holds no borrow for one to collide with.

**Problem.** [`TypeRegistry::with_node`](../../src/machine/model/types/registry.rs)
reads through a persistent-map snapshot and releases the `nodes` cell before the
reading closure runs, so a reader may intern
([type-registry.md § Reading a node](../../design/typing/type-registry.md#reading-a-node)).
The callers written against the earlier arrangement still route around it, each
paying a clone of a node variant whose payload allocates — a schema, a member
list, a field record — where an in-place read would answer:

- `view_type_members` in [ascribe.rs](../../src/builtins/ascribe.rs) clones a
  `TypeNode::AbstractType` once per abstract SIG member to mint the view's
  per-call member; both arms it dispatches to intern
  (`RecursiveGroupWindow::seal_singleton` mints a family, the generative arm
  calls `intern` directly). Its higher-kinded arm feeds the node's own
  `param_names` into the mint, so that field still has to outlive the read — the
  clone of the *whole* node is what covers both today.
- `signature_schema` in the same file, and `member_table` in
  [sig_schema.rs](../../src/machine/model/types/sig_schema.rs), clone a
  `SigSchema` out of a `TypeNode::Signature` because the substitution walk that
  consumes it interns as it recurses.
- `WITH`'s schema read in
  [type_ops/with.rs](../../src/builtins/type_ops/with.rs) clones a
  `TypeNode::Signature` to reach the schema
  [`SigSchema::fold_pins`](../../src/machine/model/types/sig_schema.rs) then
  interns from.
- `intern_union_members` in
  [registry.rs](../../src/machine/model/types/registry.rs) probes for the digest
  under a shared borrow, drops it, and takes a second mutable borrow to insert —
  two hashes and two borrows where one entry lookup would do.

None of these is measured by a shape in `audit/shapes/`, so a regression on them
would not surface in [`observe/alloc.txt`](../../observe/alloc.txt); the
attribution is per-site rather than per-row.

**Acceptance criteria.**

- Every node read in the list above whose only reason to own its node was the
  intern it feeds reads in place through `with_node`, or carries a comment naming
  a different reason it must own.
- A reader that needs one field of a node past the read copies that field, not
  the node.
- `intern_union_members` takes one borrow and one hash lookup on the miss path.
- The `cargo test --lib` node-count assertions in
  [registry/tests.rs](../../src/machine/model/types/registry/tests.rs) are
  unchanged: the sweep moves allocations, not interned content.

**Directions.**

- *Scope — decided.* The sweep is the sites above plus any the same grep for
  `types.node(` in production code turns up; test-side `node(...)` clones are out
  of scope.
- *`view_type_members`' `param_names` — open.* Options: copy the names into a
  `SmallVec` inside the read and mint outside it; or hand the mint a closure that
  runs under the read, now that interning under one is legal. The second keeps
  the copy off the path entirely.
- *Union probe — open.* Options: a single `entry`-style lookup on the persistent
  map; or hoisting the digest probe into `intern` itself so every interning door
  shares one path.

## Dependencies

**Requires:** none — the read/intern split this sweep follows has shipped.

**Unblocks:** none tracked yet.
