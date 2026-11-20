# A value binder held as a module

A value binder whose body holds it as a module, read as a module binder.

**Problem.** A binder is a module binder where its static type names a
signature (`module_unknown` in
[`walk/reads.rs`](../../src/dispatch/walk/reads.rs)), so a parameter declared
`:Any` is none, whatever its body does with it. A `:!` held in place over a
name chain is pinned at the paths through that chain only where the chain's
root is a module binder
([paths](../../src/dispatch/walk/paths.rs)), so a hold over such a parameter
stays at most its signature. Under

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
EXPR FOR ALL #[Cr] #(ECHO elem :(Counter WITH {Carrier = Cr})) -> Cr = #(elem.zero)
MODULE ints = ((LET zero = 0))
EXPR #(INLINE g :Any) -> Any = #(ECHO (g :! Counter))
```

`INLINE` is refused at load, since `(g :! Counter)` can never fit a pin over
`Cr`, and so are

```koan
EXPR #(MIX g :Any) -> Any = #(
  (LET x = g)
  (PRINT (ECHO (g :! Counter)))
  (ECHO (x :! Counter))
)
PRINT (MIX ints)
```

and

```koan
EXPR #(BOTH g :Any) -> Any = #(
  (LET m = (g :! Counter))
  (PRINT (ECHO m))
  (ECHO (g :! Counter))
)
PRINT (BOTH ints)
```

Pinning such a hold at `g`'s paths alone makes `g` a module binder for some of
its readers and not others, and both programs then fail at the run. The
resumption laws draw no `:Any` module parameter, so no law reaches the case.

**Acceptance criteria.**

- A value binder is a module binder where its declared type names a
  signature, or where its shape's text holds a `:!` over a name chain rooted
  at it; every reader of the binder reads it alike.
- `INLINE` above loads and `PRINT (INLINE ints)` prints `0`; `MIX` and
  `BOTH` load and each print `0` twice.
- The resumption laws draw a module parameter declared `:Any` and held in
  place, and resume equals restart over it.
- A value not a module held at such a binder faults at the hold, as today.

**Directions.**

- *Where the fact lives — decided.* On the shape, per slot, read by
  `module_unknown`, so every reader agrees.
- *First step — decided.* Add `ModuleAt::Any` to the law generator
  ([`generate.rs`](../../src/dispatch/tests/generate.rs)) before any code
  changes, so the laws reach the case.
- *Cost — open.* A binder held as a module gives its frame a binding event;
  measure the memo and activation cost before landing.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
