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
  `Sealed` and a resident-table entry carry it inline. A `Resident` carries no
  mask, only the key naming its entry, and is unchanged.
- The harness's keep-and-redeem and fan-out benchmarks record zero
  allocations per `alloc`, `hold`, `keep`, and slab-path `redeem` on a warm
  table, and no benchmark records more time than the row before.
- `CellTable::new` refuses, at compile time or construction, a cap above the
  width, and the crossing signal's `cap` field still reports the construction
  cap — pressure in a two-cell table is against 2, not the width.

**Directions.**

- *How the width is fixed — decided.* A const generic for the word count,
  defaulting to one, on `CellTable` and on every type that carries a row:
  `StepContext`, `Sealed`, `Operand`, and the crate-private `Bits`, `Mask`,
  and matrix. An embedder that needs a deeper slab instantiates a wider
  table without a crate edit, and the crate's own tests and benchmarks stay
  at one word. A crate constant was rejected because it would move the
  crate's tests and benchmarks to whatever width the embedder needs. On
  stable `W * 64` cannot be an array length, so a matrix is stored as `W`
  chunks of 64 rows — byte-identical to a flat array, with the chunking only
  in the index arithmetic. The word count is expected to stay small; how a
  deep slab segments its liveness matrices is separate work.
  Plan: [scratch/compile-time-slab-width-plan.md](../../scratch/compile-time-slab-width-plan.md).
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
