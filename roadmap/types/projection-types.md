# Projection types

A type naming a piece of what a variable is bound to — a field, an element, a
key or value — read at load, and a callable's head reading its own
parameters' types.

**Problem.** A read out of a value typed by a lexical variable — `x.f` over
`x :Elt`, `(xs 0)` or `FIRST xs` over `xs :Ll` — has a type that is a piece of
the bound type, and nothing in the lattice names that piece before the
binding exists. `ATTR` types `x.f` through the variable's declared bound
(`upper_field` in [`rules`](../../src/dispatch/rules.rs)), so the read's unit
is open over the variable: the door hands back a type, so the unit holds and a
resumption re-runs it under each binding
([resumption](../../src/build/README.md#resumption)). A read head typed by a
variable is no container at either end, so `(xs 0)` is at most `Any` and
`(xs "a")`, which the bound refuses, loads
([`lookups`](../../src/dispatch/walk/lookups.rs)). A generic solve over such an
argument reads the variable through its bound
([the unifier](../../lattice/src/types/unify.rs)), which no run reproduces.

A signature cannot name a piece of its own parameter either. A callable's
parameter and return types are elaborated before any argument exists, and
read no parameter: under
`SIG Counter FOR ALL #{Carrier: Number} = #[(VAL zero :Carrier)]`,
`EXPR #(ECHO elem :Counter) -> elem.Carrier = #(elem.zero)` refuses the load
with "`elem` names no binding visible here". Nothing names the type a value
carries in type position: `TYPE OF` has no shape.

**Acceptance criteria.**

- The lattice has a node for a projection of a lexical variable at a step — a
  record field, a list element, a dict key or a dict value — chained for a
  deeper read. It exists only where the variable's bound guarantees the
  piece; it reduces to the piece where its base is concrete, to the union of
  the pieces over a union, and to a point where the pieces of its base's two
  ends meet. *Fits* and `bound_above` read an unreduced projection through
  its ends, the pieces of its base's lower end and bound; the order never sees
  one; substitution of the base reduces it. It renders as `(FIELD f OF Elt)`,
  `(ELEMENT OF Ll)`, `(KEY OF Dd)` and `(VALUE OF Dd)`. Every law in the
  lattice's property suite holds over the node, and the generators draw it.
- A solve over an argument typed by a lexical variable, at a declared list,
  dict or record holding a variable, contributes the projection: under
  `EXPR FOR ALL #[Elem] #(FIRST xs :(LIST OF Elem)) -> Elem`, `FIRST xs` over
  `xs :Ll` with `Ll` bounded by `LIST OF Number` solves `Elem` to
  `(ELEMENT OF Ll)`, a solve each run reproduces.
- `x.f` over `x :Elt`, with `Elt` bounded by `:{f :Number}`, is at most
  `(FIELD f OF Elt)` at load, and exact under a binding whose retype makes the
  piece exact; two reads of one field of one variable are one type along the
  chain they were typed in.
- The unit record of such a read holds the projection, and the read adds no
  question that holds, so where the bound rules out `ATTR`'s other readings —
  a module member, a variant — the unit names the variable rather than
  re-running under every binding.
- A read head typed by a lexical variable whose bound is a list, dict or
  string is read as that container: `(xs 0)` over `xs :Ll` is
  `Result {Ok = (ELEMENT OF Ll), Error = Number}`. An argument the bound's
  container refuses — `(xs "a")`, `(xs {x = 1})` — refuses the load, worded as
  over the closed container. A head typed by a signature's head parameter or
  a path is not read through its bound: an opaque carrier stays an atom under
  `Any`. A test covers a record-bounded and a signature-bounded `FOR ALL`, and
  a head the load leaves unknown still routes by name at run.
- A slot's or a return's type reads a type member of any parameter of its
  head: `-> elem.Carrier` is, at each call, the `Carrier` the module passed as
  `elem` fits its declared signature at, and the load reads it through the
  argument's path. The head is quantified over a hidden variable that `elem`'s
  declared type pins.
- `:(TYPE OF <value>)` names a value's type. Over a parameter of its head
  whose declared type is closed, it is a hidden variable bounded by that type
  and solved per call, so `-> :(TYPE OF elem)` returns a value of the
  argument's own type; over one whose declared type is not, it is that
  declared type. Over any other name, or a chain of member reads rooted at
  one, it is that operand's static type where the load knows it exactly, so
  `m :(TYPE OF int_order)` admits a module that fits `int_order`'s
  signature; elsewhere the load refuses it.

**Directions.**

- *Phase order — decided.* The node, the solve and the load's reads first,
  the head read before the field read; the signature spellings after, once
  their prerequisite ships.
- *Type members — decided.* No projection step: a module's own type need not
  name the member (`MODULE ints = ((LET zero = 0))` carries
  `SIG (zero: Number)`), which comes from its fit against a declared
  signature, so a member read stays a [path](../../src/dispatch/README.md#paths).
- *Which parameters a type may read — decided.* Any parameter of its head;
  the priority classes say which slot solves the hidden variable, and a `FN`'s
  parameter record solves it jointly.
- *A hidden variable's name — decided.* Render-only and never resolved: the
  member's label (`Carrier`) or the parameter's name capitalized (`Elem`),
  with the smallest free digit suffix on a clash (`Carrier2`).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Every typing rule states its question](../one-build/typing-rules-state-their-questions.md)
  — a read out of a variable's value named as a type.
