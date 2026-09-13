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
[`Tagged`](tagged.rs) or [`TypeValue`](type_value.rs). `Value` is the sum and
the per-kind structs carry the methods, so a consumer that only reads lists
names `List` and never matches on every kind.

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
[`ValueFamily`](../values.rs). Every resident struct is `Copy`, so it is
`Drop`-free by construction and a region releases it whole. Three helpers spell
the shapes `Writer` lays down: `resident` (one value, `fill` at length one),
`collect` (an exact-size run, `fill` with no growth path) and `text`.

A tagged value is the one nominal wrap — a newtype construction, a union
variant, a lowered error — and its identity *is* its type, so no tag symbol
rides beside the payload. `Tagged::hold` keeps every layer of a payload that is
itself tagged; `Tagged::peel` replaces one, so a re-tag never nests.

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

It is also why **no data value is uncopyable**: every region part has a deep
copy and every program part has none to do, so the copy is total.

## The type memo and `satisfies`

Every composite stores its type as a memoized [`type_lattice`](../type_lattice/README.md)
handle, computed in the pass that lays its cells down: a list the join of its
cells' types (`Never` when empty), a dict the joins over its keys and its
cells, a record the record type of its fields in written order, a type value
`OfKind` of the kind of the type it names, a tagged value its identity.
`Value::ktype` copies that handle or names a leaf constant; it reads no registry
and walks nothing.

A type check against a value — [`satisfies`](admission.rs) — is therefore one
lattice relation between the slot and that handle. A slot that reads a
quantifier admits by unification against the handle under a fresh collector,
which checks shape alone: a variable's bound, and two slots of one call agreeing
on a variable, are the caller's collector to solve. Nothing descends into a
value to type it, so a value's precision is whatever its type says, and **an
ascription changes it**: `Value::retyped` stamps a container checked against a
declared node of its own kind with the declared handle over the same shared
runs, and a tagged value checked against a union with the member that names its
constructor. Downstream dispatch then sees the contract rather than the
contents' incidental precision.

The same module answers the question for what is not yet a value.
`admits_part` checks a raw AST part by shape, since an unevaluated literal has
no type memo: a container literal admits on its kind alone, a union on any
member, a kind slot takes a type token only for `ProperType` and `AnyType`, a
quantified slot takes every shape, and a nominal, function, signature or shape
slot takes no raw part at all. `part_ktype` is its inverse — the type dispatch
matched a raw part on, and the one a diagnostic renders — and the two agree for
every part shape. `admits` routes a working part to one or the other.

## Weight

Every composite stores its [`Weight`](weight.rs) beside its type: the bytes a
total rebuild at a destination writes, saturating. A composite weighs its own
resident struct plus every cell it lays down, and a cell weighs a whole `Value`
word plus whatever that word points at in the region — a string its bytes, a
dict key its key word and bytes, a record name its symbol. A tagged value holds
its payload word inline, so it adds only what the payload points at. Program
storage weighs nothing past the pointer. A crossing reads the weight off the
value rather than walking it; a retype shares the runs, so it shares the
weight.

## Crossing

Moving a value between regions is one verb over the substrate's two placement
doors ([crossing.rs](crossing.rs)): `cross` builds a carrier's value into
another cell's region and hands back the carrier resting there, and
`cross_here` builds it into the executing cell, where it is a plain `'here`
value the step may embed or its continuation capture. Both price the operand at
its weight, and both build the same way: a **pinned** operand arrives at the
destination's brand and embeds as it is, and a **copied** one is rebuilt through
`copy_into` — region parts written again through the destination's writer,
program nodes embedded verbatim, memoized types and weights carried over
unchanged.

The graph consults an embedder closure for each operand's verdict, and this
module owns it: [`verdict`](crossing.rs) copies when the copy costs less than a
`COPY_RATIO`th of the bytes a pin would newly retain, and pins otherwise. The
comparison saturates, and the occupancy the prices also carry does not move it.

## Dict key order

A dict is two aligned runs in the region — keys sorted, cells beside them — so a
lookup is a binary search and **entry order is key order**. A key is a string, a
number or a bool behind a private representation, and every door that makes
one refuses NaN and folds `-0` to `0`, so the key order agrees with IEEE
equality on every key there is. The order is total: every bool, then every
number in numeric order, then every string by bytes. A
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
payload; two type values are equal when they name the same handle; two quotes
compare as syntax, part by part with spans ignored. Containers compare their
contents **only when their memoized types are related**, one satisfied by the
other in either direction — an empty list of strings and an empty list of
numbers are unequal. That makes `==` intransitive across ascriptions by design.

`Value::render` is the surface `PRINT` writes: a string bare, a dict key quoted
so `{"1": x}` and `{1: x}` read apart, `[a, b]`, `{k: v}` in key order,
`{x = 1}` in field-name order, a tagged value as its type's name around its
payload, a type as its name, and a quote as its body's surface.

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
the crate.** It names no scheduler type and no scope type, so it carries no
function or module arm — a callable names the environment it captured, which is
the scope layer's. The compiler cannot enforce a module boundary inside one
crate, so [`tests::boundary`](tests/boundary.rs) reads this module's own source
and fails on any other `crate::` path, on an owning heap type (`Rc`, `RefCell`,
`Box`, `Vec`, `String`) outside the test files, and on a lifetime spelled with
a name the rest of the stack retired.

## Testing

The unit suite ([tests.rs](tests.rs)) runs every door, crossing and relation
over a fixture that owns program storage with a type registry built in it, and a
cell graph over that storage to run steps in. One test joins the koan
[Miri slate](../../observe/miri_slate.md): `a_copied_list_outlives_its_home`,
the one path only `values` drives — a deep copy nesting `fill` inside `fill`
with string writes between and a program node embedded, read after the region
it was copied from is released. The pinned and kept paths it would otherwise
pair with are `cellgraph`'s own slate.

## Open work

- [Callable values](../../roadmap/rewrite/callable-values.md) — function and
  module arms, once a scope exists for them to capture.
- [Scheduler on cellgraph](../../roadmap/rewrite/scheduler-on-cellgraph.md) —
  the scheduler that builds its graph with `verdict` and delivers values
  between cells.
- [Scope on values and types](../../roadmap/rewrite/scope-on-values-and-types.md)
  — the binding tables that hold values and their types.
