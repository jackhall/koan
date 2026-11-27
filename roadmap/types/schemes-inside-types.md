# A scheme inside a type over an encloser's names

**Problem.** A scheme inside a lattice node has closed bounds, so a `FOR ALL`
bound inside a type that names an encloser's name — a `SIG`'s `VAL` scheme, or
a `SIG`'s own group — is refused at load
([a bound over an encloser's name](../../design/binding-graph.md#what-a-bind-site-binds)).
Only a callable's own group may name one, through the recipe its static type
carries. Under

```koan
EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
  SIG Local = #[(VAL pick :(FN FOR ALL #{Item: Elt} :{y :Item} -> Item))]
  MODULE m OVER #[Elt] = (
    LET pick = (FN FOR ALL #{Item: Elt} :{y :Item} -> Item = #(y)))
  LET v = (m :| Local)
  (v.pick) {y = x}
)
PRINT (SUBJECT 5)
PRINT (SUBJECT "a")
```

the load refuses `Local`'s `VAL` at `Item: Elt`: "a `FOR ALL` bound inside a
type names a type bound where the program runs, so each binding would make it
a different type". So does `SIG Box FOR ALL #{Carrier: Elt} = #[(VAL zero
:Carrier)]` inside `SUBJECT`'s body.

**Acceptance criteria.**

- In the program above, `Local` loads, its `pick` declared at a scheme whose
  bound is `Elt` where `Elt` is bound: `(SUBJECT 5)` prints `5` and
  `(SUBJECT "a")` prints `a`.
- A `SIG`'s own group bounded over an encloser's name,
  `SIG Box FOR ALL #{Carrier: Elt} = #[(VAL zero :Carrier)]`, loads alike.

**Directions.**

- *How a type holds a scheme over names — open.* (1) A rigid variable in the
  lattice that stands for a scheme, its term the scheme's recipe, with a rule
  for a call through it. (2) Bounds written over names in the lattice, so a
  scheme inside a node may name a run-bound name. Both change the lattice's
  vocabulary, under [its laws](../../lattice/src/types/laws.md), and need the
  lattice owner's approval.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
