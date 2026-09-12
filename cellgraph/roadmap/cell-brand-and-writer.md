# Cell brand and writer doors

Two doors the first embedder that keeps structural state in a cell needs: a
brand for the executing cell's own region, and a region writer that can lay
down cells that change state.

**Problem.** A step hands out one brand, `'b`. A value built in the executing
cell's own region comes back as a `Ready<'b, _>`, readable only through `read`
at a borrow strictly inside the step, and `alloc`'s build closure quantifies a
fresh `'r`, so a value built here a moment ago cannot be embedded in the next
value built here. The one route is `alloc_into` with the earlier value as an
operand, priced by the crossing verdict for a same-region operand that crosses
nothing. `Writer`'s verbs are bounded `T: Copy` as a stand-in for "no
destructor", which also refuses `Cell<T>`, so a table whose cells move from
empty to claimed to bound across steps cannot be written into a region at all.
Both gaps land on koan's `memory` module: its slot array and the per-call
resident it exists to host
([memory-on-cellgraph.md](../../roadmap/rewrite/memory-on-cellgraph.md)).

**Acceptance criteria.**

- `StepContext` carries a second brand `'cell`, invariant and quantified per
  `enter`, with no outlives relation to `'b`. A reference at `'cell` names
  storage the executing cell's hold set covers for the cell's whole life: its
  own region, or a region a pinned crossing into this cell has minted into its
  holds. Nothing else reaches `'cell`: a foreign carrier's read is at a borrow
  strictly inside the step and cannot coerce. Compile-fail tests cover the two
  escapes: a foreign carrier's read cannot be embedded in a `'cell` build, and
  a `'cell` reference cannot leave `enter`.
- `StepContext::writer` hands out a `Copy` `Writer<'cell>`; an own-region
  write takes no closure. `alloc_here(operands, build)` is the own-cell
  placement: each operand is priced through the verdict, the pinned reach is
  minted into the executing cell's holds, and the build receives a
  `Writer<'cell>` beside views at `'cell`, returning whatever it returns.
  `alloc_into` is unchanged, and `alloc` is gone: an own-region write with
  nothing to cross is the writer.
- The continuation is stored through one door, `store_successor(C::At<'cell>)`,
  and handed back at `'cell`. Its captures may be own-region references, and a
  capture from another cell enters through `alloc_here`'s verdict, so the
  store itself prices nothing and records no reach: `store_successor_capturing`
  and the continuation's reach-table entry are gone.
- One bridge from an own-region value to a carrier, with the executing cell as
  its reach, so such a value can be an operand, kept, or delivered.
- `Writer` has two verbs: `fill(len, FnMut(usize) -> T) -> &'r [T]`, with
  `T: Copy` replaced by the compile-time `needs_drop` check `mint_and_build`
  makes, and `text`. `value` and `slice` are gone; an embedder derives them.
- [tests/surface.rs](../tests/surface.rs) names the new surface and nothing
  more; the Miri slate covers the `'cell` re-anchor;
  `python3 tools/cellgraph_perf.py --gate` is clean.
- [../README.md](../README.md) states the two brands and
  [../src/graph/README.md](../src/graph/README.md)'s staleness argument covers
  a `'cell` reference through the seal transition and every merge.

**Directions.**

- *The brand is a lifetime, not a type — decided.* An own-region value is a
  plain `&'cell T`, carrier-free; the three carrier states are for what is
  homed elsewhere or crosses a step. Invariance comes from the usual
  `PhantomData<fn(&'cell ()) -> &'cell ()>`, quantified by `enter`'s closure.
  The retype is sound by the same argument as `read`, plus one fact:
  own-region chunks never move, since sealing splices bumps and a merge
  absorbs them, so a `'cell` reference into storage that later seals stays
  valid, which is what lets the continuation's captures be re-anchored at the
  next step's `'cell`.
- *Tree cells get `'cell` for their own region only — decided.* Never the
  chain: a reference up the chain is a foreign one and takes the verdict.
- *`fill` lives on `Writer`, not on the context — decided.* `Writer<'r>` has
  no type parameter; a shape whose constructor takes one stays free of the
  continuation family. An `Allocator` impl on `Writer` was rejected: it widens
  the surface with `allocator_api2`, and a growable container's drop glue
  defeats the check.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Memory on cellgraph](../../roadmap/rewrite/memory-on-cellgraph.md) — the
  slot array is written through `fill` at `'cell`.
