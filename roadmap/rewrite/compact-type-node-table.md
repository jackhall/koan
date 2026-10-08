# A compact type node table

**Problem.** A [`TypeNode`](../../lattice/src/types/node.rs) is 112 B, sized by
its largest variants: `Signature` (a `SigSchema` is 88 B), `SetMember` and
`ExpressionShape`. The variants a
program mints most often (`List`, `Dict`, `Record`, `KFunction`, `Union`) need
48 B or less. The [registry](../../lattice/src/types/registry.rs)'s node table
stores an `Entry` beside each key: the node plus its four probe flags
(`quantified`, `rigid`, `parametric`, `carrier`), which pad it to 128 B, so each
bucket is 144 B. The table is a hashbrown map
in [program storage](../../src/program/README.md) that doubles when full. The
bump can't take the old table back, so after each doubling the previous bucket
array sits dead until the program ends. A registry of 16k mixed types, measured
over a 128 B node, held 11.8 MB: about 5.3 MB of live node table, 5.3 MB of dead
tables, 1.2 MB of interned content and 49 KB of verdict table. Nothing sizes the
table ahead of the program it serves.

**Acceptance criteria.**

- `size_of::<TypeNode>()` is 48, pinned by a `const` assert beside the enum.
- `Signature` and `SetMember` each keep the fields their hot paths read inline
  and carry the rest behind one `&'run` reference into the registry's region,
  laid on an intern miss. `TypeNode` stays `Copy` and free of drop glue, and
  the digest of every type is unchanged.
- One program interned twice, into two registries, yields the same digests,
  pinned by a test.
- The four probe flags are a one-byte `Flags` field in each
  non-leaf variant, read through one `TypeNode::flags` door that answers
  `Flags::NONE` for a leaf. `Entry` does not exist, and a node table bucket is
  64 B.
- Parsing a program yields a count of its type-expression positions, and
  `CellSubstrate::load` sizes the node table from that count times a
  multiplier.
- The multiplier is measured: the design doc records the nodes-per-position
  ratio observed over a named set of example programs, and the value chosen
  from it.
- The node table's structure is chosen by a benchmark over the 48 B node that
  compares a presized hashbrown map with a segmented chained table. The
  benchmark is kept in the repo, and the type lattice's design doc records
  its build, hit, miss and dependent-lookup timings, bytes per entry and dead
  bytes at 1k, 16k and 64k entries.
- A registry of 16k interned types, measured by a test that totals its bump,
  holds under 5 MB.
- The type lattice adds no `unsafe`.

**Directions.**

- *`Carrier` — open.* An opaque view's carrier holds a 16 B `ContentKey` the
  [view door](../../src/knot/module/view.rs) computes, beside its 16 B name and
  its 16 B met bound, so the three do not fit 48 B inline with a tag. Options:
  move the key behind one `&'run` reference laid on an intern miss, which costs
  a hop only where a carrier is digested or compared; or the met bound, which
  only a signature's fit reads.
- *The layouts of `Signature` and `SetMember` — decided.*
  Each has room for a `u16` at offset 2, a `u32` at offset 4, one thin
  reference at offset 8 and two 16 B fields:
  - `Signature { flags, schema: &'run SigSchema, schema_digest }`.
    `sig_fits` reads the schema's tables together and its verdict is
    cached, so the hop is paid once per pair.
  - `SetMember { flags, kind, index: u32, data, scc_digest, name }`, where
    `data` holds `scc_size` and the `NodeSchema`. `kind_of` and sibling
    resolution read `kind`, `index` and `scc_digest` without the hop.

  These hot fields are chosen from what each field is for. A field that the
  order, window or dispatch paths turn out to read per dispatch moves inline
  instead.
- *Packing the flags — decided.* A safe `flags: Flags` field per non-leaf
  variant: rustc places a variant's one-byte field in the tag's padding at
  offset 1, and the size assert turns any layout drift into a build error.
  A hand-packed header over a `union` payload reaches the same 48 B and adds
  `unsafe`.
- *How `ExpressionShape` fits 48 B — open.* Only `elements`, `ret`, the
  quantifier arity and `bounds` carry identity: `quantifiers` is render-only. Options: move the whole payload
  out of line, which puts a hop on every dispatch read of `elements` and
  `ret`; or keep `elements` and `ret` inline and move `quantifiers` and
  `bounds` behind one `Option<&'run ShapeBinders>`, which is `None` for a
  monomorphic shape and has its arity as its length. rustc places that
  reference in the tag's slot at offset 8, so the variant is 48 B with no
  `unsafe`. Recommended: the second, since dispatch never takes the hop and only
  substitution and rendering of a generic shape do.
- *The table structure — open.* A presized hashbrown map is fastest in the
  spike, but still resizes when a program outgrows its estimate. A segmented
  chained table never leaves a byte dead: growth adds a segment and relinks
  each chain. With a 144 B entry, its dependent lookups ran about 30% slower
  at 16k. Its settings are the presize, the load factor (0.75 measured best)
  and head fingerprints (which help misses at scale only). Recommended: decide
  once the benchmark has run on the 48 B node.
- *Where the type-expression count is taken — open.* The parser tallies each
  type-expression position as it lowers it and returns the count beside the
  statements, or `load` walks the parsed statements afterwards. Recommended:
  the parser tally, since it visits every position already.

## Dependencies

The multiplier is measured over programs that elaborate their types, which
needs dispatch.

**Requires:** none.

**Unblocks:** none — a leaf.
