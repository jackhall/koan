# Module values

The module value, its self-signature, and the ascription and `USING` doors that
read one.

**Problem.** The rewrite's [scopes](../../src/scope/README.md) build a `MODULE`
body as a module shape — capturing, eager — and its activation is one shape with
a function's, down to the `callable` field
[`Activation`](../../src/scope/activation.rs) already documents as a callable's
*or module's*. No value stands for the module once its body has run:
[`Value`](../../src/values.rs)'s knot-member parameter is closed by a function
node and a data node alone, nothing elaborates a module's self-signature from
the members its activation binds, and the shape builder reports `USING … SCOPE`
unsupported because resolving a surfaced name statically needs the module's
signature. The other half rests in the lattice, where no koan program reaches
any of it: `Signature`, `SigSchema`, `sig_subtype`,
`meet_schemas`, the signature wall in `OfKind`, and the generativity an opaque
ascription mints
([src/type_lattice/README.md](../../src/type_lattice/README.md#the-node-vocabulary)).
The `:|` and `:!` ascription operators are reserved symbols
([src/parse/builtin_shapes/binder.rs](../../src/parse/builtin_shapes/binder.rs))
with no bucket of their own. The old runtime's
[`Module`](../../src/machine/model/values/module.rs) holds a bare `&Scope` and a
frozen member table, both of which the rewrite replaces.

**Acceptance criteria.**

- The callable parameter [functions](../../src/function/README.md) close gains a
  module node beside the function node, and the value word stays at twenty-four
  bytes. [`Resolved`](../../src/values/circular.rs) gains a third arm, opaque
  and incomparable as a function's is, so `values` still names no module type.
- The tie births a module member: a `MODULE` or `GROUP` binder is a member class
  of its own rather than `Opaque`, so a module has one representation, one birth
  path and one copy.
- A module value reports its memoized type handle: its self-signature,
  elaborated from the members its activation binds.
- `USING … SCOPE` over a module whose signature is known statically resolves
  the surfaced names in the shape; over one whose signature is not, it
  requires an ascription at the site and is refused without one.
- `:|` and `:!` are buckets of the [builtin shape
  table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact),
  and the door each names builds a view module carrying the signature's members,
  with opaque members minted per application under `:|`.
- A module crossing a cell is priced and copied as its activation: each member
  the activation binds is rebuilt through the one crossing, a member that is
  itself a knot member bringing its whole knot with it.
- Every door is exercised over a program shaped and activated in a cell with its
  members bound as the test asks, the way
  [`elaborate`](../../src/elaborate/README.md#testing) exercises a type
  expression: a self-signature, a `USING` resolution, a transparent view, an
  opaque one, a copy of a module whose body ties a knot, and each refusal.

**Directions.**

- *A module is a knot node — decided.* Closing the callable parameter with an
  enum over a function member and a module reference costs eight bytes the value
  word does not have; the node keeps the member at sixteen. The node is the
  carrier, not a claim about cycles. A module body is an eager context
  ([src/scope/README.md](../../src/scope/README.md#visibility)), and a mention is
  classified by its path from its binder's root down to the outermost callable
  body it crosses — so every mention reached from a module binder's root is
  eager whatever callable body it sits in, and a component holding a module
  member and a fellow, or a module naming itself, is an eager cycle refused
  where the shape is built. Every module born is therefore a one-node knot.
  Knots *inside* a module body are ordinary, since a mention there is classified
  from its own binder's root. The arm admits a multi-node case if the
  eager-module rule is ever relaxed; nothing here relaxes it.
- *A module node has no edges of its own — decided.* `M.f` is not a knot edge:
  a module's knot and the knot a component of its body ties are two knots, tied
  by two ties in two shapes, and `Knotted::sibling` from a module node reaches
  nothing. A member is reached by a slot read on the activation. So the node
  carries no links as a data node does, and its weight and its copy are its
  activation's — whose slots hold members of knots it does not own, so reading
  `M.f` out across a cell prices that member's whole knot rather than the member.
- *A module body's context — decided, provisionally.* Eager, per
  [src/scope/README.md](../../src/scope/README.md#visibility): a function
  outside a module mutually recursive with one inside it is an eager-cycle
  error.
- *`USING … SCOPE` over a module — decided.* The surfaced names come from the
  module's signature, which must be known statically at the `USING` site; a
  module whose signature is not requires an ascription there.
- *Where the closing lives — decided.* The closed callable moves from
  `function` to this layer, which is the top of the callable stack.
- *What waits for dispatch — decided.* The doors here are built and exercised
  over activations bound by hand, as every layer below
  [dispatch](dispatch.md) is. Evaluating `:|`, `:!` and a member read as
  expressions, and running a module program, are [modules](modules.md)'.

## Dependencies

**Requires:** none — the door that turns a `SIG` into a handle ships.

**Unblocks:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — the
  placement of a `MODULE` activation is a criterion there, over a value this
  item builds.
- [Operator groups](operator-groups.md) — a `GROUP` binds a module value.
