# A solve reports what it could not establish

A solve that is not reproducible handed back with its reason, so the load
keeps what it did decide.

**Problem.** A static solve over lexical variables reports one flag, whether
a run reproduces it
([solving](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind)),
though the collector tells its reasons apart: an admission that read a
variable through an end or chose a declared union's member, a variable
answered by a meet over a contribution holding one, and a failure at a verdict
over one (`Collector::reproducible` in
[unify.rs](../../lattice/src/types/unify.rs)). A solve that is not
reproducible is stored whole at its bind site
([where a solve is decided](../../design/binding-graph.md#where-a-solve-is-decided)):
the load asks only whether it solves, each variable is a derived name no rule
sees into, and every frame runs the closed solve. Under

```koan
EXPR FOR ALL #{Elt: Number} #(ONLY x :Elt) -> Elt = #(x)
EXPR FOR ALL #[Outer] #(WRAP a :Outer) -> Any = #(
  ONLY a
)
```

the collector joins `Elt`'s contributions to `Outer`, and fails only at the
bound: whether `Outer` fits `Number` turns on the binding. The load keeps
neither fact. `Elt` is a derived name between `Never` and `Number`, and a
call of `WRAP` at `Number` solves `Elt` again to learn that it is `Outer`.

**Acceptance criteria.**

- A solve record that is not reproducible names why, in types written over
  names: each ceiling its join could not be shown to fit, each variable
  answered by a meet over a contribution holding a name beside that meet's
  operands, and whether an admission read through an end or chose a declared
  union's member.
- A solve disqualified only by such ceilings asks one `Fits` question per
  ceiling at its bind site, and on yes its solution is the join, written over
  names. In the program above `ONLY a` asks whether `Outer` fits `Number` and
  types `Elt` as `Outer`, and a test counts the solves a call of `WRAP` at
  `Number` runs for it and counts none.
- A variable answered by a meet over a name is a derived name whose term is
  the meet of its operands.
- A law in the [property suite](../../lattice/src/types/tests/properties.rs)
  states that where every reported ceiling holds at a binding, the reported
  solution with the binding written in is the solve under that binding.
- A count, over the example programs, of the solves stored whole, by reason,
  taken before the change and after.

**Directions.**

- *A verdict that fails inside a declared union — open.* There a failure
  tries the next member, so it is no condition on one solution. Such a solve
  stays stored whole; or the solve record holds one solution per member that
  could admit, each under its own conditions. Recommended: stored whole.
- *A piece read through a bound — open.* A function's return and parameter
  and a signature's type member have no
  [projection](../../lattice/src/types/vocabulary.md#the-node-vocabulary)
  step. A covariant piece as a new step, a change to the lattice's node
  vocabulary; the carried variable unfolded over derived names for its pieces
  before the solve; or stored whole. A parameter is contravariant, and every
  projection step is covariant.
- *A meet with one operand under the others — open.* Where one operand fits
  under every other at every binding, the meet is that operand, yet
  `reproducible` counts any upper contribution holding a variable against the
  solve. Whether the flag is tightened there, and whether in this item.
- *A failure the ends rule out — open.* A join that cannot fit a ceiling
  whatever is bound fails at every binding, so the load can answer its
  question for every binding and refuse there. The solve record reports it;
  or the question's own rigid answer finds it.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
