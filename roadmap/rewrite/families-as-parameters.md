# Families as parameters

A head parameter or a `FOR ALL` name ranging over a type constructor, and a
constructor application that leaves a parameter open.

**Problem.** A family — `NEWTYPE (Type AS Wrap)`, or a parameterized `UNION` —
is applied only at a declared referent: the
[elaborator](../../src/elaborate/README.md#what-a-type-expression-is) refuses an
application whose head names a `FOR ALL` variable or a signature's head
parameter, and one whose arguments are not exactly the parameters the
constructor declares ([refusals](../../src/elaborate/README.md#refusals)). So no
signature quantifies over a family — a `Monad` over `Wrap`, which
[monadic side effects](../old_foundation/monadic-side-effects.md) records the
effect system needs, has no spelling for `Wrap` in its head group — and a
two-parameter family never stands where a one-parameter one is asked, since
`Result` cannot pin `Error` and leave `Ok` open. A `FOR ALL` group's dict
spelling, `#{Elt: Number}`, writes a bound per name and nothing else
([quantified types](../../design/quantified-types.md)).

**Acceptance criteria.**

- A quantifier group declares a family variable as a declarator declares a
  family: `SIG Monad FOR ALL #[(Type AS Wrap)] = #[…]` and
  `EXPR FOR ALL #[Elt (Type AS Wrap)] …` declare `Wrap` over one parameter, and
  `(Key Val AS Wrap)` over two, named.
- An application headed by a family variable elaborates: `:(Elt AS Wrap)` as the
  one-parameter shorthand, and `:(Wrap {Key = Elt, Val = Str})` pairing by the
  names the variable declares, which fall positionally onto the solved family's
  parameters in its declared order.
- A family variable is solved through a manifest member or a pin alone: a module
  binding `LET Wrap = Option` fits `Monad` with `Wrap` solved to `Option` by
  handle, and every other member is checked under that solution. A member's
  type contributes nothing to a family variable's solve.
- Substituting a family for a variable head rebuilds the application through
  the door a written application goes through, so `:(Elt AS Wrap)` under
  `Wrap = Option` is the one handle `:(Elt AS Option)` elaborates to, a
  parameterized union's variants applied at once.
- A `FOR ALL` family variable on a callable is solved where a slot pins it
  through a signature application, `#(LIFT m :(Monad WITH {Wrap = Wrap}) …)`,
  and a group whose family variable no slot pins is refused as a variable
  nothing fixes is.
- A functor whose slot is `m :(Monad WITH {Wrap = Context})` runs over a module
  defining `RETURN` and `BIND` at every `Item`, its body using `BIND` at two
  instances through that one slot.
- A constructor application may leave a parameter open, and the result is a
  family over the parameters left open: `Result` pinned at `Error = Str` is a
  one-parameter family over `Ok`, and stands where `(Type AS Wrap)` is asked.
- An application headed by a family variable is covariant in its arguments, as
  every family is in its parameters.

**Directions.**

- *Solving by manifest member alone — decided.* A family variable is never
  solved by matching an application's head structurally: an application of a
  parameterized union elaborates to the union of its variants' applications and
  has no head to match, and higher-order unification is what the restriction
  avoids. The module says which family it is over, as OCaml's
  `type 'a t = 'a option` does (Leroy, *Manifest types, modules, and separate
  compilation*, 1994).
- *The dict spelling — open.* A family takes no bound, so whether
  `#{Elt: Number, (Type AS Wrap): Any}` is admitted at the bound `Any` alone, or
  a family variable is written only in the list spelling.
- *The partial application's spelling — open.* `:(Result {Error = Str})`, which
  relaxes the refusal of an application missing a key, or an explicit marker for
  the open parameter.
- *Builtin families — open.* Whether `List` and `Dict` stand as families, so a
  `Monad` is satisfied at `Wrap = List`; `FN` never does, since its parameters
  are contravariant.

## Dependencies

**Requires:**

- [Modules](modules.md) — a signature over a family is exercised only by a view
  that runs.

**Unblocks:**

- [Effects](effects.md) — a `Monad` signature over a family.
