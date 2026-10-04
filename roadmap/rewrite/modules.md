# Modules

Module programs running, with a module's types keyed on its content and named
through paths.

**Problem.** The [module layer](../../src/knot/module/README.md) builds views and
binds a `USING … SCOPE` block over activations a test binds by hand, but nothing
evaluates a module program: [dispatch](../../src/dispatch/README.md) answers an
ascription and a member read with a fault. An opaque view's carrier is a nonce minted at each
application ([`view.rs`](../../src/knot/module/view.rs)), so two evaluations of
`ints :| Counter` give two types where the
[module design](../../design/modules.md) gives one. No value has a content
digest. No load-time type names a member through a module, so `m.zero` and
`m.succ` are not known at load to share a type. A `MODULE` body may read any
outer name, and nothing at the binder says what it captures.

**Acceptance criteria.**

- An ascription, `m :| Counter` and `m :! Counter`, a member read, `m.f`, and a
  `USING … SCOPE` block evaluate on the rewritten stack.
- Every value carries a content digest, memoized where it is laid down. A
  closure's digest composes its code's with its captures' digests, and closures
  that call one another digest as one component.
- A carrier is keyed on content: two evaluations of `ints :| Counter` give one
  carrier, and so do ascriptions of two separately built modules of equal
  content, while `MAKESET` over an ascending and over a descending `Ordered`
  gives two.
- `m.Carrier` is a load-time type: `m.zero` and `m.succ` read through one path
  share a type at load, and a call solving one variable from both is *always*.
- A body under `MODULE <name> OVER #[<names>]` that reads an outer name its list
  leaves out is refused at load.
- Chapters [11](../../tutorial/11-modules.md) and
  [12](../../tutorial/12-functors.md) of the tutorial run on the rewritten stack.

**Directions.**

- *Identity by content — decided*, per the
  [module design](../../design/modules.md#a-module-is-its-content). A value's
  digest composes as a type's does, and where a value sits in memory plays no
  part.
- *The capture contract restricts the body — decided.* An `OVER` list that only
  documented what the load infers would let it drift from the body.
- *No switch between generative and applicative — decided.* Equal content gives
  equal types, and a module differs between applications only through what it
  captures.
- *What counts as a path — open.* Either only a name a `LET`, a parameter or a
  member chain binds, or anything the load can show is stable. Recommended: the
  names alone, so a module read out of a list or returned by a call is anonymous
  until it is bound.
- *Two different paths — open.* The load reads two different paths as *maybe*
  one type and the run compares their carriers, or reads them as *never* one
  type where it can prove their contents differ — different code, or `OVER`
  values that differ — and *maybe* elsewhere.
- *`OVER` and `CLOSE OVER` — open.* Whether one list both restricts a module's
  body and names what its value copies rather than pins.
- *A module with no `OVER` list — open.* Whether it captures nothing, or whether
  a module that reads an outer value must carry the list.
- *Brands — open.* Two opaque ascriptions of one module meant as distinct types,
  such as `Meters` and `Feet`. Expected to fall out of the capture contract with
  the choice between `:|` and `:!`.
- *A path through an application — deferred.* To
  [effects](effects.md), since only effect tracking tells the load that an
  application's module depends on nothing but the function and its argument.
- *A parameter over a family — deferred.* To
  [families as parameters](families-as-parameters.md).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Families as parameters](families-as-parameters.md) — a signature over a
  family is exercised only by a view that runs.
