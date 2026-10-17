# Two flag bits in a type handle

**Problem.** A type handle is a bare 128-bit content digest
([identity](../../lattice/src/types/identity.md#identity-a-handle-is-a-content-digest)),
so two questions a hot path asks of a handle cost a table lookup each, though
both are fixed for the program's life. *Is this type the load's?* The root is
closed after the load, so a handle either names a root node or never will;
`TypeChain::in_root` (`lattice/src/types/chain.rs`) answers it with a root
probe, which `src/values/settled.rs` asks at six sites per value door, and the
adoption of a root handle at a caching body's exit
(`TypeChain::adopt_digest`, `lattice/src/types/transfer.rs`) resolves it with a
chain lookup that always hits. A measurement over the caching suite's
programs (`scratch/eviction-and-handle-bits.md`) counts 10–45 such probes per
loop hop, none of which lays a node. Every lookup of a branch handle also
probes the root first and misses there. *Is this type concrete?*
`TypeChain::concrete` reads a flag the intern stored, one lookup per call, at
some seventy run-time sites, fifteen of them in `src/dispatch/statics.rs`.
Four koan helpers lift that conversion over a declared type, keeping a scheme
as it is: `close_declared` (`src/elaborate/channel.rs`), `concrete_declared`
(`src/knot/function.rs`), `concrete` (`src/knot/module/coerce.rs`) and `close`
(`src/dispatch/statics/paths.rs`).

**Acceptance criteria.**

- A handle's top two bits are flags, **in-root** and **concrete**, and the
  digest is the remaining 126 bits. Both are masked out of `Handle`'s
  equality, hash and order, out of the verdict key's fold, and out of every
  child digest the recipe in `lattice/src/types/digest.rs` feeds, so a type's
  identity is its content alone; the property suite (`tests/properties.rs`)
  states this and passes.
- Every door that mints a handle sets both bits from the node it interned or
  found: in-root where the probe hit the root or the root builder, concrete
  from the node's flag. The builtin constants in `lattice/src/types/handle.rs`
  carry both bits baked in.
- `TypeChain::in_root` and `TypeChain::concrete` read the bit and look
  nothing up; adopting a root handle is a bit test; a lookup of a handle whose
  in-root bit is clear skips the root; a verdict lookup over two root handles
  probes the root's table alone.
- `TypeChain::concrete_declared` answers a declared type as a concrete one, a
  scheme as it is, and `None` where a variable is reachable; it replaces the
  four koan helpers, and each caller names the invariant behind its answer.
- The caching suite's per-hop adoption probes, counted under the `counters`
  feature, fall to the handles that are not the root's.

**Directions.**

- *Bit placement* — decided. The high word's top two bits: a branch table
  slots on the digest's low word (`DigestTable::word`), which stays whole.
- *Where the two questions are asked* — decided. On the chain, as
  `TypeChain::in_root` and `TypeChain::concrete`, not on the handle: the
  chain hides how it answers, and `concrete` stays the one checked conversion
  a reviewer can search for. On the handle, nine koan functions would drop a
  chain parameter, at the cost of rewriting every call site.

## Dependencies

A change to the lattice (`lattice/src/types`), approved in the design session
that wrote this item.

**Requires:** none.

**Unblocks:** none.
