# A carried union at a contravariant position

A solve over a union an argument carries where the order turns over, answered
by a rule blind to the union's member order.

**Problem.** A declared union's members are tried most specific first, and a
tie is refused where it is declared
([solving](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind)),
but a union the *argument* carries at a contravariant position is tried member
by member in the order the union stores them, and the first member that admits
keeps its contributions (`under_carried` in
[unify.rs](../../lattice/src/types/unify.rs)). Under
`EXPR FOR ALL #[Elt] #(KAA f :(FN :{x :(LIST OF Elt)} -> Null)) -> Any`, a
function over `(LIST OF Number) | (LIST OF Str)` solves `Elt` to `Number`, and
one over `(LIST OF Bool) | (LIST OF Str)` to `Bool`. Each answer is one the call
admits, but which one a program gets turns on handle order, by keyword and by
name alike — so on how the program is spelled: with two `NEWTYPE`s `Apple` and
`Pear`, a function over `(LIST OF Apple) | (LIST OF Pear)` solves `Elt` to
`Apple`, and the same union written `(LIST OF Pear) | (LIST OF Apple)` to
`Pear`.

**Acceptance criteria.**

- `KAA` over a function on `(LIST OF Apple) | (LIST OF Pear)` solves `Elt` to
  one type whichever order the union is written in, by keyword and by name.
- A law in the [property suite](../../lattice/src/types/tests/properties.rs)
  states that a solve's answer does not depend on the order a carried union's
  members were interned in.

**Directions.**

- *What a carried union at a contravariant position contributes — open.* The
  meet of every admitting member's contributions, so `Elt` is bounded by each
  and solves to `Never` above, as upper contributions from two functions
  already meet; a refusal at the call where two members admit with different
  contributions, as a declared union's tie is refused; or a solve keeping one
  interval per admitting member and binding where some member's interval holds
  the other slots' contributions. The meet is order-free and sound but refuses
  a call another slot pins above `Never`. Recommended: the meet. Each is a
  change to the type lattice, a flagged exception needing an OK.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
