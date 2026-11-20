# A `FOR ALL` bound over a run-bound name

A callable whose quantifier's bound names a name the run binds, typed at load.

**Problem.** A callable declared inside a quantified body, whose `FOR ALL`
bound names one of the body's run-bound names, is left to the run: the load
registers no type for it, and the statement that registers it is typed again
under every binding of the outer name
([the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)).
Under

```koan
EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Any = #(
  EXPR FOR ALL #{Tee: Elt} #(INNER y :Tee) -> Tee = #(y)
  INNER x
)
PRINT (OUTER 5)
```

`INNER`'s type is a scheme whose bound is `Elt`. A
[derived name](../../src/build/README.md#derived-names) ranges over closed
types, and binding `Elt` re-bounds `Tee` without closing it, so the scheme
is no derived name. The registration is marked `ForAllBound`, one of the
three marks with no question, and every call of `INNER` in the body waits on
it.

**Acceptance criteria.**

- `INNER` above is rigid at load: its registration and each call of it in
  `OUTER`'s body are rigid facts. The program prints `5`.
- `OUTER` resumed under `Elt := Number` gives the facts a fresh load with
  that binding declared gives, and a second binding that keeps its questions
  types no statement again.
- A call of `INNER` is judged through questions the binding answers, and its
  instance is written over the names its solve reads.
- `ForAllBound` is gone: no `FOR ALL` bound over a run-bound name holds a
  statement.
- The property laws over generated programs reach a nested callable whose
  bound names an outer run-bound name.

**Directions.**

- *How the inner scheme is written over names — open.* (1) A koan-side
  declaration over names: the inner callable's registered shape stored with
  its bounds written over the outer names, closed by the binding's levels as a
  rigid declaration is, its readers asking questions of it. (2) A lattice
  change: a quantifier group's bounds may hold variables, so the scheme is
  written over `Elt` and substitution closes it. Recommended: 1, which leaves
  the lattice's group bounds closed.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Build over the binding graph](../one-build/build-over-the-binding-graph.md)
  — nothing is typed again once no unit holds.
