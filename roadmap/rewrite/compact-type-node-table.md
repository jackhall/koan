# A compact type node table

**Problem.** A [`TypeNode`](../../src/type_lattice/node.rs) is 128 B, sized by
its largest variants: `AbstractType` and `Signature` at 120 B (a `SigSchema` is
104 B), `SetMember` at 113 B and `ExpressionShape` at 96 B. The variants a
program mints most often (`List`, `Dict`, `Record`, `KFunction`, `Union`) need
48 B or less. The [registry](../../src/type_lattice/registry.rs)'s node table
stores an `Entry` beside each key: the node plus its `quantified` and `rigid`
flags, which pad it to 144 B, so each bucket is 160 B. The table is a hashbrown map
in [program storage](../../src/program/README.md) that doubles when full. The
bump can't take the old table back, so after each doubling the previous bucket
array sits dead until the program ends. A registry of 16k mixed types measures
11.8 MB: about 5.3 MB of live node table, 5.3 MB of dead tables, 1.2 MB of
interned content and 49 KB of verdict table. Nothing sizes the table ahead of the
program it serves. An opaque ascription's nonce is a `ScopeId` from
[`ScopeId::next`](../../src/memory/scope_id.rs), which draws a process-wide
random session and a global counter. `Option<ScopeId>` is 24 B, and a minted
type's digest differs from one process to the next.

**Acceptance criteria.**

- `size_of::<TypeNode>()` is 48, pinned by a `const` assert beside the enum.
- `AbstractType`, `Signature` and `SetMember` each keep the fields their hot
  paths read inline and carry the rest behind one `&'run` reference into the
  registry's region, laid on an intern miss. `TypeNode` stays `Copy` and free
  of drop glue, and the digest of every type without a nonce is unchanged.
- The registry mints nonces from a counter it owns, `Option<Nonce>` is 4 B, and
  nothing in the type lattice or `knot` mints a nonce through `ScopeId`. A test
  pins that one program interned twice, into two registries, yields the same
  digests.
- The `quantified` and `rigid` flags are a one-byte `Flags` field in each
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

- *The layouts of `AbstractType`, `Signature` and `SetMember` — decided.*
  Each has room for a `u16` at offset 2, a `u32` at offset 4, one thin
  reference at offset 8 and two 16 B fields:
  - `Signature { flags, schema: &'run SigSchema, schema_digest }`.
    `sig_satisfies` reads the schema's tables together and its verdict is
    cached, so the hop is paid once per pair.
  - `AbstractType { flags, arity: u16, nonce: Option<Nonce>, data, bound, name }`,
    where `data` holds `source` and `param_names`. `bound` (read by the order),
    the arity (read by kind checks) and the nonce (read by admission) stay
    inline.
  - `SetMember { flags, kind, index: u32, data, scc_digest, name }`, where
    `data` holds `scc_size` and the `NodeSchema`. `kind_of` and sibling
    resolution read `kind`, `index` and `scc_digest` without the hop.

  These hot fields are chosen from what each field is for. A field that the
  order, window or dispatch paths turn out to read per dispatch moves inline
  instead.
- *The nonce — decided.* `Nonce(NonZeroU32)` from a checked `Cell<u32>` counter
  on the registry, refusing rather than wrapping on overflow. The counter lives
  on the registry, so no process-wide state feeds a type's identity. Nonces are
  unique only within one registry, which is the scope a type's identity has.
- *The `source` of a minted type — open.* `mint` in
  [`knot/module/view.rs`](../../src/knot/module/view.rs) passes its nonce as
  `source` too. With a `Nonce`, a minted type needs another `source`. The
  substitution path keys on types without a nonce, so a nonced type's `source`
  is never read to resolve anything. Recommended: `ScopeId::SENTINEL`, once
  that is confirmed against `substitute.rs`.
- *Packing the flags — decided.* A safe `flags: Flags` field per non-leaf
  variant: rustc places a variant's one-byte field in the tag's padding at
  offset 1, and the size assert turns any layout drift into a build error.
  A hand-packed header over a `union` payload reaches the same 48 B and adds
  `unsafe`.
- *How `ExpressionShape` fits 48 B — open.* Only `elements`, `ret` and the
  quantifier arity carry identity: `quantifiers` is render-only, and `bounds`
  is read off the `Quantified` occurrences. Options: move the whole payload
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

**Requires:**

- [Dispatch](dispatch.md) — calibrating the presize needs running programs.

**Unblocks:** none — a leaf.
