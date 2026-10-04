# Recursion over run-time types

**Problem.** A value's carried type nests as deep as the value wherever each
level is a structural type: a list of lists a thousand deep carries a list type
a thousand deep. Solved type parameters and run-time type expressions carry
such types further, into function types, unions and meets. Every structural
walk in the [type lattice](../../lattice/src/types/README.md) recurses once per
level of the types it reads:

- the unary drivers [`visit` and `rebuild`](../../lattice/src/types/walk/unary.rs);
- the binary driver [`lockstep`](../../lattice/src/types/walk/binary.rs), and
  the three instances over it. The order re-enters
  [`is_subtype_of`](../../lattice/src/types/order.rs) per nested pair to
  memoize it, the meet re-enters the order through `join` at a contravariant
  position, and each instance's set-wise rule recurses through a callback. The
  unifier's set-wise rule also undoes its partial solution before trying the
  next union member;
- the leaf doors — the signature relation, and the instantiation that relates
  quantified shapes and function types — which run whole relations from inside
  a leaf;
- [`display_name`](../../lattice/src/types/render.rs), the one walk written by
  hand.

With the release binary and a 16 000-level list, each of these koan programs
overflows the stack:

- `==` between two such lists with different leaves, which walks the order;
- a list of two of them, which joins their types;
- `Left | Right` and `Left & Right` over solved type parameters;
- a run-time `:(FN FOR ALL #[Tee] :{y :Tee, z :Elt} -> Tee)` naming a solved
  deep `Elt`, which runs the quantifier census and the substitution.

At 64 000 levels, printing the solved type overflows too. In a debug build on
a test thread, the order, the join and the meet overflow at about 300 levels,
and `rebuild`, a quantified function type's interning and `display_name` at
about a thousand.

**Acceptance criteria.**

- Tests on the default test thread build two list types a hundred thousand
  deep with different leaves, and relate them with `is_subtype_of`, join them,
  meet them, render them with `display_name`, substitute a variable into a
  type holding one, and intern a quantified function type over one, each
  without overflowing.
- `visit`, `rebuild`, `lockstep` and `display_name` run over explicit stacks,
  and so do the order's per-pair memo, the meet's joins and every instance's
  set-wise rule, so the stack a relation uses does not grow with the depth of
  the types it relates.
- A relation reached through a leaf door grows the stack by no more than a
  constant per quantified callable or signature nested inside another, or it
  runs over the same explicit stack as the relation that reached it.

**Directions.**

- *An explicit stack per driver — open.* The drivers own every descent, so
  converting them converts every walk written as an instance. Recommended: frames
  hold a pair's pairing and its children's verdicts, and an instance's set-wise
  rule becomes a step machine the driver feeds verdicts to, with the unifier's
  trial undoing its trail between members.
- *How the leaf doors join the stack — open.* A signature relation or a
  quantified instantiation can become a frame kind of its own, or keep running
  a fresh walk and be bounded by the nesting of quantified callables and
  signatures, which a program deepens only by building a new such type per
  level.
- *A depth cap on types — decided against.* Widening a type past a fixed depth
  would bound every walk, but first-class types are data a program built, and a
  contravariant parameter has no sound widening.

## Dependencies

**Requires:**

- [`type_lattice`](../../lattice/src/types/README.md) — shipped: the walks this
  item turns into explicit stacks.

**Unblocks:** none — a leaf.
