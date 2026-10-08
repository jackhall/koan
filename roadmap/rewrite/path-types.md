# Path types

A type member named at load through a module-valued expression, and a call
unfolded into the content it builds.

**Problem.** No load-time type names a member through a module or a call. The
run keys an opaque view's carrier on content, but the
[type channel](../../src/elaborate/README.md#the-type-channel-at-load) answers
`:(m.Carrier)` as unknown at load, and
[dispatch](../../src/dispatch/README.md#static-types) types a member read by
its declared type in the operand's signature, so one declared at a head
parameter the signature leaves unpinned reads as unknown. So `m.zero` and
`m.succ` are not known at load to share a type, a call solving one variable
from both is decided only at the run, and `(MAKESET ints).Set` written at two
sites is not known to be one type. A quantified member whose scheme names such
a parameter is refused at load anywhere but a call's head (`UnpinnedMember`),
since the load has no type to instantiate it at.

**Acceptance criteria.**

- `m.Carrier` is a load-time type: `m.zero` and `m.succ` read through one path
  share a type at load, and a call solving one variable from both is *always*.
- The load reads two paths by their
  [content trees](../../design/modules.md#comparing-two-paths): `meters.Carrier`
  and `feet.Carrier` over opaque views of two `MODULE` bodies of different code
  are *never* one type, two `LET`s of `ints :| Counter` are *always*, a member a
  body binds or `:!` solves reads as the type it is, and a parameter's
  `p.Carrier` against any other path is *maybe*.
- A call [unfolds](../../design/modules.md#a-call-unfolds):
  `(MAKESET ints).Set` written at two sites is one type at load, the carrier the
  run computes; `(MAKESET asc).Set` against `(MAKESET desc).Set` is *never*; a
  functor that ignores its argument gives one type over two arguments; and a
  call that does not unfold is a leaf, keyed on the `LET` that binds it.
- A quantified member whose scheme names a head parameter its module's
  signature leaves unpinned is instantiated wherever it is wanted, the
  parameter read as the path through the module: with `m :Mapper` declaring
  `VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt)`,
  `LET go :(FN :{x :(m.Carrier), y :Number} -> Number) = m.apply` loads.
- Chapter [12](../../tutorial/12-functors.md) of the tutorial teaches a path
  through a call.

**Directions.**

- *A path is any module-valued expression — decided.* A type member is read
  through a name, a member chain, a parameter or a call alike. What differs is
  what the load can say.
- *Two different paths — decided*, per the
  [module design](../../design/modules.md#comparing-two-paths). The load
  compares their content trees, whose nodes are content and whose leaves are
  what it cannot see. A path over a constant tree is the carrier itself, and a
  path over a tree with a leaf is a rigid of the load alone, which the run binds
  to the carrier it computes and no value carries.
- *A call unfolds — decided.* No builtin yields a fresh value, so a call over
  the same trees builds the same content, and the load substitutes its
  arguments into the tree its function builds. A call it cannot unfold is a
  leaf, so no node stands for an application. [Effects](effects.md) leave an
  effectful call folded.
- *Where a path in a type expression is typed — open.* The type channel fixes
  every type expression before the static pass selects a callee, and a call
  unfolds only once its callee is selected and its group solved. Either the
  type channel defers a type expression reading a member through a value, and
  everything holding one, to the static pass, which types it through trees, so
  a type expression through an inline call that does not unfold is refused at
  load; or a type expression never unfolds, and only a member read does.
  Recommended: deferring.
- *How a rigid over a leaf is represented — open.* Either a lexical variable at
  a level past every name level, which leaves the
  [type lattice](../../lattice/src/types/vocabulary.md) unchanged and reads two
  paths with leaves as *maybe* wherever they differ, so the design's *never*
  between trees differing at content holds only for constant trees; or a
  lattice node holding the tree, which keeps that *never* at the cost of a node
  variant, a tree vocabulary in the registry, and its walk arms and laws.

## Dependencies

**Requires:** none — views, member reads and content-keyed carriers run.

**Unblocks:**

- [Effects](effects.md) — an effectful call is a leaf only where calls unfold.
- [Dependent signatures](../gradual-typing/dependent-signatures.md) — a
  signature reads a parameter's member through a path.
