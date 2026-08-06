# Nested `USING` windows

Make forwarded-bind visibility and the collision guard correct through a stack
of transparent windows, not just the innermost one.

**Problem.** Two gaps, both specific to `USING a SCOPE ( … USING b SCOPE (…) … )`
shapes; single windows are correct.

*Visibility.* A forwarded entry records two positions
([`Scope::binding_position`](../../src/machine/core/scope/registry.rs)): the
innermost window's `(ScopeId, index)` and the final forwarding target's anchor
index ([`BindingIndex::window`](../../src/machine/core/bindings.rs)). A reader
in an *intermediate* block — a later statement of `a`'s block, after `b`'s
block ends — matches neither gate: its chain no longer mentions `b`, and the
anchor is in the host's numbering, not `a`'s. The visibility predicate
(`Bindings::visible`) therefore reads the bind as invisible until `a`'s whole
block ends, where it should be visible from the statement after `USING b`. The
failure is conservative — never wrongly visible, never order-violating — but a
bind forwarded out of an inner block is unusable for the rest of the enclosing
block.

*Collision guard.* The write targets
([`ops.rs`](../../src/machine/core/bindings/ops.rs)'s `value_write_target` /
`type_write_target`) probe only the immediate window's borrowed table before
forwarding. A block-local bind whose name matches an *outer* window's surfaced
member forwards past that window unchecked, and reads inside the outer block
consult the window first — exactly the silent shadowing the single-window guard
exists to reject.

**Acceptance criteria.**

- A bind forwarded from an inner block is visible to the enclosing block's
  later statements, from the statement after the inner `USING` on — on the
  value, `functions`, type, and operator channels alike — and a test pins the
  intermediate-reader position.
- Intra-block lexical ordering stays strict at every nesting level: a forward
  reference inside any block of the stack is a position error, pinned by test.
- A block-local bind or type declaration colliding with any window on its
  forwarding path is rejected with the collision `ShapeError`, pinned by a
  nested-window test on both name channels.
- The `using_scope` suites (value and type channels) stay green.

**Directions.**

- *Entry position as the write-time chain suffix — open.* Generalize the
  two-position entry to the whole window path (innermost window down to the
  host anchor) and gate at the innermost frame the reader's chain shares;
  scopes off the reader's chain fall through outward, ending at the anchor.
  Subsumes the single-window rule as the length-one case. Costs
  `BindingIndex` its `Copy` (an `Rc`-carried frame list rippling through
  `PendingBinding` and the four entry shapes) — or a small-depth inline
  encoding that keeps `Copy` at a fixed nesting cap. Recommended: the chain
  suffix, taking the `Copy` loss.
- *Collision semantics against an outer window — open.* Either the guard walks
  every window on the `write_scope` forwarding path (consistent with the
  single-window rule: a surfaced member is never silently shadowed), or an
  inner block is allowed to shadow an *outer* window's import as ordinary
  inner-scope shadowing, with only the immediate window protected. Recommended:
  walk the whole path; the single-window rule's rationale (the window is
  consulted before the forwarded bind on every read inside its block) applies
  at each level.

## Dependencies

Extends the forwarded-entry position rule described in
[modules.md § Block-scoped opening](../../design/typing/modules.md).

**Requires:** none — the single-window substrate is shipped.

**Unblocks:** none.
