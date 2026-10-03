# Calls by name judged as keyworded calls

One judge and one contribution record for a call, however it is spelled.

**Problem.** A keyworded use is judged through the lattice's `judge_by_class`,
and its contributions are recorded per slot; a call by name is judged by rules
of its own in `admissible` and `called`
([statics.rs](../../src/dispatch/statics.rs)) — a meet of upper ends,
exactness and reproducibility restated — and its contributions are recorded
sparsely by name, which the evaluator converts back to a positional list for
the frame while `select::solved_from` filters the keyworded list by solving
slot again at the call. The two judges already disagree. A call by name has no
lower-end *never* test: under
`LET which = FN EXPR #(WHICH x :(LIST OF Number)) -> Str`, `WHICH x` over a
parameter `x :(LIST OF Any)` refuses the load while `which {x = x}` loads and
faults at every call. An instance field is wanted at a type read from raw
intervals where the judge reads a contribution as a point (`slot_wanted`'s
`exact &= argument.is_exact()`): `g {x = a, f = pick}` under
`FN FOR ALL #[Elt] :{x :Elt, f :(FN :{v :Elt} -> Elt)} -> Elt` over
`a :(Number | Str)` is refused `Unfixed`, though the frame would solve
`Elt = Number | Str` from `x`'s recorded contribution. A keyworded instance
slot ranked in one class with a contributing argument is refused the same way —
`PAIR a WITH pick` under `EXPR #(PAIR 1 WITH 1)` and
`EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH f :(FN :{x :Elt} -> Elt)) -> Elt` —
where written order, ranking `x` before `f`, fixes `Elt` from `x`'s
contribution through `solved_earlier`. `called` carries dead branches: its
early return makes `scheme` always `Some`, and `exact` equals `contributed`.

**Acceptance criteria.**

- A call by name is judged through `judge_by_class` over its callee's parameter
  record laid out as a one-class expression shape; `admissible` and `called`
  restate no relation of their own.
- A call by name whose argument's lower end does not fit a parameter refuses
  the load (`CallNeverSatisfied`), as a keyworded use is refused.
- The cell records one kind of contribution for both spellings, which the call
  and the frame read without converting.
- A keyworded call and a call by name of one combined definition over one class
  return the same value and carried type, and a property law states it over
  generated programs.
- A call by name returns the same value whatever order its record's fields are
  written in, and a property law states it.
- An instance site in one class with a contributing argument is judged alike
  by keyword and by name: `g {x = a, f = pick}` above and `PAIR a WITH pick`
  under `EXPR #(PAIR 1 WITH 1)` are both accepted or both refused.

**Directions.**

- *Whether an instance site's wanted type reads a contributing argument as a
  point — open.* (a) Yes: `slot_wanted` takes the solve-from intervals the
  judge takes, so `g {x = a, f = pick}` instantiates `pick` at `Number | Str`,
  the type the frame solves from `x`'s contribution, and a keyworded slot
  sharing a class with a contributing argument likewise — sound by the reading
  the judge already uses, since the call solves from that upper end. (b) No:
  the refusal stands, an instance site needing an exact argument or an earlier
  class, and the dispatch README's "each field contributing as a keyworded
  argument does" is narrowed to the solve. Recommended: (a).
- *How a parameter record becomes a one-class shape — open.* The registry's
  `shape_type` over the parameters in symbol order, every slot at class `0`,
  interned per callee; or a judge entry point over a record. Recommended: the
  former, since solving slots, classes and ranking then serve as they are, and
  a lattice-side door would need a law of its own.
- *Which contribution record is kept — open.* The keyworded one, positional
  with `Unknown` holes, or the by-name one. Recommended: positional, so
  `select::solved_from` and the frame read one list.

## Dependencies

**Requires:**

- [One entry per draft, one walk per capture](entry-and-capture-walk.md) — the narrowing law's program generator, which this item's laws draw from.

**Unblocks:** none — a leaf.
