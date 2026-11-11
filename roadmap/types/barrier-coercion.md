# What a barrier rebuilds

An opaque view's coercion rebuilding every value its view's types admit.

**Problem.** A `:|` view rebuilds each member declared at an unpinned head
parameter at the view's type, arm by arm along the declared type
([members are born coerced](../../src/knot/module/README.md#members-are-born-coerced)),
and refuses a value no arm rebuilds. Two values its types admit reach no arm
that rebuilds them:

- **A cyclic data value.** A container that is a knot's data node is a knot
  member rather than a container word, so the container arm has nothing to
  rebuild and refuses it `Unsupported`, as the test
  `a_cyclic_data_member_refuses_the_barrier` pins.
- **A dict's keys.** The dict arm restamps a dict at `MAP <carrier> -> …` and
  leaves its keys as they are, so a key read back through the view carries the
  source's scalar type rather than the carrier its dict type names. No builtin
  reads a dict's keys yet, so no program sees it.

**Acceptance criteria.**

- `m :| Bag` over the `m` of `a_cyclic_data_member_refuses_the_barrier` builds
  a view whose `ring` holds each of its elements sealed under the view's
  carrier.
- Where a view's dict type names a carrier at its key, a key read back through
  the view satisfies that carrier.

**Directions.**

- *How a cyclic data value is rebuilt — open.* Re-tie every such knot under the
  view's types, each held value coerced, as a
  [copy](../../src/knot/README.md#weight-and-copy) re-ties one; or rebuild the
  node as a plain container where every back edge lies behind a cell the walk
  seals whole, and re-tie only a cycle running through cells it rebuilds.
- *What a carrier key is — open.* Each key sealed under the carrier, so a dict
  holds tagged keys beside its scalar ones; each key kept as its scalar word,
  the dict's key type alone naming the carrier and a key read back retyped to
  it; or a signature refusing a dict type whose key names a head parameter.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
