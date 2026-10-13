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
from both is decided only at the run, and `(MAKESET ints).Set` is a parse
error, in type and value position alike. A quantified member whose scheme names
such a parameter is refused at load anywhere but a call's head
(`UnpinnedMember`), since the load has no type to instantiate it at. Where the
run reads `p.Carrier` through a parameter `p :Mapper`, it reads the module's
own type member of that name: a plain module that names none faults, and one
that binds `Carrier` to another type than its members realize reads that type.

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
- A leaf a functor's body binds is keyed on its binder and the call's
  arguments: two calls of `BUILD` from the
  [module design](../../design/modules.md#a-call-unfolds) over one argument give
  one type, and over two arguments of different content *maybe*.
- A module capturing closures, two of which call each other, is a closed tree
  where their captures are, and its view's carrier at load is the run's.
- A quantified member whose scheme names a head parameter its module's
  signature leaves unpinned is instantiated wherever it is wanted, the
  parameter read as the path through the module: with `m :Mapper` declaring
  `VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt)`,
  `LET go :(FN :{x :(m.Carrier), y :Number} -> Number) = m.apply` loads.
- Through `p :Mapper`, `p.Carrier` is, at load and at run, the type `p` fits
  `Mapper` at: a test binds `p` to an opaque view, to a plain module naming no
  `Carrier`, and to one binding `Carrier` to another type than its members
  realize, and reads the view's carrier and the realized type.
- A type expression through an inline call whose tree holds a leaf is refused
  at load.
- Once the load ends, each `:|` ascription's site reads back the content tree
  of its operand: closed, or each leaf beside the binder it is keyed on and the
  body that binds that binder, or beside nothing for a leaf no binder names. A
  test reads the trees of views over a parameter, a capture, a local `LET`, a
  call that unfolds and a `MODULE` body over literals.
- At a `:|` site over a closed tree, the root holds each carrier and the view's
  signature; where the load also knows the operand's signature exactly, it holds
  every type the view door computes from types alone — what *fits* solves, each
  barrier's substitutions and each coerced member's type. A test runs a view of
  a `MODULE` body over literals and finds its habitat's branch unlaid.
- Each rigid that a path over a tree with a leaf stands for reads back, at the
  load, to that tree.
- Chapter [12](../../tutorial/12-functors.md) of the tutorial teaches a path
  through a call.

**Directions.**

- *A path is any module-valued expression — decided.* A type member is read
  through a name, a member chain, a parameter or a call alike, `(e).Name` and
  `(e).name` parsed in type and value position. What differs is what the load
  can say.
- *Two different paths — decided*, per the
  [module design](../../design/modules.md#comparing-two-paths). The load
  compares their content trees, whose nodes are content and whose leaves are
  what it cannot see. A path over a closed tree is the carrier itself, and a
  path over a tree with a leaf is a rigid of the load alone, which the run binds
  to the carrier it computes and no value carries.
- *A call unfolds — decided.* No builtin yields a fresh value, so a call over
  the same trees builds the same content, and the load substitutes its
  arguments into the tree its function builds. A call it cannot unfold is a
  leaf, so no node stands for an application. [Effects](../rewrite/effects.md) leave an
  effectful call folded. A callee is typed before the first call the load
  unfolds, and a function that calls itself, directly or through others, never
  unfolds, so whether a call unfolds does not turn on declaration order.
- *Where a path in a type expression is typed — decided.* The type channel types
  a path through a name, in its own order, as a **path variable**: a lexical
  variable keyed on the path's spelling, read at run off the module its root
  names. The static pass resolves each path variable to the closed carrier its
  tree gives, or to one variable per tree and member, and reads every type the
  channel fixed through that resolution. Deferring every type expression holding
  a path to the static pass was rejected: a callable's own `FOR ALL` levels are
  read off its type, so the static pass would rebuild the channel's numbering.
  A type expression never unfolding was rejected by the criterion on
  `(MAKESET ints).Set`.
- *How a rigid over a leaf is represented — decided.* A lexical variable at a
  level past every name level, so the
  [type lattice](../../lattice/src/types/vocabulary.md) is unchanged and two
  paths over trees with leaves read *maybe* wherever their trees differ. A
  lattice node holding the tree, which would keep *never* there, was rejected
  for its node variant, tree vocabulary, walk arms and laws.
- *A head parameter through a parameter — decided.* Read as the module fits the
  declared type of the path's root — a parameter's, or an annotated binder's —
  through the same fit the view door runs; any other root reads the module's own
  type member. *Fits* taking a
  module's type member of that name as the parameter's pin was rejected: a
  signature may leave out members, so `raw` fitting `Seeded` and `Seeded`
  fitting `Mapper` would not give `raw` fitting `Mapper`, and *fits* would not
  be transitive. Viewing every signature-typed argument at the frame was
  rejected for a view per call and a mint site at every such parameter.
- *A leaf inside an unfolded body — decided.* Keyed on its binder and on the
  trees and solution the call substitutes. Not unfolding a call whose summary
  holds such a leaf was rejected as needlessly imprecise.
- *Closures in a tree — decided.* A knot of closures is a node, each closure
  over its captures' trees and a call to a fellow by its index; a knot holding a
  data or code node is a leaf. The load reaches the digest through the run's own
  recipes, factored to read digests, and walks the shape's component in the
  order the tie lays it.
- *A closed site over an operand the load cannot type exactly — decided.* Its
  carriers and the view's signature are in the root; what *fits* solves from
  the operand, and the types derived from a member's value, are laid at run.

## Dependencies

**Requires:** none — views, member reads and content-keyed carriers run.

**Unblocks:**

- [Effects](../rewrite/effects.md) — an effectful call is a leaf only where calls unfold.
- [The host's scratch as a type cache](../type-tiers/host-scratch-cache.md) — its
  site classes read content trees, closed carriers and each rigid's tree.
- [Dependent signatures](dependent-signatures.md) — a
  signature reads a parameter's member through a path.
