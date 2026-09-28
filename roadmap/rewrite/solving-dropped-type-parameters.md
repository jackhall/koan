# Solving dropped type parameters

**Problem.** Canonical form drops a type variable that occurs once in a
callable's slots or parameters ([`canonical_group`](../../src/type_lattice/registry.rs)),
so the function type and the registered shape carry no position for it, and a
call has nothing to solve it from: the frame binds it to its bound
(`Canonical::Dropped` in [`frame`](../../src/program/body.rs)), and a keyworded
call's argument record carries only the canonical group's solution
([`arguments`](../../src/dispatch/select.rs)). So under
`EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Type = #(Elt)`, `KIND 1` gives `Any`,
not `Number`: a body cannot read the type an argument brought for a variable
it names once.

**Acceptance criteria.**

- A type parameter canonical form drops binds, in the body, to what the call's
  arguments solve it to, whether the callee is called by keyword or by name:
  `KIND 1` under the definition above gives `Number`.
- A callable's function type and registered shape are the handles canonical
  form gives them, so two alpha-variant definitions still share one type.

**Directions.**

- *How the dropped variables are solved — open.* Recommended: a lattice API
  that interns a callable's group with every declared variable kept, for
  solving only; a second handle on the registration beside its registered
  shape; and a second solve per call, only for a callee whose group dropped a
  variable. It is a type-lattice change, and needs the user's approval as one.
- *What a variable occurring only in the return binds — open.* No argument
  solves it: it may bind `Never`, the value canonical form replaces it by at a
  covariant position, or its declared bound, as every dropped variable does
  today.

## Dependencies

**Requires:**

- [Canonical form](../../src/type_lattice/README.md#the-relations) — shipped:
  the renumbering that drops the variable.

**Unblocks:** none — a leaf.
