# Invariance around a tail cycle

**Optional.** An optimization nothing needs yet.

**Problem.** The load's [folds](../../src/build/README.md#the-folds) class a sealed body's mint sites by whether
the loop passes each leaf of the site's content tree through unchanged
([where each body interns](../../design/type-tiers.md#where-each-body-interns)),
and reads "unchanged" as a bare parameter at the argument slot on every tail
edge into the body. An argument built over invariant leaves — a set built
from an invariant list, a module rebuilt over an invariant capture — has the
same content from the second round on, so the site would mint one carrier at
the first round and hit it after, yet the rule calls it varying and lays it
cold: a `Fresh` loop re-lays it and recomputes its verdicts each hop, and a
`Shares` loop grows its scratch by it each hop.

**Acceptance criteria.**

- A parameter is invariant where, around every cycle of the tail-call graph
  through its body, composing the edges' argument trees maps its leaf either
  to itself or to a tree that holds no leaf of its own body and whose leaves
  are all invariant; a site is invariant where each of its leaves is. Pinned
  by a test over a two-body loop whose second body rebuilds its module over an
  invariant parameter, and by one where the rebuilt module is over the
  parameter itself, which stays varying.
- A body whose name escapes as a value keeps every parameter varying.
- A test runs a `Fresh` tail loop over such a rebuilt module and holds the
  scratch's node count constant across iterations after the first.

**Directions.**

- *The cycle walk — open.* The tail-call graph the load resolves — calls by
  name and each keyworded use's candidates — is small, but its cycles need not
  be few. One pass per strongly connected component, composing argument trees along each edge to a fixpoint with the
  comparison bounded by the trees the load already unfolds, is the expected
  shape; a cap on a composed tree's depth keeps it linear and errs toward
  varying.

## Dependencies

The site classes it refines live in [the folds](../../src/build/folds.rs),
and their tail edges and the invariance fixpoint in
[the shape's tail facts](../../src/scope/shapes/tails.rs).

**Requires:** none.

**Unblocks:** none.
