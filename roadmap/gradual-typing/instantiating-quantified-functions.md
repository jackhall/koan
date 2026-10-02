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
- A `FN FOR ALL` literal is instantiated where it is written:
  `(FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)) :! :(FN :{x :Str} -> Str)` loads,
  and `ShapeError::QuantifiedLambda` no longer exists.
- Under `EXPR FOR ALL #[Elt Out] #(MAP f :(FN :{x :Elt} -> Out) AT y :Elt) -> Out`,
  `MAP pick AT 1` loads and its static type is `Number`; `MAP pick AT y` over a
  parameter `y :(Number | Str)` refuses the load.
- Inside `EXPR FOR ALL #[Outer] …`, an instance whose solution names `Outer`
  refuses the load.
- After `LET f = (FN :{x :Number} -> Str = #("ran"))`, `(f {x = "s"})` refuses
  the load.

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
  rule already requires. A variable some contribution from the wanted type
  reaches binds its least instance, so `:(FN :{x :Number} -> Any)` fixes `Elt`
  to `Number`.
- *Keyworded forms — deferred.* `EXPR FOR ALL …` and `LET id = FN EXPR FOR ALL …`
  stay as they are. Whether a registration's name counts as a module member's
  binding belongs to [modules](../rewrite/modules.md).
- *Where an instance is made — decided.* Only where the program loads. A
  quantified function read where the load does not know the type it is wanted
  at is refused; nothing is solved at run time.
- *A slot that is itself quantified — decided.* The callee's group is solved
  from its other arguments first. A callee variable the slot shares with them
  must come out as one closed type; one only the slot names is read from above
  through `[Never, bound]`. The function is instantiated at that slot type, and
  the candidate is judged with the instance as an exact argument.
- *A solution naming a run-bound type — decided.* Refused: a solution must be
  closed. Reading a lexical variable at run needs a capture the reading body may
  not hold, the open question
  [calls solved from their static types](static-solutions.md) settles.
- *Quantified literals — decided.* A `FN FOR ALL` written anywhere but a
  `MODULE` member's binding or the head of a call is an instance site like a
  name, born already instantiated.
- *A call by name that can never be admitted — decided.* Refused at load where
  the callee's static type is exact and unquantified, which AC 2 relies on.
- *The instance solve — decided.* A lattice door, `instance_under`, beside
  `admits_function` and sharing its walk, answering the least instance or
  naming each variable no contribution reaches.
- *Where a wanted type reaches — decided.* A function type only, never split
  from a union. A container literal passes one to its elements only when it is
  that container's own type, and a keyworded argument takes one only when it is
  itself a quantified name or literal. Candidates a use keeps must agree on each
  instance.

## Dependencies

**Requires:** none — the typed handles an instance converts between have shipped.

**Unblocks:** none — a leaf.
