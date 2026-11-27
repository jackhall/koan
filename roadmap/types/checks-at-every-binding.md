# A check over a `FOR ALL` name holds at every binding

A body's check that reads a `FOR ALL` name, settled where the program loads or
refused there.

**Problem.** A check in a quantified body — a declared return, an annotation,
an ascription, a call by name's argument, an `EVAL`'s operand and result — is
refused at load only where no binding can pass it. Where some bindings pass
and others do not, the load lays it as a question each binding answers
([static types](../../src/dispatch/README.md#static-types)). So a body that
needs more of its name than its bound declares loads, and fails where a call
binds it:

```koan
EXPR FOR ALL #{Elt: :{size :Any}} #(GROW x :Elt) -> Elt = #(
  LET n :Number = (x.size)
  x)
PRINT (GROW {size = "big", width = 2})
```

```text
error: <input>:2:3: this value is Str, which can never satisfy its annotation Number
  in (GROW {size = "big", width = 2}) at <input>:4:7
```

The bound says `size :Any`, the body needs `size :Number`, and the caller is
refused inside a body it did not write. Under the bound `:{size :Number}` the
same call is refused at its own site. Three more bodies load the same way:

- `EXPR FOR ALL #[Elt] #(BAD x :Elt) -> Number = #(x)` passes only where
  `Elt` is `Number`.
- `EXPR FOR ALL #{Elt: :{size :Number}} #(FRESH x :Elt) -> Elt = #({size = 1})`
  passes only where `Elt` is exactly `:{size :Number}`, which no bound says: a
  bound caps a name from above alone.
- `EXPR FOR ALL #{Elt: :{size :Number}} #(REFRESH x :Elt FROM y :Any) -> Elt = #(y)`
  leaves the check to the value `y` carries, as the same body with no
  `FOR ALL` does, and fails as the program runs.

The load cannot yet tell such a body from one that holds at every binding. A
parameter declared at a `FOR ALL` name, a list built from it and a `MATCH`
over it are typed through
[derived names](../../src/build/README.md#derived-names), which the load
compares with nothing until a binding closes them. The return check of

```koan
EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> :(LIST OF Elt) = #([x])
```

is a question every binding answers yes: `[x]` is typed
`LIST OF (UPPER FILL OF (RETYPED TO Elt) .. Elt)`. At `ae6bd4018`, refusing
every check the load leaves undecided over a `FOR ALL` name refuses `WRAP`,
and fails 74 of the suite's 2,597 tests and one tutorial snippet. Refusing
only where no derived name is involved fails 50 tests and no snippet, 21 of
them property laws whose generated programs hold checks a binding decides.

**Acceptance criteria.**

- A check that reads a callable's `FOR ALL` name loads only where the load
  settles it with the name read through its bound. Otherwise the declaration
  is refused at load, called or not. The checks are a declared return, an
  annotation, an ascription, a call by name's argument, a view's operand, an
  `EVAL`'s operand and result, a body that must be code, a read's argument, a
  splice and a spread.
- `GROW` above is refused at load, and the message asks for the bound
  `:{size :Number}`. Under that bound it loads.
- `BAD`, `FRESH` and `REFRESH` above are refused at load, and so is
  `f {x = 1}` over a parameter `f :(FN :{x :Elt} -> Elt)`.
- An ascription is a check like any other: `y :! Elt` over `y :Any` is
  refused at load.
- Applying a value typed by a `FOR ALL` name, `(x 0)` or `x {a = 1}` over
  `x :Elt`, loads only where `Elt`'s bound is a list, a dict or a function
  type.
- The load reads a derived name from above as a type written over names, so a
  check through one is settled where it holds at every binding. `WRAP` above
  loads. So does

  ```koan
  EXPR FOR ALL #{Elt: :{size :Number}} #(REFRESH x :Elt FROM y :Any) -> Elt = #(
    MATCH y WITH #{Elt: (it), _: (x)})
  PRINT (REFRESH {size = 5} FROM {size = 6})
  PRINT (REFRESH {size = 5} FROM 7)
  ```

  which prints `{size = 6}`, then `{size = 5}`.
- A check that holds at every binding only arm by arm is refused at load:

  ```koan
  EXPR #(DOUBLE n :Number) -> Number = #(n * 2)
  EXPR #(DOUBLE s :Str) -> Str = #(s)
  EXPR FOR ALL #{Elt: :(Number | Str)} #(TWICE x :Elt) -> Elt = #(DOUBLE x)
  ```

- Selection stays a binding's. Under
  `EXPR FOR ALL #{Elt: :{size :Any}} #(DESCRIBE x :Elt) -> Str = #(SHOW (x.size))`,
  with `SHOW` at `Number` and at `Str`, the declaration loads, and a binding
  that leaves `SHOW` no candidate is refused where it is bound.
- A check over a name declared `Any`, or over a module binder, is decided
  where that name is bound:
  `EXPR #(PLAIN y :Any) -> :{size :Number} = #(y)` loads.
- The graph lays no check question that only a `FOR ALL` name's binding
  decides: a test over the programs above counts none.
- The property laws' generated programs hold every check that reads a
  `FOR ALL` name by construction, and still draw checks a name declared `Any`
  decides.

**Directions.**

- *What "holds at every binding" reads — decided.* The name read through its
  bound, as the rigid rule reads it, with no search over bindings. A check
  that holds at every binding only arm by arm is refused here, and loads under
  [a check settled arm by arm](checks-by-arm.md).
- *A check a value decides — decided.* Refused where it reads a `FOR ALL`
  name, though the same body with no `FOR ALL` loads and leaves the check to
  the value. The body tests the value with a `MATCH … WITH` guard naming the
  type.
- *An ascription — decided.* No exemption as the written request for a check.
- *Selection — decided.* Left to each binding. A load-time coverage check is
  [unplanned work](README.md#unplanned-work).
- *A requirement no bound states — decided.* Refused. `FRESH` loads under
  [a bound from below](bounds-from-below.md).
- *How the load reads a derived name from above — open.* (1) Each rule that
  types a derived name gives, beside its closed ends, an upper end written
  over names — `RETYPED TO Elt` at most `Elt`, a decided name at most the
  union of the types on its answers — and the rigid answer of a check reads
  it. (2) The load judges a check under each answer its questions can take,
  which also settles a check arm by arm. Recommended: (1), which leaves each
  rule's closed function alone.

## Dependencies

**Requires:** none.

**Unblocks:**

- [A check settled arm by arm](checks-by-arm.md) — a check this item refuses
  though it holds at every binding.
- [A `FOR ALL` name bounded from below](bounds-from-below.md) — a requirement
  this item refuses for want of a bound.
