# Instantiating a quantified function

A quantified function becomes concrete where the type it is wanted at fixes its
variables.

**Problem.** A name bound to a quantified `FN` stands only at the head of a call
([resolution](../../src/scope/README.md#resolution)). Anywhere else it is
refused, even where the type it is wanted at fixes every variable. After
`LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))`, the load refuses both
`LET inc :(FN :{x :Number} -> Number) = pick` and a list `[pick]` declared
`LIST OF (FN :{x :Number} -> Number)`. The only way to pass or store `pick` is
to wrap it by hand in an unquantified `FN` that calls it.

**Acceptance criteria.**

- `LET inc :(FN :{x :Number} -> Number) = pick` loads. `(inc {x = 1})` is `1`,
  and `(inc {x = "s"})` refuses the load.
- `LET keep :(LIST OF (FN :{x :Number} -> Number)) = [pick]` loads.
- `pick :! :(FN :{x :Str} -> Str)` is a function at `Elt = Str`.
- A call whose slot is typed `:(FN :{x :Number} -> Number)` takes `pick` as its
  argument, solving `Elt` to `Number`.
- Where nothing fixes a variable, as in `LET keep = [pick]`, the load refuses
  and names the variable.
- An instance's type is the concrete function type it was made at, so the
  instance is stored, passed and dispatched on like any unquantified function.

**Directions.**

- *What instantiates — decided.* A solve, an annotation or an ascription: each
  is a type the quantified function is wanted at.
- *A variable nothing fixes — decided.* It is refused and never read as its
  bound, as the
  [least instance](../../src/type_lattice/README.md#the-unifier-collects-it-does-not-bind)
  rule already requires.
- *Where an instance is made — open.* Either at load only, where the wanted
  type is known statically, or also at run time, where the wanted type arrives
  through `Any`.
- *A slot that is itself quantified — open.* Under
  `EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt)`, the call
  `APPLY pick TO 1` solves the callee's group and `pick`'s together. Two
  options: solve the call's group from its other arguments first and then
  instantiate, or solve both groups in one collector.

## Dependencies

**Requires:**

- [Concrete and parametric types](../rewrite/concrete-and-parametric-types.md)
  — an instance is the conversion from a quantified callable's type to a `KType`.

**Unblocks:** none — a leaf.
