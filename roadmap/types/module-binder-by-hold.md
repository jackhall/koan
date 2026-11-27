# A module pinned at its paths however it is held

A module held by a `:!` over a parameter declared `:Any`, or read through a
record's field or a retyped binder's member, typed as its `LET` spelling is.

**Problem.** A value the run retypes by its fit to a signature type leaving a
head parameter unpinned is typed exactly at that type
[pinned at its paths](../../src/dispatch/README.md#static-types) only where a
module binder roots it. Everywhere else it stays at most its signature, so a
pin over a callable's own variable never solves against it at load. Under

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
EXPR FOR ALL #[Cr] #(ECHO elem :(Counter WITH {Carrier = Cr})) -> Cr = #(elem.zero)
MODULE ints = ((LET zero = 0))
SIG Outer = #[(VAL inner :Counter)]
MODULE outer = ((LET inner = ints))
```

three kinds of program are refused at load (`no overload of ECHO _ admits
this argument`), though each one's `LET` spelling prints `0`:

- **A hold over a parameter declared `:Any`.** A binder is a module binder
  where its static type names a signature (`module_unknown` in
  [`walk/reads.rs`](../../src/dispatch/walk/reads.rs)), so a parameter
  declared `:Any` is none, whatever its body does with it, and a `:!` held in
  place over it is not pinned
  ([paths](../../src/dispatch/walk/paths.rs)). These are refused at load:

  ```koan
  EXPR #(INLINE g :Any) -> Any = #(ECHO (g :! Counter))
  ```

  ```koan
  EXPR #(MIX g :Any) -> Any = #(
    (LET x = g)
    (PRINT (ECHO (g :! Counter)))
    (ECHO (x :! Counter))
  )
  PRINT (MIX ints)
  ```

  ```koan
  EXPR #(BOTH g :Any) -> Any = #(
    (LET m = (g :! Counter))
    (PRINT (ECHO m))
    (ECHO (g :! Counter))
  )
  PRINT (BOTH ints)
  ```

  The same hold over `g :Counter` or `g :Module` prints `0`. Pinning such a
  hold at `g`'s paths alone makes `g` a module binder for some of its readers
  and not others. The instance laws draw no module parameter declared `:Any`
  ([`generate.rs`](../../src/dispatch/tests/generate.rs)), so no law reaches
  the case.
- **A read through a record's field.** A field read is not pinned
  (`module_chain` in [the walk's paths](../../src/dispatch/walk/paths.rs)),
  so this is refused at load:

  ```koan
  EXPR #(GO rr :{c :Counter}) -> Any = #(ECHO rr.c)
  PRINT (GO {c = ints})
  ```

  The run's path binding (`path_binding` in
  [paths.rs](../../src/elaborate/paths.rs)) reads a path through module
  members alone, so a written `:(rr.c.Carrier)` faults at run with
  `` `Carrier` names no binding visible here ``. With `(LET i = rr.c)` first,
  `ECHO i` prints `0` and `:(i.Carrier)` binds.
- **A member of a retyped binder.** A member read off a binder the run
  retyped to a signature is not pinned, so this is refused at load:

  ```koan
  EXPR #(GO g :Any) -> Any = #(
    (LET o = (g :! Outer))
    (ECHO o.inner)
  )
  PRINT (GO outer)
  ```

  and alike over `LET o :Outer = g`, and over `LET o = (MAKE g)` under
  `EXPR #(MAKE g :Any) -> Outer = #(g)`. With `(LET i = o.inner)` first,
  `ECHO i` prints `0`, as `ECHO o.inner` over a parameter `o :Outer` does.

**Acceptance criteria.**

- A value binder is a module binder where its declared type names a
  signature, or where its shape's text holds a `:!` over a name chain rooted
  at it; every reader of the binder reads it alike.
- `INLINE` above loads and `PRINT (INLINE ints)` prints `0`; `MIX` and
  `BOTH` load and each print `0` twice.
- The instance laws draw a module parameter declared `:Any` and held in
  place, and an instance equals a fresh load over it
  ([properties](../../src/build/README.md#properties)).
- A value not a module held at such a binder faults at the hold.
- `PRINT (GO {c = ints})` over `ECHO rr.c` above prints `0`, decided at load,
  and a written `:(rr.c.Carrier)` binds at run.
- Each of the three `ECHO o.inner` programs above prints `0`, decided at
  load.

**Directions.**

- *Where the module-binder fact lives — decided.* On the shape, per slot,
  read by `module_unknown`, so every reader agrees.
- *First step — decided.* Add `ModuleAt::Any` to the law generator
  ([`generate.rs`](../../src/dispatch/tests/generate.rs)) before any code
  changes, so the laws reach the case.
- *Cost — open.* A binder held as a module gives its entry's bind site two
  names, the type its module carries and its content
  ([what a bind site binds](../../design/binding-graph.md#what-a-bind-site-binds));
  measure the views and instances it adds before landing.
- *A path through a record's field — open.* Alternatives: the run's path
  binding steps through a record's field as it does a module's member; or a
  field read roots its paths at its own site, as a part held in place does.
- *A retyped module's members — open.* Alternatives: a member of a binder
  exact at a pinned application reads its class off that application, as a
  written path does; or the member read is a holder of its own, as its `LET`
  spelling is.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
