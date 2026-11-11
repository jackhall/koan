# A value retyped in place, pinned at its paths

**Problem.** A value the run retypes by its fit to a signature type leaving a
head parameter unpinned is typed exactly at that type pinned at its paths only
where a binder roots it and every walk reaches its path's class
([pinned at its paths](../../src/dispatch/README.md#static-types)). Everywhere
else it stays at most its signature, so a pin over a callable's own variable
never solves against it at load. Under

```text
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
EXPR FOR ALL #[Cr] #(ECHO elem :(Counter WITH {Carrier = Cr})) -> Cr = #(elem.zero)
MODULE ints = ((LET zero = 0))
SIG Outer = #[(VAL inner :Counter)]
MODULE outer = ((LET inner = ints))
```

three kinds of program are refused at load (`no overload of ECHO _ admits
this argument`):

- **A value retyped in place.**
  `EXPR #(VIA g :Any) -> Any = #(ECHO (g :! Counter))` with
  `PRINT (VIA ints)`, though `(LET m = (g :! Counter)) (ECHO m)` prints `0`.
  A path's class is keyed by its content (`Resolved::class_of` in
  [typed.rs](../../src/scope/typed.rs)), and an inline root's class is
  reachable (`Pass::reachable` in [paths.rs](../../src/dispatch/walk/paths.rs))
  only where some binder holds the same content. Pinning an inline root anyway
  breaks the rule that a load verdict holds at run. Written in one body beside
  `LET m = (g :! Counter)`, the static load reaches `ECHO (g :! Counter)`'s
  class through `m`. The body's load at the call, a resumption walk, holds `m`
  at a closed identity while the inline path stands at the open one, so the
  class is unreachable there and the run refuses a call the load admitted.
- **A read through a record's field.**
  `EXPR #(GO rr :{c :Counter}) -> Any = #(ECHO rr.c)` with
  `PRINT (GO {c = ints})`. The run's path binding (`path_binding` in
  [paths.rs](../../src/elaborate/paths.rs)) reads a path through module
  members alone, so a written `:(rr.c.Carrier)` faults at run with
  `has no member Carrier`, and a field read is not pinned
  (`module_chain` in [the walk's paths](../../src/dispatch/walk/paths.rs)).
- **A member of a retyped binder.**
  `EXPR #(GO g :Any) -> Any = #((LET o = (g :! Outer)) (ECHO o.inner))` with
  `PRINT (GO outer)`, and alike over `LET o :Outer = g`, over
  `LET o = (MAKE g)` under `EXPR #(MAKE g :Any) -> Outer = #(g)`, and over
  `ECHO it.inner` in an `Outer:` arm, though `ECHO o.inner` over a parameter
  `o :Outer` prints `0`. The path's class is the retyped module read through
  `inner`, and the holding index records the members of module bodies alone,
  not those of a retyped module, so the class is out of reach.

**Acceptance criteria.**

- `PRINT (VIA ints)` above prints `0`, its `ECHO` decided at load.
- `ECHO (g :! Counter)` written in one body beside `LET m = (g :! Counter)`
  and `ECHO m` is admitted at load and prints `0` at run.
- `ECHO (MAKE g)`, under `EXPR #(MAKE g :Any) -> Counter = #(g)`, and
  `ECHO (EVAL #($g) -> Counter)` each print `0`, decided at load.
- `PRINT (GO {c = ints})` over `ECHO rr.c` above prints `0`, decided at load,
  and a written `:(rr.c.Carrier)` binds at run.
- Each of the four `ECHO o.inner` and `ECHO it.inner` programs above prints
  `0`, decided at load.

**Directions.**

- *How every walk reaches an inline root's class alike — open.* The static
  load and a resumption walk must answer one reachability for the same inline
  path. Alternatives: class an inline root by its site and statement as well as
  its content, so no walk leans on a binder holding that content; or have a
  resumption walk stand an inline root at the identity the static load read it
  at.
- *A path through a record's field — open.* Alternatives: the run's path
  binding steps through a record's field as it does a module's member; or a
  field read roots its paths at its own site, as an inline root does.
- *A retyped module's members — open.* Alternatives: the holding index records
  a retyped module's members beside a module body's; or a member of a binder
  exact at a pinned application reads its class off that application, as a
  written path does.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
