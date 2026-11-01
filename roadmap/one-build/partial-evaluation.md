# Partial evaluation of the top level

What the run decides from known inputs alone, decided before it starts.

**Problem.** [The build image](build-image.md) holds what the text decides
and never a run cell: a constant top-level binding, a value folded from
literals and a fault no input can avert are all decided at the first step
that reaches them, in every run. A run cell whose inputs are all known — a
literal, a load-named binding, the result of such a cell — could be stepped
once at build time and its value kept, and only a cell that reads an input —
a file, the network, the clock — held for the run. Nothing of a run's value
has a form outside its process, so nothing can keep one.

**Acceptance criteria.**

- A build steps every top-level run cell whose inputs are known and holds
  every cell that reads an external input; the held kinds are enumerated,
  and a test names each.
- A value a build decided is in the image, and a run started from the image
  reads it without stepping its cell; a fault a build reached surfaces before
  the program runs, worded as the run words it.
- A program with no external input runs to its output at build time, and a
  run from its image prints the output and steps nothing.
- Every program's output equals a cold run's.

**Directions.**

- *A run value outside its process — open.* A value laid in a region
  serialized by the image's relocation walk, which widens the walk from build
  storage to values, knots and modules; or a build-decided value re-laid at
  load from a plain-data form, as a composition keeps a type as a term. The
  first keeps one walk; the second keeps run cells out of the image's layout.
- *What counts as an input — open.* A file, the network and the clock are
  inputs; whether an action's result and an `EVAL` of composed code count is
  decided here.

## Dependencies

**Requires:**

- [The build image](build-image.md) — a value a build decided needs an image
  to be kept in.

**Unblocks:** none — a leaf.
