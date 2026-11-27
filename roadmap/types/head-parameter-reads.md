# A head reading its parameters' types

A callable's slot and return types reading a type member of one of its own
parameters, and `TYPE OF` naming a value's type.

**Problem.** A callable's parameter and return types are mentions of the
enclosing shape, elaborated before any argument exists
([scope § Visibility](../../src/scope/README.md#visibility)), so they read no
parameter of their own head. Under
`SIG Counter FOR ALL #{Carrier: Number} = #[(VAL zero :Carrier)]`,
`EXPR #(ECHO elem :Counter) -> elem.Carrier = #(elem.zero)` refuses the load
with "`elem` names no binding visible here" (`ShapeError::Unbound`, raised in
[the shape builder](../../src/scope/shape/build.rs)). Nothing names the type a
value carries in type position: `LET x = 5` then `LET y :(TYPE OF x) = 6`
refuses as "this type expression is not supported", since
[`Reader::composite`](../../src/elaborate/expression.rs) has no `TYPE OF` arm.

**Acceptance criteria.**

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
  signature. Over a module name a bind site binds, such as a module parameter
  of the enclosing body, it is a type written over that name and answered in
  each `BoundInstance` of its bind site: a body of
  `EXPR #(KEEP m :Counter) -> Any` reading `LET n :(TYPE OF m) = m` then
  `n.zero` loads, and `KEEP` of a module whose `zero` is `0` returns `0`.
  Elsewhere the load refuses it.

**Directions.**

- *Which parameters a type may read — decided.* Any parameter of its head;
  the priority classes say which slot solves the hidden variable, and a `FN`'s
  parameter record solves it jointly. An operator's head reads none.
- *A hidden variable's name — decided.* Render-only and never resolved: the
  scheme may render the member's label (`Carrier`) or the parameter's name
  capitalized (`Elem`), with the smallest free digit suffix on a clash
  (`Carrier2`), but every message to the user shows the read as written in
  the head (`elem.Carrier`, `TYPE OF elem`).
- *How a hidden variable's type-channel slot is keyed — decided.* By a
  symbol no written name can equal, minted structurally: a dedicated
  `TypeSymbol` constructor digesting the tagged (parameter, label or
  `TYPE OF`) pair, as `RegistrationSymbol::of` does
  ([`symbols`](../../lattice/src/symbols.rs)), its display text the read's own
  spelling. Two reads of one spelling share one variable, and a body's type
  binder of the rendered name (`LET Carrier = Number`) never collides with it.
- *A hidden variable and `OVER` — decided.* It is never listed under `OVER`:
  a body's `MODULE` reaches it only through the parameter it is read from, so
  listing that parameter covers it, and
  `MODULE inner OVER #[elem] = ((LET z :(elem.Carrier) = elem.zero))` in
  `ECHO`'s body loads. Should a read trip the `OVER` check on a hidden
  variable, the message names that parameter (`OVER #[elem]`). A `Carrier`
  written in the body names the outer `Carrier`.
- *The rendered name of a scheme with a hidden variable — decided.* A
  scheme's names are render-only and alpha-variants intern once, so a
  rendering may show the first-interned spelling rather than the head's own
  `Carrier` or `Carrier2`.
- *`TYPE OF` over a parameter whose declared type names a type a run binds —
  decided.* It is that declared type, as the second criterion says, spelled
  through an alias or not. Inside
  `EXPR FOR ALL #[Outer] #(WRAP x :Outer) -> Any`, a `LET Al = Outer` names
  no binding of its own
  ([which binders are names](../../design/binding-graph.md#which-bind-site-owns-a-question)),
  so `EXPR #(SAME elem :Al) -> :(TYPE OF elem) = #(elem)` returns `Outer` as
  `SAME elem :Outer` does.
- *What a head read is in the body — open.* A hidden `FOR ALL` variable with
  a type-channel slot of its own, keyed and named as the directions above
  say, or the
  [projection](../../lattice/src/types/vocabulary.md#the-node-vocabulary) of
  the parameter's bound name, which a frame writes its binding into. Under the
  first, a read that is no hidden variable still needs a slot: a pinned label,
  a manifest member, or `TYPE OF` over a declared type that is not closed.
  Settled when this item is planned over the binding graph.
- *Whether a head read makes its callable quantified — open.* Scope decides
  by syntax, before any type is elaborated, whether a callable is quantified:
  its name is call-only, a body's last statement naming it is refused as
  `QuantifiedValue`, and a module member's read of it is eager. Whether a
  head read is a hidden variable depends on its parameter's declared type:
  under `LET NumCounter = (Counter WITH {Carrier = Number})`,
  `EXPR #(FIXED elem :NumCounter) -> elem.Carrier = #(elem.zero)` reads the
  pin `Number`. Settled when this item is planned over the binding graph.

## Dependencies

Pins over a callable's own variables, which a hidden variable's desugaring
rests on, have shipped.

**Requires:** none.

**Unblocks:** none — a leaf.
