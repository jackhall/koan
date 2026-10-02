# Instantiating a quantified function

A quantified function becomes concrete where the type it is wanted at fixes its
variables.

**Problem.** A name bound to a quantified `FN` stands only at the head of a call
([resolution](../../src/scope/README.md#resolution)). Anywhere else it is
refused, even where the type it is wanted at fixes every variable. Inside
`MODULE lib = (…)`, after `LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))`,
the load refuses both `[pick]` and `pick :! :(FN :{x :Str} -> Str)`, though the
type the ascription wants fixes `Elt`. The only way to pass or store `pick` is to
wrap it by hand in an unquantified `FN` that calls it.

A plain `LET` binds a quantified `FN` in any body today, although nothing may
ever solve its group. The load refuses only a body whose last statement binds
one (`ShapeError::QuantifiedValue`, [resolution](../../src/scope/README.md#resolution)),
since that statement's value is the body's. That refusal is a stopgap: it also
refuses a body whose declared return would solve the group, such as
`FN :{} -> :(FN :{x :Number} -> Number) = #(LET g = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))`.

`LET` has one overload, `LET <name> = <value>`. No binding states the type its
value is wanted at.

**Acceptance criteria.**

- `LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))` outside a module body
  refuses the load and names the unsolved variable `Elt`, whether or not `pick`
  is read. The same binding as a `MODULE` body's member loads.
- `LET <name> :<type> = <value>` binds `<value>` at `<type>`.
  `LET inc :(FN :{x :Number} -> Number) = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))`
  loads anywhere. `(inc {x = 1})` is `1`, and `(inc {x = "s"})` refuses the load.
- In a module body that binds `pick`,
  `LET inc :(FN :{x :Number} -> Number) = pick` loads, and so does
  `LET keep :(LIST OF (FN :{x :Number} -> Number)) = [pick]`.
- `pick :! :(FN :{x :Str} -> Str)` is a function at `Elt = Str`.
- A call whose slot is typed `:(FN :{x :Number} -> Number)` takes `pick` as its
  argument, solving `Elt` to `Number`.
- Where nothing fixes a variable, as in `LET keep = [pick]`, the load refuses
  and names the variable.
- A body whose declared return solves the group of the quantified function its
  last statement binds returns that function at the solved type:
  `FN :{} -> :(FN :{x :Number} -> Number) = #(LET g = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))`
  loads, and its call's value is a `:(FN :{x :Number} -> Number)`.
- An instance's value is stamped with the concrete function type its group was
  solved at, and is stored, passed, returned and dispatched on like any
  unquantified function.

**Directions.**

- *Where a quantified `FN` may be bound — decided.* Only as a module member, or
  under a declared concrete type that solves its group. Any other binding
  refuses the load, whether or not its value is read. The body-tail refusal
  (`QuantifiedValue`) is the subset of this rule that ships first.
- *What instantiates — decided.* A solve, an annotation or an ascription: each
  is a type the quantified function is wanted at. Once its group is solved, the
  function's value is stamped with the solved concrete type and used normally:
  returned under a declared return such as `-> :(FN :{x :Number} -> Number)`, or
  ascribed `(… :! T)`.
- *The annotated binding — decided.* This item adds the overload
  `LET <name> :<type> = <value>` beside `LET <name> = <value>`, and an ascription
  `(… :! T)` instantiates as well.
- *A variable nothing fixes — decided.* It is refused and never read as its
  bound, as the
  [least instance](../../src/type_lattice/solving.md#the-unifier-collects-it-does-not-bind)
  rule already requires.
- *Keyworded forms — deferred.* `EXPR FOR ALL …` and `LET id = FN EXPR FOR ALL …`
  stay as they are. Whether a registration's name counts as a module member's
  binding belongs to [modules](../rewrite/modules.md).
- *Where an instance is made — open.* Either at load only, where the wanted
  type is known statically, or also at run time, where the wanted type arrives
  through `Any`.
- *A slot that is itself quantified — open.* Under
  `EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt)`, the call
  `APPLY pick TO 1` solves the callee's group and `pick`'s together. Two
  options: solve the call's group from its other arguments first and then
  instantiate, or solve both groups in one collector.

## Dependencies

**Requires:** none — the typed handles an instance converts between have shipped.

**Unblocks:** none — a leaf.
