# Recursion over runtime data

**Problem.** The walks that follow a value's structure recurse once per level
of it: [rendering](../../src/values/render.rs), [equality](../../src/values/equality.rs)
and the [crossing](../../src/values/crossing.rs)'s deep copy in `values`, and
likely the type lattice's structural walks
([`visit`, `rebuild`](../../src/type_lattice/walk/unary.rs) and
[`lockstep`](../../src/type_lattice/walk/binary.rs)) over the types such a value
carries. A program builds that structure at run time, without bound, so a deep
enough value overflows the interpreter's stack rather than raising a koan
error: `==` over a chain of 20 000 newtypes over records, each naming the next,
aborts the process with a stack overflow. Which other walkers overflow, and at
what depth, is unprobed. [`memory::strongly_connected_components`](../../src/memory/components.rs)
is the precedent: it recursed per chain link, overflowed at load on about a
thousand chained `LET`s, and now walks an explicit worklist.

**Acceptance criteria.**

- A test builds a chain of newtypes over records far deeper than the stack
  admits a frame per level, and renders it, compares it with `==`, and crosses
  it into another region, each without overflowing.
- Every walk in `values` that follows a value's structure runs over an explicit
  worklist, so the stack it uses does not grow with the value's depth.
- Every lattice walk a run-time value's depth can drive holds the same
  property, or the item records which walk cannot reach that depth and why.

**Directions.**

- *An explicit worklist per walker — decided.* Each walker keeps its own stack
  in the step's scratch, as `memory::strongly_connected_components` does,
  rather than a shared traversal framework.
- *Which lattice walks a value's depth reaches — open.* A value's carried type
  nests as deep as the value when each level is a structural type, but a
  nominal chain's type stays one node deep; probing decides which of `visit`,
  `rebuild` and `lockstep` need the worklist.

## Dependencies

**Requires:**

- [`values`](../../src/values/README.md) — shipped: the walks this item turns
  into worklists.

**Unblocks:** none — a leaf.
