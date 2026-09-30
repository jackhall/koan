# Solving to the least instance

A type-parameter solve that bounds each variable below by the join of what
reaches it from below and above by the meet of what reaches it from above.

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
- The order's clause for a shape pins each later class to the solution an
  earlier class found over the candidate's slot types, though a call fixes it
  from the carried types under them. In written order,
  `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str` lies under
  `#(PAIR x :(Number | Str) WITH y :(Number | Str)) -> Str`, which admits
  `PAIR 1 WITH "s"` where the first refuses it, so a module offering the first
  satisfies a signature declaring the second. The first also lies under
  `#(PAIR x :Number WITH y :Str) -> Str` through the second, but not directly.

**Acceptance criteria.**

- A solve bounds each variable by a pair: below by the join of its lower
  contributions, above by the meet of its upper contributions and its declared
  bound. It fails only where the set the pair denotes is empty — where the lower
  end lies above an upper contribution or above the bound.
- A contribution set with a maximum gives that maximum as the lower end, and one
  with a minimum under the bound gives that minimum as the upper end.
- A call binds each variable to its pair's least instance: the lower end where a
  lower contribution reached the variable, and the upper end otherwise.
- `FLAT [["a"], [1]]` runs with `Elt` bound to `Number | Str`.
- `FIRST` over the list above runs and has the type `FN :{x :Never} -> Null`,
  and over a `FN :{x :(Number | Str)} -> Null` and a
  `FN :{x :(Number | Bool)} -> Null` the type `FN :{x :Number} -> Null`.
- `FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> Null` lies under
  `FN :{x :Number, y :Str} -> Null`, and a function of the first type passes to
  a slot of the second.
- Priority classes pin at a call as they do: under
  `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, the first class fixes
  `Elt` to `Number` from `1`, and `PAIR 1 WITH "x"` refuses the load.
- In written order, `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str` does
  not lie under `#(PAIR x :(Number | Str) WITH y :(Number | Str)) -> Str`, and
  `EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Null) TO y :Elt) -> Null`
  lies under `#(APPLY f :(FN :{x :Number} -> Null) TO y :Number) -> Null`.
- A lexical variable has a lower end beside its upper one, and a `FOR ALL`
  name's lower end is `Never`. Below a lexical variable lie itself and whatever
  lies under its lower end; above it lie itself and whatever lies above its
  upper end.
- The laws hold by handle over concrete types and up to equivalence over types
  holding a quantified binder. Antisymmetry and the join and meet laws stated by
  handle draw concrete types; transitivity and the join and meet laws stated up
  to equivalence draw binders too.
- The order's transitivity, the interval law and the verdict law hold over
  generators that put carried unions under quantified positions.

**Directions.**

- *A least instance, not a maximum — decided.* A solve that needs a maximum
  refuses what an instance admits, which puts the instantiation clause out of
  step with substitution. A same-type check across slots stays where priority
  classes put it: a later class admits against what an earlier one fixed.
- *Both variances — decided.* A contravariant position takes the dual, as the
  interval's ends are read. Joining alone would leave the order intransitive at
  every contravariant position — a function-typed slot, a return. A meet may be
  `Never`: a function no argument may be passed to.
- *A variable is a pair while it is solved — decided.* A variable can slide
  between its ends while it is solved, so a solve picks no point: the order's
  instantiation clause asks only that each pair denote some type. A point is
  taken where something needs one — a call's binding at run, which its carried
  types determine — and a call's return at load is read through its intervals.
  A bounded variable lives only inside the solve.
- *What a later class admits against — decided.* At a call, the least instance
  of the pair the earlier class solved. Where the arguments are static types —
  judging a use, the order's instantiation clause, ranking — each stands for
  every type a call carries under it, so an earlier class fixes no point but an
  interval: `[Never, L]` where a covariant position names the variable, and
  `[U, bound]` where only a contravariant one does. A later class reads the
  variable as a lexical variable between those ends.
- *Concrete types and binders — decided.* A type is concrete when it holds no
  quantified binder and no free variable. A binder is abstract while it is
  solved and concrete once a solve picks its instance, at load or at run. The
  laws hold by handle over concrete types and up to equivalence over binders:
  `∀Elt :{x :Elt, y :Elt} -> Elt` and `∀A B :{x :A, y :B} -> A | B` lie under
  each other as two handles.
- *Static solutions ship separately — decided.* [Calls solved from their static
  types](static-solutions.md) is its own item. Until it ships, a call solves
  from its carried types, which may bind a variable below the static solve, and
  the load reads a call through each variable's interval.
- *A signature's head — decided.* Ascription solves a head quantifier with a
  call's unifier ([modules](../rewrite/modules.md)), so it takes the least
  instance too: a module whose members contribute `Number` and `Str` to a
  covariant `Carrier` satisfies the signature at `Number | Str`.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
