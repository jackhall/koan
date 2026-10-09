# A compact type node table

**Problem.** A [`TypeNode`](../../lattice/src/types/node.rs) is 112 B, sized by
its largest variants: `Signature` (a `SigSchema` is 88 B) and `SetMember` at
111 B, and `ExpressionShape`, `Carrier` and `Lexical` at 64 B. The variants a
program mints most often (`List`, `Dict`, `Record`, `KFunction`, `Union`) need
48 B or less. The
[registry](../../lattice/src/types/registry.rs)'s node table stores an `Entry`
beside each key: the node plus its four probe flags (`quantified`, `rigid`,
`parametric`, `carrier`), which pad it to 128 B, so each bucket is 144 B. The
table is a hashbrown map in [program storage](../../src/program/README.md) that
doubles when full. The bump can't take the old table back, so after each
doubling the previous bucket array sits dead until the program ends. A
registry of 16k mixed types, measured over a 128 B node, held 11.8 MB: about
5.3 MB of live node table, 5.3 MB of dead tables, 1.2 MB of interned content
and 49 KB of verdict table.

**Acceptance criteria.**

- `size_of::<TypeNode>()` is 48, pinned by a `const` assert beside the enum.
- `Signature`, `SetMember`, `Carrier` and `Lexical` each keep the fields their
  hot paths read inline and carry the rest behind one `&'run` reference into
  the registry's region, laid on an intern miss. An expression shape keeps each
  slot's priority class in that slot's element. `TypeNode` stays `Copy` and
  free of drop glue.
- No variant's digest recipe changes: a test pinning the digest of one node of
  every variant, added before the layout changes, passes after them.
- One program loaded and run twice, into two registries, interns the same set
  of digests, pinned by a test.
- The four probe flags are a one-byte `Flags` field in each variant whose
  flags its children decide, read through one `TypeNode::flags` door that
  answers a constant for every other variant. `Entry` does not exist.
- The node table is a segmented chained table. A slot holds a key and its node
  in 64 B aligned to a cache line, a parallel run of `u32` links chains the
  slots, and the heads double by laying a new head segment and splitting each
  chain. It grows only by laying fixed runs, and asks its storage for nothing
  but a run of `n` values, so a table can be laid in a cell's region
  ([type branches](type-branches.md)).
- A benchmark kept in the repo times build, hit, miss and dependent lookups,
  and totals bytes per entry and dead bytes, at 1k, 16k and 64k entries: for
  the chained table grown from its smallest size and presized, against a
  hashbrown map; and for a two-tier lookup in each probe order
  ([type branches](type-branches.md)). The type lattice's design doc records
  the results, and the load factor and head fingerprints they chose.
- A registry of 16k interned types, measured by a test that totals its bump,
  holds under 5 MB.
- The type lattice adds no `unsafe`.

**Directions.**

- *`Carrier` — decided.* `Carrier { key: &'run ContentKey, name, met }`. After
  interning, only the digest and the view table read the key; a signature's
  fit reads `met`.
- *`Lexical` — decided.* `Lexical { level: u32, name: &'run TypeSymbol, lower,
  bound }`. Rendering, the digest and dispatch's unresolved-level refusal read
  the name; every walk reads the two ends inline. Moving `lower` out instead
  would give a `Never` lower end two spellings.
- *`KFunction` — decided.* `KFunction { flags, params, ret }`, with nothing
  out of line: a quantified callable's group lives in the `Scheme` node over
  it ([the node vocabulary](../../lattice/src/types/vocabulary.md)).
- *`ExpressionShape` — decided.* `ExpressionShape { flags, elements, ret }`.
  A shape element is 32 B with 15 spare bytes, so a slot's priority class rides
  in its element. The shape doors keep their `classes` argument and pack it,
  and the digest still feeds nothing for written order.
- *The layouts of `Signature` and `SetMember` — decided.*
  `Signature { schema: &'run SigSchema, schema_digest }`, and
  `SetMember { kind, index: u32, data, scc_digest, name }`, where `data` holds
  `scc_size` and the `NodeSchema`. `kind_of` and sibling resolution read
  `kind`, `index` and `scc_digest` without the hop; `sig_fits` reads a schema's
  tables together and its verdict is cached, so the hop is paid once per pair.
  Neither carries flags: every walk treats both as leaves.
- *Packing the flags — decided.* A safe `flags: Flags` field in `List`,
  `Dict`, `Record`, `KFunction`, `ExpressionShape`, `Union`,
  `ConstructorApply`, `SignatureApply`, `SignatureMeet` and `Scheme`: rustc
  places a variant's one-byte field in the tag's padding at offset 1, and the
  size assert turns any layout drift into a build error. A flags byte beside
  the link would keep the node free of it, but costs a probe a second cache
  line; a hand-packed header over a `union` payload adds `unsafe`.
- *The table structure — decided.* A segmented chained table, in the root and
  in every [branch](type-branches.md). A segmented table never leaves a byte
  dead: a new slot segment doubles the capacity, and a new head segment
  doubles the heads, each chain split by the next digest bit. A segment is a
  unit a region's bump hands out, so a branch is a small head array and one
  segment released with its region. A link inside the slot would make it 80 B,
  across two cache lines.
- *Sizing the root — decided.* The root starts at the table's smallest size
  and grows. The node count a load reaches is not predictable from its text:
  equal content interns once, the load interns types no position spells, and
  until type branches the run interns into the root too. Growth strands
  nothing and costs one relink per entry over the table's life, which the
  benchmark's grown row records against the presized one.
- *The load factor and head fingerprints — open.* Set by the benchmark. Over a
  144 B entry, 0.75 measured best, and dependent lookups ran about 30% slower
  at 16k than a hashbrown map. Fingerprints help misses only, and with two
  tiers every lookup of a root handle from a cell misses in the tier probed
  first.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Type branches](type-branches.md) — a branch is laid as this table in its
  region.
