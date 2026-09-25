# Values

Koan's data values and the per-dispatch expression form, laid down in a cell's
region through [`memory`](../memory/README.md)'s shapes and nothing else.

A value is what a container cell holds, what a binding holds, what a spliced
slot holds and what one cell delivers to another. This module says what one is,
how it is built, what type it has, what moving it costs, and how it moves; it
says nothing about who asks. Dispatch, delivery and name resolution are the
layers above, and they call in here.

## What a value is

A [`Value`](../values.rs) is one `Copy` word of 24 bytes: a number, a bool,
null, a string borrowed where its bytes live, a quoted expression borrowed where
the parse put it, or a borrow of a **per-kind resident struct** —
[`List`](list.rs), [`Dict`](dict.rs), [`Record`](record.rs),
[`Tagged`](tagged.rs) or [`TypeValue`](type_value.rs) — or a **knot member**.
`Value` is the sum and the per-kind structs carry the methods, so a consumer
that only reads lists names `List` and never matches on every kind.

**A knot member is a parameter.** A group of values that refer to one another —
mutually recursive functions, a ring of tagged values, a list holding a
function that captures it — is born together as one
[knot](../memory/README.md#the-knot), and a function holds the environment it
captured, which is the [scope layer's](../scope/README.md#three-tiers), which
`values` may not name. So `Value<'graph, 'cell, X>` has one arm, `Knotted(X)`,
over a type parameter a layer above closes — [`knot`](../knot/README.md)
closes it with a sixteen-byte `(knot, index)` member, so the word stays at 24
bytes. `values` states what it asks of `X` as a trait pair
([values.rs](../values.rs)): per value, `Knotted` — a `Copy` type whose
equality is node identity, with its memoized type handle, its knot's weight,
the fellow member an edge of its own knot names, and what the node holds; per
family, `KnottedFamily` — the member at each region lifetime, and the copy of
its knot from one to another. The parameter defaults to the uninhabited
`Nothing`, whose family is `NoKnot`, so a value spelled without it holds no
knot member and every arm that builds one is unreachable. Every container, the
working expression, and every door and relation over them carry the same
parameter.

**What a member holds** is the one total answer `Knotted::resolve` gives, a
[`Resolved`](circular.rs): a **function** or a **module**, both opaque to
`values` — a module carries no type this module names — or a **data node**, a
[`Circular`](circular.rs). `Value::as_callable` and `Value::as_module` answer
only for their own arm, `Value::as_opaque` for either of the two opaque ones —
which is what equality refuses and rendering writes the type's name for — and
`Value::as_circular` only for the data node, so no arm's meaning rests
on an invariant `values` cannot check. A data node is a list, dict, record or
tagged resident whose cells are [`Link`](link.rs)s instead of value words: a
link is a value word or an `Edge` naming a sibling node of the same knot, since
a sibling has no address until the knot is tied. A link is read only through
the member holding it, which resolves an edge to `Value::Knotted` of the
sibling. The four residents take the cell type as a parameter defaulting to the
value word, so their accessors and deep-copy doors are written once; each has
a `linked` door that lays a data node down under a memo its caller already
derived, and the plain doors stay on value cells. A function's closure
bindings are the same `Link` run.

No type handle rides in the word. A `KType` is a `u128` aligned to 16 bytes, and
one inline would more than double every cell; every handle lives in the resident
struct an arm points at, and a leaf's type is a constant.

Every composite is **born through a door that takes the region's `Writer`** —
the brand-confined capability `cellgraph` hands a step or a build closure — and
comes back as a `&'cell` borrow co-located with everything it points at. There
is no other constructor, no reference count, and no per-value reach
description: a value built in a step is a plain reference whose reach is the
executing cell, and a value crossing a step rides the substrate's carrier,
[`ValueCarrier`](../values.rs), `memory`'s `Ready` bound to
[`ValueFamily`](../values.rs) over a knot-member family. Every resident struct,
and every knot member, is `Copy`, so it is
`Drop`-free by construction and a region releases it whole. Every door lays its
struct down through `memory`'s `resident` and its runs through `collect`, the
two shapes derived from `Writer::fill`; `text` is the one helper here, a string
value over `Writer::text`.

A tagged value is the one nominal wrap — a newtype construction, a
construction through a family, a union variant, a lowered error — and its
identity *is* its type, so no tag symbol rides beside the payload.
`Tagged::hold` keeps every layer of a payload that is itself tagged;
`Tagged::peel` replaces one, so a re-tag never nests.

**A construction has one rule.** [`construction`](admission.rs) takes the
handle a construction's head names and the type of its payload, and answers the
identity the tagged value takes, or a `ConstructionRefused`:

- a **newtype** gives the head itself, or `Misfit` when the payload's type does
  not satisfy its representation;
- a **family with a representation** — a one-parameter `NEWTYPE` family, or a
  parameterized union's variant ([families](../elaborate/README.md#families)) —
  solves the representation's parameters against the payload's type and gives
  the family applied at the solution: `Boxed` over a number gives
  `:(Boxed {Type = Number})`. The solve takes the least solution, so a
  parameter the payload does not reach is `Never`, and a construction of
  `Result.Ok` over a number carries `:(Result.Ok {Ok = Number, Error = Never})`,
  which lies under every `Result` whose `Ok` admits a number. A payload the
  representation cannot be solved against — a structural misfit, or two
  contributions to one parameter with no maximum — is `Unsolved`, naming the
  family and the payload's type;
- any other head is `NotConstructible`: a scalar type, a family that constructs
  nothing, and an applied family, whose arguments a construction does not take
  from its head.

`Tagged::construct` is the checked door over it;
`hold` stays the raw wrap for a union variant, a lowered error and a retype.
Every construction goes through the rule — an ordinary one through `construct`,
a knot's tagged node by the tie over the payload type it derived — so there is
nothing to keep in agreement.

**A seal is the second checked door.** [`sealing`](admission.rs) is what an
opaque view's barrier goes through
([members are born coerced](../knot/module/README.md#members-are-born-coerced)): an
abstract type records no representation for `construction` to check a payload
against, so what is checked instead is that the identity is a *per-application
mint* — a nonced abstract type, or an application of one — and that the payload
satisfies what the source binds that member to. It answers the mint as the
identity, or a `SealRefused`: `NotAMint` for an identity that is no mint,
`Misfit` for a payload the source's binding does not admit. `Tagged::seal` is
the checked door over it, and it `peel`s, so a sealed member takes the mint as
its one tagged layer rather than a second one. Sealing happens where a view is
built, never where a koan program writes a construction.

**A seal its bound reveals is read through.** A sealed value stays a `Tagged`
wrapper — a scalar carries no type of its own, so the mint rides on the wrapper
— and rendering keeps it. A reader that needs a representation reads through
the mint layer exactly where the mint lies under the representation it reads,
which is where the member's declared bound reveals it. [`unsealed`](admission.rs)
is that one reading for equality and dict keys, under the payload's own kind —
`Number` for a number, `LIST OF Any` for a list: a 5 sealed behind a member
bounded by `Number` equals 5, equals another view's sealed 5, and keys a dict as
5. Behind a member bounded by `Value`, or by `Number | Str`, the mint lies under
no one kind, so the seal stays opaque to both. A reader reached through a slot
typed `T` reads through a mint lying under `T` by the same rule. A seal takes
one layer, so there is one layer to read through.

## Two lifetimes

A value borrows at two lifetimes, `Value<'graph, 'cell>`, following
[cellgraph's contract](../../cellgraph/README.md#the-contract-two-embedder-types).
What it holds of **program storage** — a quoted expression's node, a
`ProgramNode` — sits at `'graph`, the lifetime of storage that outlives the cell
graph and that the substrate never retypes. What a **writer laid down** sits at
`'cell`. `'graph: 'cell` always holds, so a program borrow shortens to a region
one where a single lifetime is wanted, and never the reverse.

The split is what makes a quote free to move. A copied operand arrives at the
destination severed, as `Value<'graph, 'severed>`: its region parts are
unreachable and must be rebuilt through the destination's writer, but its
program nodes are `'graph` borrows and embed as they are. **A quoted expression
is the same node on either side of every crossing** — a pin, a copy, and a
forced tree-cell copy alike — and weighs its pointer. No AST node is ever homed
in a cell.

It is also why **every value copies**: every region part has a deep copy,
every program part has none to do, and a knot member is rebuilt by its family, so
the copy is total.

## The type memo and `satisfies`

Every composite stores its type as a memoized [`type_lattice`](../type_lattice/README.md)
handle, computed in the pass that lays its cells down: a list the join of its
cells' types (`Never` when empty), a dict the joins over its keys and its
cells, a record the record type of its fields in written order, a type value
`OfKind` of the kind of the type it names, a tagged value its identity, and a
knot member reports its own. A join across families is their union, so a list
holding a number and a type memoizes `List<(Number | ProperType)>`.
`Value::ktype` copies that handle or names a leaf constant; it reads no registry
and walks nothing.

**A data node's memo is exact and finite.** The lattice has no structural
recursive type, so recursion in a value's type goes through a declared memo: a
function's is its signature type and a tagged node's its newtype, both known
before the knot exists. A container node's memo is derived by the one rule
of its kind — `list_type`, `dict_type` or `record_type`, which the plain doors
use too — from its cells, an edge contributing its target's memo, so
the layer that ties a knot derives it — and refuses a cycle of containers
alone, which no finite type describes
([the tie](../knot/README.md#the-tie)). `satisfies` over a circular value
is therefore the same one relation against that memo.

A type check against a value — [`satisfies`](admission.rs) — is therefore one
lattice relation between the slot and that handle. A slot that reads a
quantifier admits by unification against the handle under a fresh collector,
which checks shape alone: a variable's bound, and two slots of one call agreeing
on a variable, are the caller's collector to solve. Nothing descends into a
value to type it, so a value's precision is whatever its type says, and **an
ascription changes it**: `Value::retyped` stamps a container checked against a
declared node of its own kind with the declared handle over the same shared
runs, and a tagged value checked against a union with the member that names its
constructor. A knot member is never restamped, since a knot never grows a
node. Downstream dispatch then sees the contract rather than the
contents' incidental precision.

The same module answers the question for what is not yet a value.
`admits_part` checks a raw AST part by shape, since an unevaluated literal has
no type memo: a container literal admits on its kind alone, a union on any
member, a family top — `Value` or `Code` — on any concrete type of its family,
a kind slot takes a type token only for `ProperType` and `AnyType`, a
quantified slot takes what its bound takes — a slot bounded by `Value` refuses a
quote — and a nominal, function, signature or shape
slot takes no raw part at all. So a type token is taken by `Code` (as a
`TypeNameToken`) and by `Type` alike. `part_ktype` is its inverse — the type dispatch
matched a raw part on, and the one a diagnostic renders — and the two agree for
every part shape. `admits` routes a working part to one or the other.

## Weight

Every composite stores its [`Weight`](weight.rs) beside its type: the bytes a
total rebuild at a destination writes, saturating. A composite weighs its own
resident struct plus every cell it lays down, and a cell weighs a whole `Value`
word plus whatever that word points at in the region — a string its bytes, a
dict key its key word and bytes, a record name its symbol. A tagged value holds
its payload word inline, so it adds only what the payload points at. A link
weighs its word plus what a value word points at; an edge points at nothing. A
knot member weighs what its own rebuild writes, which its layer memoizes — the
whole knot it sits in, since a member copies by re-tying its knot, and a data
node's resident weighs only its own struct and links. Program storage weighs nothing past the
pointer. A crossing reads the weight off the
value rather than walking it; a retype shares the runs, so it shares the
weight.

## Crossing

Moving a value between regions is one verb over the substrate's two placement
doors ([crossing.rs](crossing.rs)): `cross` builds a carrier's value into
another cell's region and hands back the carrier resting there, and
`cross_here` builds it into the executing cell, where it is a plain `'here`
value the step may embed or its continuation capture. Both price the operand at
its weight, and both build through `cross_view`, which turns one crossed operand
into a value at the destination's brand: a **pinned** operand arrives there and
embeds as it is, and a **copied** one is rebuilt through
`copy_into` — region parts written again through the destination's writer,
program nodes embedded verbatim, memoized types and weights carried over
unchanged, and a knot member handed to its family's copy together with
`copy_into` itself, so every value its knot holds is rebuilt by the same
copy. A data node rebuilds through `Circular::copied`: each value link through
that copy, each edge verbatim — an edge names a node by index, so it means the
same node in the copy — and its memo and weight carried over.

`copy_into` is private to the crossing, and its callers are two public doors,
because a placement a layer above builds over values hands them the views.
`cross_view` rebuilds a value that is itself the crossed operand — a
scheduler's result built in its home. `copy_severed` rebuilds a value held
inside a *copied* operand of another family — a birth that carries values, such
as a call's callee and arguments woken into its frame — at the operand's
severed brand. Each takes a `CrossedOperand`, which only a priced placement
mints — its arms are
[non-exhaustive](../../cellgraph/README.md#the-crossing-verdict), so nothing
outside `cellgraph` can build one — so every copy is one the graph priced.

A crossing door requires its family to be
[`Covariant`](../../cellgraph/src/reattach.rs), which a generic `ValueFamily<XF>`
cannot show, so `cross` and `cross_here` state it as a where-clause and each
concrete family — `ValueFamily<NoKnot>` here, `KValueFamily` in
[knot](../knot/README.md) — says `covariant!` once.

The graph consults an embedder closure for each operand's verdict, and this
module owns it: [`verdict`](crossing.rs) copies when the copy costs less than a
`COPY_RATIO`th of the bytes a pin would newly retain, and pins otherwise. The
comparison saturates, and the occupancy the prices also carry does not move it.
The [scheduler](../scheduler/README.md) builds its graph with this closure, so
every crossing a running program makes is priced by it; how a value reaches
another cell at all is [its delivery](../scheduler/README.md#delivery). Both
doors are generic in the graph's delivery bundle, so a step at any bundle —
`NoDelivery` in a fixture, `KDelivery` under the drain — reaches them.

## Views

Data is immutable, so an edit builds a new value from slices of its source and
the parts spliced between them. A slice, a concatenation or a splice of a list
or a string is a **view**: it reads through its sources' runs rather than
laying down cells or bytes of its own, and a view built from views is one view,
not a chain.

**A view is invisible.** It carries the type of the eager value it stands for,
and equality, rendering and `satisfies` read through it, so a program tells a
view from that value only by what it costs.

**A crossing resolves a view.** A copy writes only the elements a view shows,
as flat runs at the destination, and the copy holds nothing of its sources. A
view weighs what that copy writes, so the [verdict](#crossing) copies a small
view of a large source rather than pinning the source whole; a pinned view
stays a view.

**Only a structural transformation is a view.** A view remaps its sources'
indices and runs no koan code. A transformation that runs code, as `map` and
`filter` do, resolved at a crossing would run that code inside a copy, which
could fail, never finish, or perform an effect wherever the value happens to
cross, at a cost no weight measures. Such a transformation is lazy only as a
stream, a value of its own type, and a copy of a stream copies its pending
call without forcing it.

## Dict key order

A dict is two aligned runs in the region — keys sorted, cells beside them — so a
lookup is a binary search and **entry order is key order**. A key is a string, a
number or a bool behind a private representation, and every door that makes
one refuses NaN and folds `-0` to `0`, so the key order agrees with IEEE
equality on every key there is. The order is total: every bool, then every
number in numeric order, then every string by bytes. A sealed scalar
[its bound reveals](#what-a-value-is) keys as that scalar; any other value is
refused, naming its own type. A
repeated key keeps its last occurrence. The sort and the de-duplication are
staged in a `BumpVec` over the caller's scratch, and string keys are written
into the dict's own region wherever they borrowed from.

A dict keyed this way is a value, not a table: it is built once and never
written. A cell-resident keyed table that a binding channel writes into is the
scope layer's shape, per [memory](../memory/README.md#shapes-not-instantiations).

A record is laid out the same way over field symbols, so a field read is a
binary search too, and two records compare order-blind. Rendering is the one
place symbol order would show, so a record renders its fields in the order of
their names' text.

## Equality and rendering

`Value::equals` is what `==` means over data. Numbers follow IEEE; a tagged
value compares its identity before its payload, so it never equals its bare
payload — save a seal its bound reveals, which is read through on either side;
two type values are equal when they name the same handle; two quotes
compare as syntax, part by part with spans ignored. Containers compare their
contents **only when their memoized types are related**, one satisfied by the
other in either direction — an empty list of strings and an empty list of
numbers are unequal. That makes `==` intransitive across ascriptions by design.

**An opaque member has no structural equality.** `equals` answers
`Result<bool, Incomparable>`: a comparison with a function or a module on either
side is `Incomparable`, which the `==` builtin reports as an error rather than
`false`, and so is a pair of related containers whose aligned cells reach one.
Every aligned pair is compared, so an unequal pair before an opaque member does
not hide it. A container pair with unrelated types is still unequal without
descending, whatever it holds.

**Circular values compare as a bisimulation.** A data node compares as the
plain value of its kind would, its links resolved through it, so a node and a
plain value of the same kind and contents are equal. Two nodes compare under a
set of `(member, member)` pairs already entered: a pair is recorded before its
cells are compared, and a pair met again while recorded counts as equal. That
is the coinductive hypothesis — the greatest fixpoint, bisimilarity — and it is
sound because every result is a conjunction: a hypothesis never turns an
unequal pair equal, and every recorded pair is fully compared by the call that
recorded it. So a one-node ring and a two-node ring with the same contents are
equal. A node against a plain value records nothing, since the plain side is
finite and bounds the descent. Plain and linked composites share one reading,
the private `Composite` view in [circular.rs](circular.rs), so equality,
rendering and the mark pass below are written once over both.

`Value::render` is the surface `PRINT` writes: a string bare, a dict key quoted
so `{"1": x}` and `{1: x}` read apart, `[a, b]`, `{k: v}` in key order,
`{x = 1}` in field-name order, a tagged value as its type's name around its
payload, a type as its name, a quote as its body's surface, an opaque member —
a function, or a module — as its type's name, which is a module's signature; its
closure bindings or members are program state and never print — and a
data node as the plain value of its kind.

**A cycle prints with labels.** A mark pass walks depth first from the first
data node the write meets that no earlier pass entered, entering each node
once, and records every node reached again while it is still being entered;
every cycle holds such a back edge, so every cycle holds a recorded target, and
a plain value with no data node is walked once. The write labels a target at
its first occurrence, `@0 = …`, and writes every later occurrence as `@0`, so
it stops wherever a cycle closes: `LET a = (Ring {next = a})` prints
`@0 = Ring({next = @0})`. Labels count from zero per render in order of first
appearance, and a node that is no target prints inline each time it is reached.

`Value::lower_part` builds a value straight from a region-pure AST part — a
scalar or string literal, a quote, or a container literal whose every element
lowers and whose every dict key is a scalar literal — and refuses anything
that needs dispatch or a scope. The part is checked whole before anything is
written, so a refusal leaves the region untouched.

## Working expressions

A [`KExpression`](../parse/README.md#the-ast-borrowed-copy-and-splice-free) is
parsed AST and never changes. A [`WorkingExpression`](working.rs) is a
dispatch's own copy of one, built in the executing cell's region, whose slots
the scheduler rewrites. Each slot is a [`WorkingPart`](working.rs):

- `Ast` — a part of the node it was made from, a pointer copy at `'graph`;
- `Spliced` — a resolved sub-result, a `Value` reachable at the cell, beside the
  bare name the slot held before the splice so a diagnostic quotes the operand
  as the source spelled it;
- `Expression` — a nested node the scheduler synthesized, as an operator-chain
  fold's accumulator is;
- `RecordType` — a `:{…}` body whose co-declared references are threaded,
  kept apart so the slot still classifies as a record type;
- `StagedSlot` — a positional hole whose value a sibling dispatch is producing.

A working expression carries the same [`NodeCache`](../parse/ast/shape.rs) a
parsed node does. `from_ast` carries the parsed node's cache over whole — at
`'cell`, which the `'graph` borrow shortens to — since a splice substitutes
slots one for one and writes no keyword. `respliced` keeps the key and reads
only the head class again. A node the scheduler builds from scratch computes
its cache from its own key and has no binder plan, because a binder is always
parsed AST. A synthesized node takes its origin's file and the extent its own
parts span.

A working expression is never a value and never crosses a cell: a continuation
captures it at the cell's own lifetime. That is what keeps the AST splice-free —
a resolved sub-result or a staging hole exists only here.

## The import rule

**Outside doc comments and `#[cfg(test)]`, `values` names `crate::memory`,
`crate::parse`, `crate::source` and `crate::type_lattice`, and nothing else in
the crate.** It names no scheduler type, no scope type and no function type,
so its knot-member arm is a parameter rather than a function or module arm. The
compiler cannot enforce a module boundary inside one
crate, so [`tests::boundary`](tests/boundary.rs) reads this module's own source
and fails on any other `crate::` path, on an owning heap type (`Rc`, `RefCell`,
`Box`, `Vec`, `String`) outside the test files, and on a lifetime spelled with
a name the rest of the stack retired.

## Testing

The unit suite ([tests.rs](tests.rs)) runs every door, crossing and relation
over a fixture that owns program storage, a bump for the type registry, and a
cell graph over that storage to run steps in. Circular values are exercised
without `knot`: the fixture closes the parameter with a test-only member
whose every node is a data node, tied through `KnotPlan` with memos supplied
by hand, and the suites cover the `linked` doors, the construction rule and the
seal ([tests/construction.rs](tests/construction.rs)), bisimilar and unequal rings
([tests/equality.rs](tests/equality.rs)), labelled and shared-inline renders
([tests/render.rs](tests/render.rs)), a ring crossed under a copy and a pin
([tests/crossing.rs](tests/crossing.rs)), and `satisfies` by a node's memo
([tests/satisfaction.rs](tests/satisfaction.rs)). One test joins the koan
[Miri slate](../../observe/miri_slate.md): `a_copied_list_outlives_its_home`,
the one path only `values` drives — a deep copy nesting `fill` inside `fill`
with string writes between and a program node embedded, read after the region
it was copied from is released. The pinned and kept paths it would otherwise
pair with are `cellgraph`'s own slate.

## Open work

- [Slicing and splicing](../../roadmap/rewrite/slicing-and-splicing.md) — views
  over lists and strings, resolved at a crossing.
- [Yielding iterators](../../roadmap/rewrite/yielding-iterators.md) — streams,
  the lazy transformations that run koan code.
- [Dispatch](../../roadmap/rewrite/dispatch.md) — a builtin reading a sealed
  argument through a mint lying under the slot type it was reached through.
