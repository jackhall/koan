# A name surfaced from a module the load knows

**Problem.** A type name a `USING … SCOPE` surfaces is the type its module's
static signature declares it at, where the signature declares it exactly: a
manifest member, or a head parameter the signature's application pins. A member
the signature leaves abstract licenses nothing, as a `:Type` parameter does
([what a body may ask of a `FOR ALL` name](../../design/quantified-types.md#what-a-body-may-ask-of-a-for-all-name)).
That holds even where the load sees the module value whole. Under

```koan
SIG Shaped FOR ALL #[Shape] = #[(VAL s :Shape)]
UNION Maybe = #{Some: Number, None: Null}
MODULE m = (LET s = (Maybe.None null))
LET h = (m :! Shaped)
PRINT (USING h SCOPE (
  LET x :(Shape.None) = (Maybe.None null)
  x))
```

the load knows `h` is `m`, whose `Shape` is `Maybe`, yet refuses the program:
`Shape has no member None: its bound Any declares none`.

**Acceptance criteria.**

- A name surfaced from a module value the load knows exactly reads as the
  closed type the module gives it: the program above prints `None(null)`.
- A name surfaced from a module the load cannot see whole reads as its
  signature declares it, as it does now.

**Directions.**

- *How far the load reads a module value — open.* Reading the value behind a
  header or a view at load is analysis the load does not do yet: it reaches
  every place a module value can flow before a `USING` surfaces it.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
