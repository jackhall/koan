# Module values

The module value, its self-signature, and the ascription and `USING` doors that
read one.

**Problem.** The rewrite's [scopes](../../src/scope/README.md) build a `MODULE`
body as a module shape — capturing, eager — and its activation is one shape with
a function's ([`Activation`](../../src/scope/activation.rs)). No value stands
for the module once its body has run:
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

- The knot node that closes the callable parameter
  ([functions](../../src/function/README.md)) has a module arm beside the
  function arm — a signature handle, a run of member values and the knot's
  weight — and the value word stays at twenty-four bytes. [`Resolved`](../../src/values/circular.rs) gains a
  third arm, opaque and incomparable as a function's is, so `values` still names
  no module type.
- A module's members rest in one layout order — value members sorted by name,
  then type members sorted by name — which a module body's shape, a signature's
  tables and a `USING` block's parameters all share, so member `i` means the
  same member to each.
- The tie births a module member: a `MODULE` or `GROUP` binder is a member class
  of its own rather than `Opaque`, born from the activation its body ran in,
  which the caller supplies with every slot bound. A module has one
  representation, one birth path and one copy.
- A module value reports its memoized type handle: its self-signature, a value
  slot per value binder at the type its value carries and a manifest member per
  type binder, with no keyworded member and no operator record.
- `USING … SCOPE` is a supported form whose body is a block shape whose
  parameters are the names its operand surfaces, read where the shape is built:
  off a `MODULE` or `GROUP` binder's body, off the `SIG` an ascription at the
  site names, through a `LET` rooted at either, a value or type alias, and a
  `WITH`. An operand whose names cannot be read — a parameter, a call — is
  refused, naming the ascription the site needs.
- `:|` and `:!` are buckets of the [builtin shape
  table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact),
  and the door each names checks the module against the signature and builds a
  view module carrying exactly the signature's members, with each abstract
  member minted per application under `:|` and read at the source's own binding
  under `:!`.
- An opaque view's members carry the view's types: a data member is sealed under
  the mint through one checked constructor beside
  [`construction`](../../src/values/admission.rs), a container is rebuilt cell
  by cell, a nested module is re-viewed, and a function member whose type names
  a mint is a wrapper node holding the function it stands before and the two
  bindings it coerces between.
- A door binds each parameter of a `USING` block to the member it names, and a
  door reads a module's member by name.
- A module crossing a cell is priced and copied as its member run: each member
  is rebuilt through the one crossing, a member that is itself a knot member
  bringing its whole knot with it.
- Every door is exercised over a program shaped and activated in a cell with its
  members bound as the test asks, the way
  [`elaborate`](../../src/elaborate/README.md#testing) exercises a type
  expression: a self-signature, each `USING` reading, a transparent view, an
  opaque one, each coercion, a copy of a module whose body ties a knot, and each
  refusal.

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
  from its own binder's root.
- *What the node holds — decided.* Its signature, its member run in layout
  order, and its weight — not an activation. A view has no body and so no shape
  to activate, and a shape built per application would grow program storage
  without bound; a member run serves a body-born module and a view alike. `M.f`
  is not a knot edge: a member is an index into the run, and the run holds
  members of knots the module does not own, so reading `M.f` out across a cell
  prices that member's whole knot rather than the member.
- *Birth order — decided.* Body first. A module's type and weight are facts
  about the members its body binds, and a knot node is written once, so the
  caller runs the body in an activation that carries no callable — a module's
  captures are never edges — and the tie reads the finished activation.
- *A module body's context — decided, provisionally.* Eager, per
  [src/scope/README.md](../../src/scope/README.md#visibility): a function
  outside a module mutually recursive with one inside it is an eager-cycle
  error.
- *`USING … SCOPE` over a module — decided.* The surfaced names are the block's
  parameters, bound where the block is entered, so a surfaced name resolves like
  any other and no coordinate names a member. The names must be readable where
  the shape is built; a slot typed by a signature may hold a wider module, so a
  parameter is never readable and the site ascribes it.
- *Members are born coerced — decided.* A view's members take the view's types
  where the view is built, so every read surface agrees by construction. The
  seal is a second checked constructor, since an abstract type records no
  representation for [`construction`](../../src/values/admission.rs) to check
  against. A function member is wrapped here and called under
  [modules](modules.md).
- *The keyworded and operator channels — decided.* Empty here. A bucket-only
  definition has no slot and a named combined form is to bind a lambda, both
  [dispatch](dispatch.md)'s; a `GROUP`'s chaining record is
  [operator groups](operator-groups.md)'. A signature declaring a keyworded head
  is therefore unsatisfied until dispatch ships.
- *Where the closing lives — decided.* In `function`. A module node and a
  wrapper node name nothing above `values` and the type lattice, so the closed
  node stays where the tie is, and the view door, the coercion walk and the
  `USING` entry door are a layer above it.
- *What waits for dispatch — decided.* The doors here are built and exercised
  over activations bound by hand, as every layer below
  [dispatch](dispatch.md) is. Evaluating `:|`, `:!` and a member read as
  expressions, calling a wrapped function, and running a module program, are
  [modules](modules.md)'.

## Dependencies

**Requires:** none — the door that turns a `SIG` into a handle ships.

**Unblocks:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — the
  placement of a `MODULE` activation is a criterion there, over a value this
  item builds.
- [Operator groups](operator-groups.md) — a `GROUP` binds a module value.
