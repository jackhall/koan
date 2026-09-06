# Compile-time slab width

**Problem.** The slab cap is a runtime value, so every row of bits over it is
a heap allocation of `ceil(cap / 64)` words: `Bits::new` boxes a slice
([matrix.rs](../src/matrix.rs)), and `Mask::empty` ([mask.rs](../src/mask.rs))
boxes one for every reach. That puts a heap allocation on the per-value path.
`alloc` builds an empty mask for a reach that names one slot; `cross` builds
one per placement; `hold` builds a one-bit mask only to OR it into a row;
`redeem` clones the stored mask on the slab path and builds a whole row to
carry one sealed id on the record path; `Sealed::clone` clones the row;
`pin_price` copies the destination's pin row for its seen set; every
resident-table entry holds a row of its own; the two matrices and the
executing row are boxed slices sized at construction. At a cap of 4096 each
row is 512 bytes, allocated and freed once per value built, and each matrix is
2 MiB, most of it zero for a slab that is rarely more than a few dozen cells
deep.

**Acceptance criteria.**

- The slab width is a compile-time constant of the table's type: a row of
  bits is an inline word array, the two matrices and the executing row are
  inline arrays over it, and no path in the crate allocates to build, copy,
  or store a slab row.
- The first width shipped is one word — 64 cells — and the crate's tests and
  benchmarks run at it.
- A reach mask is the inline row plus its sealed half, so a mask naming no
  sealed region is built, copied, and compared with no allocation, and
  `Sealed`, `Resident`, and a resident-table entry carry it inline.
- The harness's keep-and-redeem and fan-out benchmarks record zero
  allocations per `alloc`, `hold`, `keep`, and slab-path `redeem` on a warm
  table, and no benchmark records more time than the row before.
- `CellTable::new` refuses, at compile time or construction, a cap above the
  width, and the crossing signal's `cap` field still reports the width.

**Directions.**

- *How the width is fixed — open.* (a) a crate-level constant, one word, with
  the cap a construction-time value at or below it; (b) a const generic on
  `CellTable`, `Bits`, and `Mask` for the word count, defaulting to one, so
  an embedder that needs a deeper slab instantiates a wider table without a
  crate edit and the 64-cell figure is a parameter rather than a ceiling.
  Recommended: (b). A cell is a call frame's worth of execution in the
  embedder's terms ([adopt-cellgraph.md](../../workgraph/roadmap/adopt-cellgraph.md)),
  and the parent chain of a non-tail-recursive call stays live for its whole
  depth, so 64 cells is the width for the crate's own tests and for loops
  that recycle a slot per hop, not for a koan program that recurses past
  that depth; the embedder should be able to widen it in its type.
- *Cap below the width — decided.* The cap stays a construction value at or
  below the width, so a test can build a two-cell table over a 64-bit row
  and admission is still refused at the cap, not at the word boundary.
- *Word type — decided.* `u64`, one word per 64 slots; the row's `place`
  arithmetic is unchanged.

## Dependencies

**Requires:** none — the zero-allocation criteria are read off
[tools/cellgraph_perf.py](../../tools/cellgraph_perf.py) against the record in
[observe/perf.csv](../observe/perf.csv).

**Unblocks:** none — leaf.
