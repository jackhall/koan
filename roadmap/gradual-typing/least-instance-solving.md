# Solving to the least instance

A type-parameter solve that joins what reaches a variable from below and meets
what reaches it from above.

**Problem.** [`Collector::solve`](../../src/type_lattice/unify.rs) takes, per
variable, the maximum of its lower contributions, else the minimum of its upper
ones, and fails where neither exists; `set_wise` pours every member of a carried
union into the one pool. The same walk is the order's instantiation clause
(`admits_function` and `admits_shape` in
[`sig_relations.rs`](../../src/type_lattice/sig_relations.rs)) and ranking's
`class_at_least`, so a solve that refuses what an instance admits reaches the
order itself:

- The order is intransitive. `FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> Null` lies
  under `FN :{x :(Number | Str), y :(Number | Str)} -> Null`, which lies under
  `FN :{x :Number, y :Str} -> Null`, but the first does not lie under the third:
  its parameters contribute `Number` and `Str` to `Elt`, which have no maximum.
- Under `EXPR FOR ALL #[Elt] #(FLAT rows :(LIST OF (LIST OF Elt))) -> :(LIST OF Elt) = #(…)`,
  `FLAT [["a"], [1]]` is a no-overload fault, though `Elt = Number | Str` admits
  it: the literal carries `LIST OF (LIST OF Str | LIST OF Number)`, whose members
  contribute `Str` and `Number`. Its static type is exact, so the load's solve
  fails alike.
- Under `EXPR FOR ALL #[Elt] #(FIRST fs :(LIST OF (FN :{x :Elt} -> Null))) -> :(FN :{x :Elt} -> Null) = #(…)`,
  a list holding a `FN :{x :Number} -> Null` and a `FN :{x :Str} -> Null`
  contributes `Number` and `Str` from above, which have no minimum, so `FIRST`
  of it faults.

**Acceptance criteria.**

- A solve takes, per variable, the join of its lower contributions, else the
  meet of its upper ones, else its declared bound, and fails only where the
  lower solution lies outside the bound or above the upper one.
- A contribution set with a maximum solves to that maximum, and one with a
  minimum to that minimum.
- `FLAT [["a"], [1]]` runs with `Elt` bound to `Number | Str`.
- `FIRST` over the list above has the type `FN :{x :Never} -> Null`, and over a
  `FN :{x :(Number | Str)} -> Null` and a `FN :{x :(Number | Bool)} -> Null`
  the type `FN :{x :Number} -> Null`.
- `FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> Null` lies under
  `FN :{x :Number, y :Str} -> Null`.
- Priority classes pin as they do: under
  `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, the first class fixes
  `Elt` to `Number` from `1`, and `PAIR 1 WITH "x"` refuses the load.
- The order's transitivity and the interval law hold over generators that put
  carried unions under quantified positions.

**Directions.**

- *A least instance, not a maximum — decided.* A solve that needs a maximum
  refuses what an instance admits, which puts the instantiation clause out of
  step with substitution. A same-type check across slots stays where priority
  classes put it: a later class admits against the solution an earlier one
  fixed.
- *Both variances — decided.* A contravariant position takes the dual, as the
  interval's ends are read. Joining alone would leave the order intransitive at
  every contravariant position — a function-typed slot, a return. A meet may be
  `Never`: a function no argument may be passed to.
- *A signature's head — decided.* Ascription solves a head quantifier with a
  call's unifier ([modules](../rewrite/modules.md)), so it takes the least instance too: a
  module whose members contribute `Number` and `Str` to a covariant `Carrier`
  satisfies the signature at `Number | Str`.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
