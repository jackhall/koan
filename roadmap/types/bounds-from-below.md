# A `FOR ALL` name bounded from below

A bound a `FOR ALL` name lies above, so a body may return a value it builds at
the name.

**Problem.** A `FOR ALL` bound caps its name from above alone
([bounds](../../src/elaborate/README.md#what-a-type-expression-is)). A body
that builds a value and returns it at the name needs the name to lie *above*
what it builds, which no bound states:

```koan
EXPR FOR ALL #{Elt: :{size :Number}} #(FRESH x :Elt) -> Elt = #({size = 1})
PRINT (FRESH {size = 5})
PRINT (FRESH {size = 5, width = 2})
```

The body returns a `:{size :Number}`, which lies under `Elt` only where `Elt`
is exactly that type. Under
[a check over a `FOR ALL` name](checks-at-every-binding.md) `FRESH` is refused
at load, and is written without the `FOR ALL`, returning `:{size :Number}`.
In a body the name already reads as a lexical variable between two ends
([the node vocabulary](../../lattice/src/types/vocabulary.md#the-node-vocabulary));
a `FOR ALL` name's lower end is always `Never`, and a quantified variable
carries a bound alone.

**Acceptance criteria.**

- A `FOR ALL` group states a bound its name lies above. A call binds the name
  to a type between its two bounds, and a call whose solution lies outside
  them is not admitted.
- `FRESH` above, declaring `Elt` above `:{size :Number}` as well, loads, and
  `FRESH {size = 5}` prints `{size = 1}`.
- A body reads the name between its two bounds: a value of the lower bound's
  type satisfies the name at load.
- A lower bound is closed, names no type variable and lies under the upper
  bound; any other is refused where it is declared.

**Directions.**

- *How a lower bound is written — open.* A group's dict maps each name to one
  bound quote today. The spelling is the designer's to rule, and none is
  proposed here.
- *What carries the lower bound to a call — open.* (1) A quantified variable
  gains a lower end beside its bound, as a lexical variable has, and the
  solve joins it into the variable's lower contributions. (2) The declaration
  keeps the lower bound beside its scheme, and admission checks the solution
  against it. Either changes the
  [type lattice](../../lattice/src/types/README.md) or what a scheme's
  identity is.

## Dependencies

**Requires:**

- [A check over a `FOR ALL` name holds at every binding](checks-at-every-binding.md)
  — the refusal a lower bound lifts.

**Unblocks:** none — a leaf.
