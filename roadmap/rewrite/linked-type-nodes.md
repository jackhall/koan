# Linked type nodes

**Problem.** The [registry](../../lattice/src/types/registry.rs) stores each
type as a 48 B lattice-private
[`StoredNode`](../../lattice/src/types/stored.rs) in a hashbrown map keyed by
its digest. A node names each child by the child's 16 B digest, so a walk or a
relation looks every child up in the table, one probe per step. The digest
itself is stored again in every node that names it and in each table key. A
shape element is 32 B: a 16 B handle and a one-byte priority class. Because
the class is a `u8`, a shape of 256 or more slots panics when it is interned.
The map
doubles when full in [program storage](../../src/program/README.md). The bump
cannot take the old bucket array back, so each doubling leaves it dead until
the program ends.

**Acceptance criteria.**

- A stored node holds its own digest, and names each child by a `&'run`
  reference to the child's stored node. A shape element names its slot's type
  the same way.
- The registry's table maps a digest to a `&'run` stored node, and it stores
  no second copy of the digest.
- Every walk and relation inside the lattice follows references from node to
  node. A handle is looked up in the table once, where it enters the lattice.
- Two nodes are compared by their digests.
- `TypeRegistry::node` and `TypeNode` are unchanged for every reader outside
  the lattice. A handle stays a bare digest.
- A slot's priority class is a `u32`. A test interns and ranks a 300-slot
  written-order shape and a 300-slot ranked shape.
- The stored node's size and the shape element's size are each pinned by a
  `const` assert.
- `TypeNode`, the node `TypeRegistry::node` returns, carries every field by
  value and no flags.
- No variant's digest recipe changes: the test pinning the digest of one node
  of every variant passes.
- One program loaded and run twice, into two registries, interns the same set
  of digests.
- A registry of 16k interned types, measured by a test that totals its bump,
  holds under 5 MB, dead tables included.
- The type lattice adds no `unsafe`.

**Directions.**

- *What a reader sees — decided.* The table stores a lattice-private node,
  and `TypeNode` is its read view: one conversion lays a view as a stored node
  on an intern miss, reading each child's reference out of the table, and one
  reads a stored node back as a view, each child rewrapped from the digest its
  node holds. A lattice reader that needs only inline fields, such as
  `kind_of`, reads the stored node.
- *The probe flags and out-of-line fields — decided.* The four probe flags
  are a one-byte `Flags` field in each stored variant whose children decide
  them. `Signature`, `SetMember`, `Carrier` and `Lexical` keep the fields their
  hot paths read inline and carry the rest behind one `&'run` reference into
  the registry's region, laid on an intern miss.
- *Where the digest lives — decided.* Once, in the node. The table probes by
  hashing the digest and compares against the digest in the node it reaches,
  so a probe that matches reads the node it returns.
- *Equality by digest — decided.* One content may be stored in two tiers once
  [type branches](type-branches.md) land. A reference is therefore an
  identity only within one table, and a digest is one everywhere.
- *A slot's class — decided.* A `u32` sits in the padding beside the slot's
  reference, so the class costs no space and no shape size panics.
- *The table — deferred* to [the node table](node-table.md). This item keeps
  the bump-backed hashbrown map, holding references.

## Dependencies

**Requires:** none.

**Unblocks:**

- [The node table](node-table.md) — the table is chosen on the workload of
  linked nodes.
