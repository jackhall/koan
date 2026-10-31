# Scopes

Koan's lexical environments: the layer that answers what a name means at the
point it is read, over the values in [`values`](../values/README.md) and the
types in [`type_lattice`](../../lattice/src/types/README.md).

A scope resolves value names, type names and bucket keys. A keyworded use
resolves where its shape is built, as a name does, to the list of callables it
may select ([keyworded uses](#keyworded-uses)); choosing one of them per call is
[dispatch](../dispatch/README.md), the layer above.

## Three tiers

A scope is built in three tiers, each at the moment its contents become known.

- **The body shape** — `BodyShape`, one per body, built once in program storage
  and shared by every scope instance of that body. It **owns the body's
  statements**, with every [operator run](#operator-groups) in them already
  chained, and it records the value names and
  type names the body declares, each with the lexical position its binder writes
  at, the class of every mention and the components its bindings form, and it
  resolves every name the body reads. It walks a builtin node's parts by the
  [roles](../parse/builtin_shapes/role.rs) that node's `BUILTIN_SHAPES` entry
  gives them. Each shape a load builds carries its **segment**, an index the
  one shape builder assigns in the order it seals shapes: distinct per shape,
  the same in every load of one text, and what a
  [settlement's key](../build/README.md#keys-which-bindings-make-a-settlement)
  names the body by, beside its base's digest. Every shape of a load reaches the one table the builder
  fills once the root seals, so any shape finds any other by its segment
  (`BodyShape::shape_at`).

  **A reader takes a body's statements from `BodyShape::body()`, never from the
  parse.** Every site a shape records — a mention's part, a nested shape's
  holder, a `LET`'s right-hand side — is an address inside that run, so a reader
  holding the parsed statements instead would look its own facts up against
  nodes the shape never saw.
- **Closure bindings** — one run per callable, held in the callable value and
  built when the callable is born. Each slot is a `values::Link`, the one
  value-or-edge type a knot's data node holds too. Each name the body reads
  from an enclosing scope is copied in shallowly: the binding's value word, not
  a deep copy of what it points at. A name that is a member of the callable's
  own [component](#visibility) is held as an edge into the knot the component
  is born in, never as a copied value. A callable's birth reads every capture into
  scratch first and lays the run down only once every read is finished, so a
  closure binding is never a placeholder. Beside the run the bindings carry
  the arena node the callable was born under, an
  [`ArenaRef`](../build/README.md#references-between-frames-and-the-arena) a
  call seeks its settlement beneath, the [stamp](stamp.rs) a call replays
  where that arena node is gone, and the layers its encloser's activation
  decided before its birth ([`closure`](closure.rs)); none is content, so the
  closure's digest and equality read the run alone.
- **Per-call bindings** — one activation per call, laid down in the call's
  frame region, and the top level's in the region of the program's
  [root](../program/README.md#the-top-level): a pointer to the callable's
  closure bindings, the callable itself, and one slot for each parameter and
  each local the shape declares.
  Nothing is copied out of
  the closure bindings; a read of a capture goes through the pointer, one load
  more than a read of a local. Copying the captures in would cost a word per
  capture per call, multiplied by recursion depth, and a copied knot edge would
  need its knot carried beside it, where an edge left in its node resolves
  against the knot it already lives in.

  An activation has a read half of its own, the **`ActivationView`**: the
  header pointers, the callable, a read view of the slots, and the digest memo
  of each slot the load marks digested (`digest_memo`), with no door that
  binds. An evaluation is handed the view of the activation it was asked from
  and reads names through it where they lie, so a binding never travels to its
  reader. The view is covariant in its region brand, so it rides a birth into a
  shorter-lived cell; the `Activation` — the view beside the slot array, which
  binds — is invariant and stays where it was laid down, reading as its view
  through `Deref`. Both are generic over the member's *family* rather than the
  member, because a slot holds its value erased and names its payload through
  the family ([the slot array](../memory/README.md#the-slot-array)). The view
  takes the member as a type parameter of its own, defaulted to the family's
  member at the view's brand, and nothing but the default is ever meant:
  rustc computes variance over the unnormalized field types, and a field naming
  the brand through the family's projection would make the view invariant in
  it. For the same reason `knot`'s `KActivationView` spells its member out, so a
  type holding one is covariant too; the tie and
  [elaboration](../elaborate/README.md) read the one view type at every level
  and name no habitat.

Values are immutable, so a shallow copy of a binding means the same thing as a
reference to it: every copy names the same value, and what the copy retains is
the region that value lives in.

**Five kinds of shape.** The program's top level is one shape with no
captures. A function body (`FN`, `EXPR`, `OP`) is a *callable* shape: it
captures, and it is a deferring boundary (below). Its parameters are the names
its signature declares — each `<name> :<Type>` pair, a `:{…}` schema's fields,
every `FOR ALL` type parameter — or `left` and `right` for a binary `OP` and
`operands` for a unary one, all at position `0`. A `MODULE` or `GROUP` body is
a *module* shape: it captures, since its activation outlives the frame that
births it, but it is not a deferring boundary, because its statements run when
the statement holding it runs. A `MATCH` or `TRY` arm, a `USING … SCOPE` body,
and the block a [pairwise rewrite](#operator-groups) synthesizes to hoist
a shared operand, is a *block* shape: an arm's one parameter is `it`, a `USING`
body's are
the names its operand surfaces (below), a synthesized block's are the anonymous
slots its hoists bind, its statements count
from `1`, its activation is laid down in the same frame as the enclosing one,
and instead of captures it holds a pointer to the enclosing activation. A name
declared in the block shadows the enclosing one from the next statement on and
is gone once the block ends, because the block's activation is not the
enclosing one. A quote's code is a *code* shape ([quotes](#quotes)): it
captures its `$` names where the quote is written, as a callable captures, and
holds its holes and unfilled `\` marks open for `USING` and `EVAL` to fill; its
activation is laid down in the frame an `EVAL` runs in, with no pointer to the
enclosing one.

One builder serves all five kinds. It walks a body once, building every body
and arm nested in it as it meets them, and lays each finished shape down in
program storage. A nested shape is found from its enclosing one by the address
of the part that holds it, and a mention by the address of its own part — an
identity any holder of the part recomputes, and which program storage never
moves. For a callable, the part that holds its body is its form's body-role
part, and `Site::of_body` finds that site from the form node, so a caller that
meets a `FN` no binder names finds its body shape.

**Code is read where it is written.** Before the builder walks a builtin node's
parts it checks each part the node's roles read as written
([the builtin shape table](../parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)):
an `EXPR` head is a quote, and a callable's body a quote or a part evaluated
to code; an arm set, a union's variants, a `FOR ALL` group and a `SIG` body are
a list or dict of quotes; a
binder name, an in-place body, a type expression and a `TRY` or `CATCH` operand
are bare; and each admits one of its slot's code types. A type expression's and
an in-place operand's slot type is the value it denotes, so only their spelling
is checked. One static check does it, against the table's own types through
[`admits_part`](../values/admission.rs) over the program's registry — the one
rule a raw part is admitted by anywhere — so no position list and no second
admission rule sits beside the table; the readers after it assume a well-formed
part. A part no slot type admits is `Inadmissible`, which names the slot's type
through the lattice's renderer. A callable's body that is no quote is typed by
the value it evaluates to, where the program loads ([building code](#building-code)).
A callable's body shape is keyed by its body part, so `Site::of_body` finds it
as it finds any body: built over a written quote's body, over the quote a lone
name traces to, or holding no statements where its code arrives at run time. An `EXPR` head's
names are read through its quote. An arm set is a dict of guard quotes to arm
quotes, and each arm is a block shape binding `it`. A `MATCH … WITH` guard is a
type — a type name, a `:(…)` or a `:{…}`, whose names are mentions of the shape
holding the `MATCH`, typed where the program loads
([load-time types](#load-time-types)) — or a lone scalar literal, so its arm
set is typed `Dict(TypeCode | Literal, Block)`; a `MATCH … UNDER` or `TRY`
guard is a label, typed `Dict(Name, Block)`, and names no binder. A key that
is a nested quote, or a marked name as a label, is `NotAGuard`. A `UNION`
declaring one variant is refused where it is declared, `LoneVariant`: it would
be the same type as its variant, which a single `NEWTYPE` declares. A
`SIG` body's members and a bodyless `GROUP`'s heads are the statements of a
list's quotes, each walked by its own builtin shape's roles.

**An arm knows it is one.** An arm's block shape carries an `Arm`
(`BodyShape::arm`): its `Guard` as written — a type quote or a scalar literal
under `MATCH … WITH`, a label under `MATCH … UNDER` or `TRY`, or `Default` for
the `_` arm — and whether the arm's last statement is in tail position. It is
exactly where the `MATCH` or `TRY` is: at the root of a body's last statement,
binding nothing, in a tail body. A callable's body is a tail body and an arm's
is when its own tail is; the program, a module, a `USING` body and a
synthesized block are not. A label, a value guard or `_` written twice in one
arm set is refused as `RepeatedGuard` where the builder reads the set, a value
guard compared by value, so `1` and `1.0` are one; a type written twice is
refused where the walk types it, by handle
([guards](../elaborate/README.md#the-type-channel-at-load)). A tail arm's last
statement is one of its owner's tail statements (`Shapes::tail_owner`, through
every tail arm out to the callable or code body the `MATCH` ends): its calls
are tail edges of that owner, and pair the owner's parameters it passes bare.

## Resolution

Every name a body reads resolves when its shape is built, to one of three
coordinates:

- a slot of the body's own per-call bindings,
- a slot of its closure bindings, read through the activation's pointer, or
- an index into the builtin table, read through the reader's own header.

A slot and a closure binding are prefixed by how many enclosing block
activations to step through first: none for a callable's own body, one per
block the reader sits inside. A builtin carries no such count, so a builtin
coordinate that steps out is not expressible. Reading a
name is then one indexed load, two for a capture, plus one per enclosing
block. No runtime walk visits an enclosing *scope* by name, and no binding
holds a reference into another scope: a coordinate is computed from the name
at the reading site, and what a slot holds is a value or an edge into the
holder's own knot.

A name a callable body reads from outside is added to its capture layout once,
however often the body reads it, with where the callable's birth reads it from:
a coordinate of the enclosing activation, or — when the name is a fellow member
of the component the binder of the callable's statement belongs to — that
member's place in the component, which the birth turns into a knot edge the
caller mints. That covers a callable a binder births and a `FN` a data binder's
constructor slot holds alike.

A callable or module also holds a **type capture** past the builder's captures
for each run-bound type name of the bodies around it on its chain that is
bound when its birth runs: its **layout** (`Shapes::layout`), each beside the
coordinate of the enclosing activation its birth reads. So a body reads a type
name it never writes. Births read the builder's captures, then the layout, so
the closure is `capture_count()` slots long. The closure's digest and equality
compose a type capture only where its **contract** names it: where a
[contribution](../dispatch/README.md#static-types) or an instance's solution
in the body, or in a callable nested in it, names the variable
([the folds](../build/README.md#the-folds)). A type capture is never merged
with a builder capture of the same name.

**A module body is held to its capture contract**
([build.rs](shape/build.rs)), per the
[module design](../../design/modules.md#the-capture-contract). A `MODULE` or
`GROUP` binder may write an `OVER` list after its name,
`MODULE m OVER #[x Elt (HELPER _)] = (…)`: each entry a value or type name, or a
registration's key written with `_` in each slot, read as data where the
binder's statement is walked. An **outer** name is one the body reads that
neither the body nor a top-level statement binds, each capture followed to its
source; a top-level binding or a builtin is read where it lives and needs no
listing. The first outer name the list leaves out refuses the body,
`` `x` is read from outside this module; list it under its `OVER`, as
`OVER #[x]` ``, a registration named by its key; a body with no list captures
nothing from outside. A type capture the body's contract composes must be
listed too, which the load's [walk](../dispatch/README.md#static-types) checks.
An entry the body never reads is captured all the same, and a top-level entry
is recorded by its binding (`BodyShape::listed_top`), since the run never reads
one through a capture. An entry naming nothing visible is `Unbound`. Inside a
quote's code, a read that reaches one of the code's open holes or `\` marks is
no outer name: the `USING` or `EVAL` filling it decides it, and the module's
content composes what it is filled with. A name the code itself binds outside
the module is outer, as anywhere.

**A body's code is digested as resolved** where its shape is built
([digest.rs](shape/build/digest.rs)), nested shapes first, into the content
digest a closure's or a module's [content](../values/README.md#content-digests)
composes: its kind, its parameter layout and each statement part by part. A
keyword hashes its symbol and a literal its syntax — a scalar's payload, a
container literal's parts, a record literal's fields in symbol order; a name
hashes its
resolution — a builtin by its index, a local slot by its hops and slot, a read
of the program's top level by the binding it reads, any other capture by its
hops and capture slot — and a keyworded use its key and each candidate's
coordinate by the same rule, so two bodies of one text whose uses resolve apart
digest apart. A nested shape hashes its own code digest, and spans are left
out, so the digest does not depend on where the body is written. A module's
digest then takes each `listed_top` entry its code does not already name. A
capture that reads the top level is therefore not **composed** into a closure's
or a module's content (`BodyShape::composes`): the code digest names it.

A code shape is where a [mark](#holes-and-marks) is spent, so its captures are
keyed by name **and** mark: a hole `x`, a `$x` and a `\x` read in one body are
three captures. A `$x` skips every binder in the code, the parameters of a
callable nested in it included, and at the code shape resolves outward,
unmarked, where the quote is written; its capture reads there as a callable's
does, or names a fellow member. Every other name the code does not bind stays
open: a hole is a `Hole` capture, which a `USING` fills, and a `\` mark an
`Offered` one, which the `EVAL` running the code fills. A callable nested in the
code captures each of the three through its own mark. An `EVAL` whose operand
is a parameter typed `:(<kind> NEEDING #[…])` records what it offers, keyed by
the operand's site (`BodyShape::offers`): each listed name resolved as an eager
read at the `EVAL`'s statement, and unbound there when nothing binds it.

**No reader sees a bare edge.** The scope layer is generic over the knot member
a value holds — the parameter [`values`](../values/README.md#what-a-value-is)
takes — and reads one only to resolve an edge. An activation of a callable
holds the member it runs, and a block inside it inherits that member, so a
read of a capture that is an edge resolves through it to the sibling the edge
names — a function or a data node — a bound value like any other. A capture read at birth through
such a coordinate is therefore the sibling's value word.

**Where a quantified function is read.** A name bound to a quantified
function resolves by how it is bound, read off the syntax of its declaration:

- a **call-only** name — one a `LET … = FN EXPR FOR ALL …` binds, or a
  quantified `VAL` member a `USING … SCOPE` surfaces — resolves only as the
  head of a call by name, `(pick {x = 1})`, and is refused `QuantifiedRead`
  anywhere else;
- a **member** — a `MODULE` or `GROUP` body's `LET pick = (FN FOR ALL …)` — resolves at
  the head of a call, and unmarked anywhere else, where the
  [load's walk](../dispatch/README.md#static-types) instantiates it at the type
  it is wanted at. Such a read is eager whatever its position, since making the
  instance needs the member's value. A `$pick` in a quote's code is refused
  `QuantifiedRead`, and a bare `$pick` refuses the program rather than the code;
  an `EVAL` refuses to offer either kind of name, since an offer passes the
  name's value in;
- any other binding of a quantified `FN`, a `LET` outside a `MODULE` or `GROUP`
  body included, resolves as an ordinary name: the load's walk instantiates it where
  it is bound, or refuses the load.

A quantified member read through its module, `m.pick`, is no name: the read is
an `ATTR` call, and the load's walk makes it an instance site where it sees the
member's scheme ([static types](../dispatch/README.md#static-types)).

A quantified `FN` literal is written anywhere; outside a call's head, the
load's walk instantiates it where it is written. A body whose value is read — a
callable's, a block's, a quote's code — takes its last statement's, so that
statement binds no call-only function: a `LET … = FN EXPR FOR ALL …` or a bare
`EXPR FOR ALL …` definition there is refused `QuantifiedValue`. Inside a quote's
code the refusal waits for the `EVAL` that runs it. A keyworded hole filled from a module and
the candidates an `EVAL` offers are lists, not names, so either may hold a
quantified registration; such a list is typed `List<Any>` without reading its
functions' types ([values](../values/README.md#the-type-memo-and-satisfies)),
and dispatch reads each function by its own. Every other value's type is
concrete, which is what lets a reader outside a call's head take it as one.

A search by symbol happens only where a shape is built.
How the shape's runs are searched — linear below some length, binary above —
is an implementation detail measured, not a commitment of the design.

## Visibility

A binding is visible to a reader when its declared position is strictly less
than the reader's, compared within the scope that declares it. A parameter
writes at position `0` and a body's statement `i` at `i + 1`. Where a reader
reads depends on what it does with the name.

**Eager and deferred mentions.** Every mention of a name in a binding's
right-hand side is classified by its path from the right-hand side's root. A
mention is **deferred** when that path is non-empty and every context on it,
down to the outermost callable-body boundary it crosses, is a constructor slot
(a list element, a dict value, a record field) or that boundary itself: the
value is only stored in a container or captured by a callable, never
inspected. A type declaration's definition part — a `UNION`'s variants, a `SIG` or
`NEWTYPE`'s fields — is a constructor slot too, so a type naming itself or a
later sibling in its definition is a deferred mention. So is a nominal
construction's payload: in a two-part node whose head is a type name,
`(Ring {next = a})`, the head is an eager mention and the payload a constructor
slot, so a tagged value naming itself reaches the tie rather than being an
eager cycle. A type-constructor application written in value position has the
same shape and is classified the same way; the tie, not the shape, refuses it
when it closes a cycle. A union variant's construction (`Tree.Node x`) is a
attribute form and stays eager. A callable body is opaque — nothing inside it changes the class,
since none of it runs until the callable is called. Any other mention is
**eager**: the value is needed at the point it is read. A call, a keyword
shape's slot, an operator's operand, a dict key, a type expression outside a
definition, a `MODULE` body and a `MATCH` or `TRY` arm are all eager contexts, and
so are a callable's parameter and return types and each `FOR ALL` name's bound,
which are mentions of the enclosing shape read where the callable is born. A
parenthesized group of one
part is transparent: it is the part. So in

```
LET f = FN <reads g>
LET x = [f, compute(5)]
LET y = g([f])
LET z = (FN <reads f>)()
LET a = b
```

`f`'s mention of `g` is deferred, however `g` is used inside the body, as is
`x`'s mention of `f`; `x`'s mention of `compute` is eager. `y`'s mention of
`f` is eager, because the list sits under a call. `z`'s mention of `f` is
eager although it sits in a body, because that body is called. `a`'s mention
of `b` is eager: the root itself is the mention.

An eager mention reads at its statement's position, so it sees every binding
declared before the statement and never a later one, and a binder never sees
its own right-hand side. A deferred mention reads at the body's end, so it
sees every binding the body declares, in any source order. Both positions are
fixed where the statement is written, not where a callable it births is
called. A function body therefore sees its own name, every sibling declared
after it, and everything declared before it; a forward *use* of a name is an
unbound-name error.

**Components.** With every mention resolved, the shape's bindings form a
reference graph, and the shape computes its strongly connected components, as
the [type lattice](../../lattice/src/types/identity.md#recursive-groups-identity-is-the-scc-not-the-declaration)
does for a group of recursive types. A component whose internal mentions are
all deferred is a group of values that only store and capture one another. Its
members can be born together as one [knot](../memory/README.md#the-knot) once
each member's eager mentions, which all leave the component, have evaluated:
a member's reference to a sibling or to itself is an edge, never a wait on a
pending slot. Two mutually recursive functions form such a component, and so
do a ring of containers and a container holding a callable that captures it.
A lone self-recursive function is a one-member component.

A component containing an eager mention of a fellow member has no solution:
the mentioning binding cannot finish until the member's value exists, and the
member's value cannot exist until the component is tied, which waits on the
mentioning binding. The shape rejects such a program where it is built, never
a hang. `LET a = b; LET b = a` is the smallest case, an eager mention at each
root; `LET f = FN <reads x>; LET x = f()` is the same error through a call,
and so is a function outside a module that is mutually recursive with one
inside it, since a module body is an eager context — the two belong in one
module.

**A module is therefore never in a cycle, so every module born is a one-node
knot.** Every mention reached from a module binder's root is eager whatever body
it sits in, so a component holding a module member and a fellow is an eager
cycle, refused here. A module naming *itself* is refused one step earlier: an
eager read at the binder's own position does not see that binder, so it is
`Unbound` rather than a cycle.

The shape hands the layer above each body's components — each with whether it
is `deferred_only` and whether it is `cyclic`, holding more than one member or
a member that reads itself — and the class of every mention, and
`BodyShape::form`, the builtin shape node a callable body sits in, where its
signature is read. What the layer above reads off a binder is **one record per
slot**, indexed by the slot, which the builder fills as its walk finds each fact
and the seal freezes:

- the callable body the binder births — `BodyShape::births`, set when the
  binder's right-hand side is a callable shape at its root, its shape is a
  combined one, or it is a `MODULE` or `GROUP` binder, whose module body carries
  no `form` — beside `BodyShape::birth_site`, where that body sits in the
  binder's own node, which a caller that must ask for the body by site names it
  by;
- `BodyShape::rhs`, a `LET` binder's right-hand side part, where a data member
  is read, and `BodyShape::annotation`, the type part of the
  `LET <name> <type> = <value>` writing it;
- `BodyShape::declarations`, a type binder's whole declaration node — a
  `NEWTYPE`, `UNION`, `SIG` or a `LET` of a type name — where the declaration
  door reads which declaration it is and where its declared part sits, off the
  node's own builtin shape;
- `BodyShape::registration`, the registration bound at the slot;
- the load-time facts of a type binder and a registration, which the load's
  records keep ([below](#load-time-types)).

The position a binder writes at is a read by slot too
(`BodyShape::position_of`). Every other table a shape holds by site — mentions,
nested shapes, candidate lists, `EVAL` offers and type expressions — is sparse
over its key, so it is held sorted by site and searched in one place
([`Keyed`](keyed.rs)). A caller ties a component of value binders
when it is cyclic or every member births a callable or a module; a non-cyclic
data binder is an ordinary value, and a component of type binders goes through
[the elaborator's door](../elaborate/README.md#declarations). A component never
mixes the two channels: a definition names types only, so no mention leaves a
type binder for a value binder. Tying is
[`knot`](../knot/README.md#the-tie)'s, which writes a deferred mention
below a nested constructor into the knot as an anonymous node, and a `FN`
there that captures a fellow member as a function node.

A declaration's definition part is walked under the constructor state, so every
type name it reads is a deferred mention — but a definition's own statements are
declarators with builtin shapes of their own, and a type expression written
inside one is a node with a shape of its own too: each is walked by *their*
roles rather than by structure. A `SIG`'s own `FOR ALL` group declares its head
parameters for every member, and is read where the declaration is, as a
callable's group is, so a bound in it is an eager mention; a `FOR ALL` group
inside one of its heads declares its quantifiers, bounded or not;
a manifest `LET` member declares its name, so a later `VAL` naming it is no
mention either; a union's tags name its variants and are no mentions, while
each payload quote is read as a type expression; and a parameterized
`UNION (Elem AS Option) = #{Some: Elem, …}` declares `Elem` in its declarator,
so a variant payload naming it is no mention. Every name a definition declares is the definition's own, and
the declaration door resolves it against the definition it is elaborating.

### Units

A body runs as a sequence of **units**, which `BodyShape::units` hands out in
the order they are performed. A unit is one component whose members are not all
parameters, or one statement that binds nothing; a statement belongs to its
binder's component's unit. Each unit follows every unit that binds a slot it
reads, and among the units free to go next the one written first goes first,
so independent units come out as they are written and a forward reference moves
only the binder its reader needs. A read's wait is acyclic by construction,
since a cycle among bindings is one component. Each unit records whether it
holds the body's last statement, whose value a called body's is.

Before them stands the body's **entry unit**, `UnitIndex::ENTRY`, which the
run never runs: it holds every slot bound before a statement runs — the
parameters, the body's own `FOR ALL` names, its `:Type` parameters, a block's
surfaced names, a code shape's captures — and the load's walk types it first.
`UnitIndex(i + 1)` is `units()[i]`, and two dense tables give the unit of each
slot and of each statement (`unit_of_slot`, `unit_of_statement`); `nested_in`
lists the bodies each statement births or runs inline, in program order; and
`holds` gives each unit's own entries — the slots it binds, the paths its
statements spell and their type expressions in program order — so a unit's
work walks those and never the whole body.

A statement containing `EVAL` is ordered like any other: the code an `EVAL`
runs reads no name of the body around it by symbol ([quotes](#quotes)), so its
operand is its only mention.

### The threaded set

A callable's, a code body's or a tail arm's shape seals, beside the slots its
last statement reads, its **threaded set** ([threaded.rs](shape/build/threaded.rs)): what a
tail call that statement makes hands on that the frame built itself, so the
frame builds it where it outlives the hop
([what a loop keeps](../program/README.md#frames-contracts-and-tails)).
`BodyShape::threaded` lists the binders in the backward closure over the
body's read edges from the last statement's reads, and
`BodyShape::threaded_parts` the last statement's argument parts that are no
name and no literal — a keyworded use's slots where some candidate is no
builtin, or an application's argument, each field of a record literal written
there. Both leave out what the frame **received**, which rests where it was
handed from: a type slot, a parameter, a capture, a binder bound bare to a
received value, and a member read of one through the builtin `ATTR` alone.
Every other kind of body has an empty set.

A tail arm's `it` is received where its `MATCH`'s scrutinee is a value the body
holding the `MATCH` received, decided at that body's seal; otherwise the
`MATCH` built it, so an arm handing `it` on threads it, and the body holding
the `MATCH` threads the scrutinee among its argument parts. An arm reads any
other name of the bodies around it as received: the body it lands in reads it
in its own last statement, so that body's own set holds it where it is built.

The set is static and composes without knowing the loop it runs in, which
matters because the runtime tails more than the static
[tail facts](#load-time-types) find. A member missed costs a copy at the hop,
never safety; a member that need not be one stays in the loop's cell until it
compacts.

## Three channels

A value name and a type name are different key types, so the value channel and
the type channel cannot collide by construction. A name whose text classifies
as neither is rejected where the text is classified, before any scope sees it.
The third channel holds **registrations**: the slot a keyworded definition's
function is bound to under its bucket key, a binder no text names. The
partition lives in the shape: each channel is its own run of declared binders,
sorted by symbol, and the three share one index space — value names take the
first slots, type names the slots after, and registrations the last. The
builtin table lays its names out the same way, with its overloads after them,
grouped by bucket key. An activation holds one run of slots over `Value`, since
a type and a function are `Value` arms, and the shape's key types keep the
channels' indices apart.

**Builtins are immutable and unshadowable.** A user binding whose name collides
with a builtin's, in either channel, is a rebind error at any depth, never a
shadow. Because no scope can hide a builtin, a builtin name resolves in the
shape to an index into the builtin table, and a read goes through a base
pointer to that table carried in the activation's header. An activation copies
no part of the builtin table.

## Keyworded uses

A **keyworded use** — a node holding a keyword whose key is no closed
[builtin expression shape](../parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
— resolves where its shape is built to a **candidate list**
(`BodyShape::candidates`, keyed by the node's site): the builtin overloads at
its full bucket key, in table order, then each registration at that key visible
to the use, the enclosing bodies innermost first. The key is the whole key, so a
registration at `MOVE _ TO _` is no candidate for `MOVE _`, and a lone keyword
`(NOW)` is a use of the key `NOW`. A builtin expression shape whose slots
dispatch evaluates — `ATTR`, `FROM`, `USING` — is closed and lists its
own overloads alone, and so does the `NOT` a `!=` is rewritten to. A keyworded
node inside a type expression is no use.

**A registration is read as a name is.** Each registration in a list is a
mention of its registration's symbol, classified eager or deferred as a name
mention is ([visibility](#visibility)): at a statement it reads at the
statement's position, so a definition written after it is invisible, and in a
callable body it is deferred, so the body sees its own registration and later
ones and captures them. Captures, components and units therefore treat a
registration as they treat a name, and mutually recursive definitions tie as one
knot. A use with no candidate — no builtin overload and no visible registration,
whatever declarations it sees — is refused as an unbound name is, save in a
quote's code, where it is a hole ([holes and marks](#holes-and-marks)), and in a
`USING … SCOPE` body, whose operand's registrations are
[parameters](#names-that-arrive-at-run-time).

**Every keyworded definition registers.** `EXPR`, `EXPR FOR ALL`, `OP` and
`UNARY OP` declare a registration at their statement's position, a `UNARY OP`
two — under `⊕ _` and `_ ⊕ _`. A combined `LET f = FN EXPR …` or
`LET plus = OP …` declares its name and its registration in one component over
one body, so both are born together. A definition at the key of a closed
builtin expression shape, or at a key that spells no keyword anywhere, is
refused.

**One ranking per key.** A bucket declaration, `EXPR #(MOVE 2 TO 1)`, binds
nothing: it gives its key a ranking, each slot's
[priority class](../../lattice/src/types/solving.md#priority-classes). A definition
takes the ranking of the declaration visible where it is written — visible as a
name read at its statement is — and with none, its slots' written order; an
operator takes its chaining's, fold left and pairwise ranking `left` first and
fold right `right` first. A definition's head carries no integer. Two
declarations of one ranking are one, so libraries that declare a key alike
compose; two rankings of one key that meet are refused wherever the builder sees
both — a declaration or a definition seeing another, or builtin overloads at the
key ranked otherwise — so every candidate list carries one ranking. An inner
declaration never shadows an outer definition's ranking: a use sees both scopes.
Nothing indexes rankings by key across a program, so two libraries that never
meet never conflict.

**A binder is its statement's own.** A binder or bucket declaration anywhere
but at its statement's root — `PRINT (LET doubled = 42)`,
`LET a = (LET b = 1)` — is refused; a parenthesized statement, `(LET a = 1)`,
is still its statement's own.

## Placeholders and writes

A slot is two-state: empty until its unit's turn, then bound, once. Nothing
claims a slot ahead of binding it and nothing rewrites it after, so a binding is
as immutable as the value it holds.

A slot visible to a running reader is never empty. The shape orders a body's
[units](#units) so that each follows every unit it reads, and the body runner
performs them in that order, so a deferred mention that reads at the body's end
still finds its sibling bound by the time anything reads it. A read that finds
an empty slot is a scheduler bug, and `ActivationView::read` panics on one
rather than returning a pending state nobody could act on.

A function activation and a module activation are one type with two
constructors. A module's carries no knot member — a module's captures are never
edges, since it is alone in its component — and the caller runs its body to
completion and only then ties the binder over the finished activation, reading
its slots out through `ActivationView::slots`. A module body's binders are its
exports in flight, and nothing outside the body reads them before its binder is
tied.

## Load-time types

A shape holds no fact a type decides: it is a function of the text, the
builtin table and the operator claims alone, and holds
no cell, so `BodyShape` is covariant in its lifetime and `Sync`. What
[the load's walk](../build/README.md#the-one-walk) decides rests in records
off the shape, and the facts of the text that span bodies rest in the
**shape table**.

**The shape table.** [`Shapes`](shapes.rs) holds every shape one load builds,
by segment, beside the statement of its encloser it sits in, laid once where
the root's shape is built. It answers every text fact that reads more than one
body:

- `bind_site`, where an unknown binds — its body's entry, or the unit binding
  it ([seams](../build/README.md#frontiers-and-seams));
- `encloses`, whether one body is another or encloses it, read off the
  segments: a body's nested bodies take the segments before its own, so a
  body's segment exceeds every body's it encloses, the order a frontier's
  [cut](../build/README.md#frontiers-and-seams) reads;
- which unit's record holds a fact the shape places: a slot's (`slot_unit`), a
  statement's (`running`) and a callable's or a quote's code's birth
  (`birth_unit`), an inline body's facts resting in the unit running the
  statement of its encloser that holds it;
- each mention's **landing**, its coordinate followed through every capture to
  a builtin, a binder or a quote's open capture (`Mention::lands`), beside
  whether it reads a fellow of its own component with no callable between
  (`Mention::fellow`). A capture is followed by one rule, [`follow`](shape.rs)
  over a `CaptureChain`: the builder follows it over its drafts by level, the
  table over the sealed shapes by segment (`Shapes::follow`) and the load's
  walk over its chain by level, so a `Lands` names its body by either. A
  capture's source is a builder capture's own or, past those, its layout
  entry's coordinate outside (`Shapes::capture_source`);
- the **levels** of a chain: every run-bound type name of every body along it,
  a closed type binder included, numbered as the
  [type half](../elaborate/README.md#the-type-channel-at-load) reads them —
  an encloser's names first, then a body's own `FOR ALL` names in written
  order, its other type names in slot order, and at a chain root its
  type-named captures, siblings numbering alike — each beside the slot or
  capture its activation holds the bound type at (`homes`, `level`); then
  every spelled **path identity** of the chain, its root, member chain and
  label, in first-spelling order (`BodyShape::spelled`, `path_level`), each
  part holding a path a type spells decided by its tokens
  ([`path_spelling`](spelling.rs)); a path no type spells, which a member read
  pins, takes its level past these (`path_base`) as a fact of the unit
  pinning it;
- a callable's or a `MODULE` body's **layout**: each run-bound type name of
  the bodies around it on its chain that is bound when its birth runs, by
  level, beside where the birth reads it in the activation around it, held in
  the closure past its builder's captures (`layout`, `coordinate_of`);
- the tail-call facts ([`tails`](shapes/tails.rs)): which callables
  **escape**, the parameters a body's own tail rebinds, and the parameters
  every tail edge into a body passes on bare, a greatest fixpoint;
- the **digest memos** ([`memos`](shapes/memos.rs)): the slots that keep one,
  each `:|` over a module binder read bare that reads them (`view_slots`), and
  which digested slot each call passes bare (`handoffs`).

**Records.** The walk keeps one record per unit ([`records`](records.rs)): a
`UnitFacts`, which holds one `Typed` per body the unit typed — the unit's own
body and each inline body it ran — what it births, the path entries it
resolved first on its chain, and what the folds read; one `Closing` per owning
body; and, once the walk ends, the `Folds` per body. A `Typed` holds, each
table sorted by its key:

- the type channel's facts: each type binder's declaration and each
  registration's expression shape, by slot, and each recorded **type
  expression**'s type, by site — a `:(…)` or `:{…}` in value position, a type
  part of an expression shape that births no callable — a `MATCH … UNDER`'s
  union clause, a `TRY`'s result, an ascription's type — and each
  `MATCH … WITH` type guard. A callable's signature, a declaration's definition and
  a type `LET`'s right-hand side are not recorded: they are typed with their
  callable or binder, and a type nested in a recorded one is part of it;
- the value channel's facts: a static type, an interval, for each part the
  evaluator reads as a value (by site), each statement (by index) and each
  binder (by slot); what the load fixed about each keyworded use, by the site
  of its node (`UseStatics`) — its `Narrowing`, the one candidate selected or
  each candidate kept beside its verdict, *always* or *maybe*, and its
  **contributions**, a static type per argument the call solves from,
  `Unknown` where it reads the carried type — with a use the load fixed
  nothing about left out, every candidate kept and every argument read as
  carried; each use's selection, and each kept candidate, beside the
  **points** its group solves to — closed, rigid over the run's variables, or
  an argument's carried type — and each kept list of closed candidates beside
  its **elimination table**, which pair outranks which at each class; each
  callable's tail call's **route**, per candidate whether it keeps the frame's
  contract and whether it hops into the frame's own body; each `MATCH`'s
  **arms** (`Arms`), by its node's site — the arms a run selects among beside
  their verdicts, how those that admit rank, and the arm the load chose, if
  any ([matching](../dispatch/README.md#matching)); each **retype** to a
  closed declared type, what it re-stamps a value at, an arm's `it` among them
  at the arm's site;
  the site of each **settled** `:!` or annotation, whose value's static type
  lies under its type, so the run checks nothing there; what the load fixed at
  each [instance site](../dispatch/README.md#static-types) — the solution it is
  instantiated at, closed or rigid, or, for a member read at a call's head,
  that the run reads it as it is (`InstanceRead`); each call by name's
  contributions, by its argument's site, a static type per parameter of its
  callee in symbol order; the **content tree** of the view each `:|`
  ascription at a closed signature builds, by its site, beside the overloads
  each keyworded member's covering picks where the module's content is closed,
  and each module binder's content tree, by slot; and each minting site;
- what an inline body folds into its owner's closing: its exits, its minting
  sites and a `MODULE` or `GROUP` body's exact signature.

A unit's birth record holds each callable's type, its own group's levels, the
solution a `FN FOR ALL` the load instantiated is born at, and which of its
parameters digest at a constant cost. A path entry is a plain `PathEntry` —
its root, a binder or a part written in place, its value-member chain, its
label, its level and the declared application it is read through, if any —
beside its `Resolution`: the path's content tree and what
[dispatch](../dispatch/README.md#paths) resolved it to, closed at a type or
the variable of its class. A closing holds an owning body's class, exits,
summary and recursion flag, each closed parameter's and the return's retype,
and for a quote's code the type a traced `EVAL` runs it at, the code's typing
refusal and its overlap, which an `EVAL` reports beside the builder's own
(`BodyShape::refusal`).

A load-time type is `Unknown`, a closed value, or a rigid value beside the
`Variable`s that the run substitutes: each a variable's level and its source —
a name's coordinate (`Source::Type`), or a path, by its chain root's segment and
its level there, beside the coordinate of its root (`Source::Path`), which the
layer binding the root resolves closed before the run reads it. A born
instance is a `StaticInstance`, closed or rigid, its type recorded beside its
solution, or nothing: it has no unknown arm. The vocabulary lives here, beside
the shape its records key on: `Static`, `StaticInstance`, `Variable`, `Point`,
`Kept` and `Table`, `Route`, `Arms`, and the callable-typing records the
elaborator fills, `Callable`, `Registered`, `ParameterBinding`, and
`Elaboration`, so a shape error can carry an elaboration's refusal.

**The read door.** Every reader — the evaluator, the body runner, a callable's
birth and the suites — reads facts through one set of readers
([`records`](records.rs)), each accessor taking the body's shape
(`typed_expression`, `value_type`, `use_statics`, `settled`, `instance_at`,
`named_contributions`, `view_tree`, `route`, `arms`, `retype`, `class`, `caching` and
the rest), so no reader names where a fact is kept. `LoadFacts` reads one
load's records; `Facts`, what a step's door hands it, reads a cell's layers —
its activation's decisions, the settlements it runs under, and the
[composition](composition.rs) or load they rest on — innermost first. A step
holds a `Facts` at its own borrow alone, so what it parks names a segment, a
site or a digest. Every reader whose fact names a type takes the reading
frame's chain before the shape and hands the fact back with each type it names
resolving there: over a composition, a type the chain lacks is minted from its
term, found by digest in the composition's table and then each settlement's.
The mint is private to the readers, so no run-side read can skip it; over the
load and the settlements under the root it returns at once, since every type
their facts name lies in the root or the agent tier. A walk that rests a layer
on a step's facts takes their raw layers through `Facts::whole`, which first
mints every term they hold into the walk's chain; only `LoadFacts`, whose types
all lie in those two tiers, is itself a raw source of records. A reader whose
fact names no type (`route`, `picks`, `view_slots`, `handoffs`, `caching`) takes no chain.
`at_run` reads a fact where the cell runs — closed, refused at every run, or
left open to the run's own door — and never yields a rigid one, since every
variable the load left open is bound in a layer the cell reads. A slot's, a statement's or a birth's fact
lies in the record of the unit the shape table names for it, so it reads that
record directly; it indexes only each site, through and path fact, whose unit
the shape cannot name, to the record holding it, and forwards the text facts to
the shape table. A path's entry and its resolution read in one lookup (`path`).
A `LoadFacts` may sit over a base: every read tries its own records, then its
base's, and a unit it re-ran replaces the base's record of it whole. The
load's is laid in program storage before the program is, and indexes every
closed content tree its units computed by its **identity**, the digest the run
computes for that content (`tree_of`), as a settlement's and a composition's
facts index theirs. A body the walk has not typed — a quote's
code it refused — has no static types, every candidate of every use in it is
*maybe*, and no ascription in it is settled.

**What a fact waits on.** [`facts`](facts.rs) holds the solver's
vocabulary, beside the shape whose sites, slots and segments it keys on
([the store](../build/README.md#the-store)): an `Unknown`, a name a binding
may remove, by its lexical address — the declaring body's segment and the slot
or capture its activation holds the name at; a `Frontier`, what a unit waits
on — the unknowns it gathered itself beside a `Cut` read of each ended
frontier it read whose cut keeps one of its unknowns, never laid flat and
compared only by identity; a `UnitId`, a body's segment and a unit's index in
it; a `Bound`, what a binding gives an unknown — a type, or a module's closed
content tree beside the exact type it carries; a `Binding`, unknowns sorted
beside their bounds; and the `Store` trait the walk runs over.
[`writes`](writes.rs) is the table a store keeps its writes in, one per unit,
before it lays them into a `LoadFacts`.

**Content trees.** The value channel's trees are [`tree`](tree.rs)'s
vocabulary: what the load knows of a module-valued expression's content
([the module design](../../design/modules.md#the-load-and-the-run)). A
`Tree` is a digest the load computes, a `MODULE` or `GROUP` body over the trees
of what it composes, a knot of closures, an opaque view over its operand's tree,
a retype — a knot member under a header — over its bare member's tree, or a
**leaf** for what the load cannot see: a binder and a value-member chain read
through it (`Leaf::Bound`), keyed too on which run of the binder's body holds
it — a body's builder captures, or a call the load unfolded — or an expression
no binder names (`Leaf::Unnamed`), unlike every tree, itself included. A tree
with no leaf is closed, and its digest is the run's, computed through the run's
own [recipes](../values/README.md#content-digests). `Tree::same_as` is the one
equality: closed trees by digest, any other structurally. Trees are `Copy`, and
a tree names a body, and a binder its body, by segment.

**Covariant facts.** Every record — static types, trees, path entries,
resolutions, `UnitFacts`, `Closing`, `Folds` — names a body by its segment and
a path by its chain root's segment and level, never by a reference into
program storage, and holds no cell, so each is `Copy` and covariant in its one
lifetime: a record laid in program storage reads as one laid in any
shorter-lived region. The walk lays every value a record keeps where the
record rests, through its store's `lay`, so nothing is copied out after it.

## Names that arrive at run time

Two forms introduce names no shape can see.

- **`EVAL`** runs the shape built for its code from that code alone
  ([quotes](#quotes)): a name binds to a binder in the
  code, and a `\` mark the evaluating parameter's type supplies resolves where
  the `EVAL` is written, as a coordinate like any written name. A binder in the
  evaluated code binds in that block shape and is gone when it ends; nothing an
  `EVAL` runs declares into the scope around it. No search reaches outward at
  run time, so every shape resolves through coordinates alone and keeps no link
  to its parent.
- **`USING … SCOPE`** makes the names its operand surfaces the **parameters of
  its body's block shape**, so a mention of one resolves through the ordinary
  local read, a callable nested in the block captures it the ordinary way, and
  no coordinate names a member. The keys it surfaces are parameters the same
  way, and each is a spread candidate for a keyworded use in the body. Only the
  binding is left to run time, which is
  [the module layer](../knot/module/README.md#entering-a-using--scope-block)'s:
  a surfaced key is bound to the list of every function the module offers
  there.

  The operand surfaces each keyworded head it declares under every bucket key
  the head's definition would register at, two for a `UNARY OP`: an operand
  ascribed to a signature each bodyless `EXPR` and `OP` head the signature
  declares, and a `MODULE` or `GROUP` binder each definition its body holds.
  The block holds **one registration per key**, over every head at it, laid
  out among the parameters at an index no statement takes and ranked by the
  heads' written ranks — or an operator's chaining, or the ranking a definition
  at the block would adopt — so heads that disagree at a key, or a ranking that
  disagrees with another at its key, are refused as a definition's is. It
  records each head's place (`Registration::surfaced`): a signature head's
  statement, `SIG` and naming ascription, or a body definition's statement and
  module binder, which is how
  [the load](../elaborate/README.md#the-type-channel-at-load) types it.

  That works only if the names are readable where the shape is built, so the
  builder walks the operand's spine back to a declaration that states its
  members: a `MODULE` or `GROUP` binder's body, the `SIG` an ascription at the
  site names — its head parameters and its members — a `LET` rooted at either,
  a value or type alias, and a `WITH` pin, which changes no name. The same walk reads the
  [operator groups](#operator-groups) the operand surfaces — a `GROUP` binder's
  group, or a `SIG`'s bodyless `GROUP` heads — which the body then holds, so an
  operator run of their members may be written in it. The walk is fuel-bounded,
  so an alias that names itself
  terminates, and it records no mention and pushes no capture — the operand
  itself is walked as an ordinary eager argument by the mention pass. An operand
  that says nothing statically — a parameter, which may hold a module wider than
  its signature, a call, a member read — is refused `Unsurfaced`, naming the
  ascription the site needs.

## Quotes

A quote is code as a value: its syntax, and bindings for some of its names.
Nothing in a quote resolves unless the quote says how, and the shape its code
runs in is built only where that code is built — as a callable's body, or by
`EVAL` or `BUILD`. One rule holds the pieces together: **code fills holes, frames never
do.** A binder anywhere in the code a quote is composed into binds a hole in
it, a callable's parameters among them where the code is its body; a callable's
locals and scope never reach a hole in code it receives; and a name bound with
`$` is never re-resolved.

### Holes and marks

Inside a quote, a name or a keyworded use is in one of three states:

| | a name | a keyworded use |
|---|---|---|
| a hole, unmarked | `x` | `GREET "bob"` |
| resolved where the quote is written | `$x` | `$(GREET "bob")` |
| resolved where its code is built | `\x` | `\(GREET "bob")` |

- **A hole** binds only to a binder in the same code — a `LET` before it in the
  quote, one in code composed with the quote, or a parameter of the callable
  the code is the body of — or to a builtin value or
  type, which belongs to no scope. A keyworded use that is a hole
  also has the builtin table's overloads as candidates, since they belong to no
  scope, and a registration is a candidate for it only when composed ahead of
  it or filled in by a `USING`. So `#(PRINT x)` finds `PRINT` wherever it goes,
  and `#(GREET "bob")` finds a user's `GREET` only through composition, a
  `USING` or a mark. A user's overload of a builtin's key reaches an unmarked
  use only through composition or a `USING`; one merely visible where the quote
  is written or run never does.
- **`$` resolves where the quote is written.** `$x` binds `x` to its binding
  when the quote is born; the part stays a name and compares by its binding.
  `$(…)` resolves the bucket key of the one keyworded use it wraps where the
  quote is written, to a candidate list as a written use does. It covers only
  that use: the use's arguments stay as written, each a hole or marked on its
  own. Over an operator run the [operator rewrite](#the-four-rewrites)
  expands, it covers every use the rewrite builds, a pairwise combiner
  included, so every operator in it resolves the same way; over `a != b` it
  covers the `==`, since the `NOT` around it is always the builtin's.
  `$` never evaluates. A computed value enters a quote as a bound name, and
  only so: `LET v = (…)`, then `#(… $v …)`.
- **`\` resolves where the code is built**, as the list of free names a
  syntactic closure leaves open does (Bawden and Rees). `\x` binds to the
  nearest binder in the code it is composed into, and otherwise to what the
  build supplies ([building code](#building-code)); `\(…)` covers only the one
  keyworded use it wraps. Since an overload set accumulates where a name binds
  once, `\(…)` lists the registrations at its key that its code holds where the
  use sees them beside its key offered where the code is built, as a hole lists
  them beside its key's hole: in
  `#((EXPR #(GREET y :Any) -> Any = #(y)) (PRINT \(GREET 1)))` both the code's
  `GREET` and the one the build offers are candidates, and the quote still needs
  `GREET _`. A mark keeps its `\` through every composition until something
  binds it.
- **`code USING src`** binds the holes `src` surfaces — a record's fields, a
  module's members, and a keyworded hole to the module's registrations at its
  key — and returns code with the others still holes, as
  [`USING … SCOPE`](#names-that-arrive-at-run-time) makes the names its operand
  surfaces its body's parameters. It applies as often as a program likes, so a
  template's holes can be filled in stages. A field naming no hole is ignored,
  as record width subtyping ignores a field a slot does not name, so templates
  can share one context record; a hole an earlier `USING` filled is no hole, and
  is never rebound. What it returns is composed code in which each filled hole
  is a **filled mark** in a scope of its own ([splicing](#splicing)): resolved
  through its fill as a `$` name is through its binding, so the fill survives
  any splice or slice of the code and no binder composed beside it rebinds it,
  and printed bare, as it was written. A filled keyworded use in an operator run
  lists the keys it fills, so the run's other keys stay holes. Its result is
  built, with the type of the code it filled.

Both marks work on value and type names alike, a type name's in a type
expression too: `:(LIST OF $Alias)`, or `:($Alias)` alone, since the `:` sigil
takes a type name or a group. A group mark wraps exactly one
keyworded use, so `$(y)` is not `$y`: a group holding no keyworded use gives
`$(…)` or `\(…)` nothing to resolve and is refused, and so is one wrapping a
closed builtin expression shape such as `LET`, whose bucket resolves the same
everywhere. A use of an open bucket — `PRINT`, `==`, an operator — can be
overloaded, so a mark around it stands. A mark belongs to the innermost quote
that is a value; a quote a builtin reads as written — a callable's body, a
head, an arm — is syntax of the code around it, so a `$y` in an `FN` body
inside `#(…)` is that quote's. A [pinned](#splicing) mark belongs to its code's
root wherever it sits. Neither mark has a reading outside a quote
value, where every name already resolves where it is written, and a mark there
is refused; running code is `EVAL`'s. Neither leads a line: each prefixes one
atom or glues to one group, so `$x` alone on a line is the bound name, and on a
compound atom it marks the leading name, so `$a.b` is `ATTR $a b`. Both lex as the other
[sigils](../parse/README.md#the-division-of-labour-with-sexlex) do: `\x` is one
atom, and `\(` is the atom `\` glued to its group.

### Composition

Code values joined into one piece of code, by [splicing](#splicing), make one
body whose binders bind its holes as if they had been written together. A block holding `#(LET x = 4)`
followed by `#(PRINT x)` binds the second's `x` to the first's `LET`. Each
quote is a closure over its `$` bindings, but not an opaque one. Holes fill in
either direction: a binder in a composed part fills a hole in the code around
it as readily as the code around it fills one in the part.

A part carries its bindings into every composition, so a `$` name keeps the
binding its own quote gave it whatever the code around it binds. Composition
therefore only fills holes, and cannot capture a name its writer resolved. The
one silent case is a forgotten `$`: a hole that a composed binder of the same
name fills.

### Splicing

A splice's outer sigil says when it happens. `$..xs` spreads where the quote is
written: `xs` names a list of code, and its elements become the quote's syntax
when the quote is born. `..$xs` binds `xs` where the quote is written and
spreads its value when the code runs, as `..` spreads outside a quote, and
`..\xs` spreads what the build supplies. `$` still never evaluates, since
splitting a list runs no koan code, so the operand of `$..` is a name, and each
element is code: a value element would be spliced syntax or a nested quote
according to its kind. A computed list enters by `LET` first. `\..xs` has no
reading, since a `\` name binds where a body is built, to a slot whose value
arrives with each call. A spread is one level deep, as Lisp's `,@xs` and
Julia's `$(xs...)` are, and a spliced part [composes](#composition) as any part
does. A splice stands only as a part of an expression: a list or brace literal
holds none as an element, and a type expression and a marked use hold none
anywhere under them. A splice in a group a literal's element or a
[template](../parse/README.md#templates)'s brace holds composes as one in a
paren group does, and so does one in a quote a builtin reads as written there,
such as a `MATCH` arm in `#{…}`; a quote value written there composes as its
own. Where the load can tell that
`xs` is never a list of code it refuses the program, and otherwise a list that
is not one is an error value where the quote is born.

The level a splice acts at is read from the parts around it, never from the
lines, since a block is any node of two or more groups
([code kinds](../parse/README.md#the-ast-borrowed-copy-and-splice-free)),
written on one line or several. A splice whose siblings other than splices are
all groups acts at statement level, and each element adds its statements, since
an `Expression` stands in for a `Block`. Any other acts at part level: each
element becomes one part, bare when it is a single part and a group otherwise,
as Julia's `$ex` is. With `args` bound to `[#(a + b) #(c)]`, `#(f $..args)` is
the quote of `f (a + b) c`. A quote of splices alone is at statement level, so

```koan
LET stmts = [#(LET x = 4) #(PRINT x)]
EVAL #($..stmts) -> Any
```

prints `4`. Lowering makes a layout line that is one splice atom the splice
itself ([lowering](../parse/README.md#the-division-of-labour-with-sexlex)), so
a block split across lines splices as it does on one line, while a written
`($..xs)` stays a group, as every written paren does.

Taking a fragment out of code is code's read, as it is a list's
([reads](../dispatch/README.md#what-a-node-is)): `code i` is part `i` as code, a
group part as the node it wraps and any other part as a one-part quote, so it
inverts a part-level splice; `code {from, to}` is the node of exactly those
parts; and `LENGTH code` counts its parts. A hole names no binding, so a
fragment taken away from the binder that filled it holds a hole again:
`#((LET x = 4) (PRINT x)) 1` is `PRINT x` with `x` a hole. A fragment keeps its
`$` bindings wherever it goes, even where a read cuts a quote away from the
builtin that read it as written. With `LET q = #(FN :{a :Number} -> Any = #(PRINT $x))`,
`q 5` is `#(#(PRINT $x))`: the inner quote is now a quote value, but its `$x`
still reads `q`'s binding of `x`. Such a mark is **pinned**: it belongs to the
code's root past every quote around it, and prints as written.

A node holding a splice has no expression shape until the quote is born, so the
load cannot tell whether a quote part beside the splice will be a quote value
or a builtin's body. The quote therefore binds, where it is written, every `$`
name such a part would own, as it binds its own, and reads a quote a literal
beside the splice holds the same way: with `a` bound to `[#(1)]` and `b` to
`[#(2)]`, `#(f $..a [#($..b)])` is the quote of `f 1 [#(2)]`. A part the splice
makes a builtin's body keeps those bindings; a part left a quote value binds its `$`
names where that quote is born, as any quote value does. A name nothing binds
where the quote is written is refused at load, so with no `y` in sight

```koan
LET q = #((LET y = 2) (f $..args #(PRINT $y)))
```

is refused, and the inner quote is bound outside the splicing node instead:
`#((LET y = 2) (LET p = #(PRINT $y)) (f $..args p))`. A quote value inside a
spliced element is the element's own, so its `$` names bind where it is born.
When the splice makes it a builtin's body instead, as
`#(FN :{a :Number} -> Any = $..qs)` does with `qs` bound to
`[#(#(PRINT $x))]`, that quote is never born, so nothing binds its `$x`, and an
`EVAL` of the code reports `x` unbound.

**Scopes.** Composition puts together parts whose `$` names were bound by
different quotes, so a `$` mark carries the **scope** of the quote its binding
came from, and a code value's bindings are one run keyed by scope and name.
With `mk` returning `#(PRINT $i)`, splicing `(mk 1)` and `(mk 2)` holds `$i`
twice in two scopes and prints `1` then `2`. Scopes are numbered by their first
appearance in the code's own marks, so a quote spliced alone is its written
text, and the same code value spliced twice is one scope, since its bindings
are one run. A filled mark is scoped the same way, and a pinned mark reads its
root's binding at its scope. Printing never shows a scope or a pin.

**Code views.** Spliced and sliced code is a **view**
([views](../values/README.md#views)): a balanced tree over stretches of the
syntax it came from, beside the code each stretch's bindings come from. A
splice, a read and `LENGTH` copy, renumber and digest nothing, so code grown a
statement at a time costs each step the logarithm of its size. A view is
**unbuilt**: its type is `:(UNBUILT <kind>)`
([the code family](../../lattice/src/types/vocabulary.md#the-code-family)),
its kind read off its parts, needing whatever it needs. Only the shape builder
can say which `\` names the code's own binders fill, and a view runs it only
when it is [built](#building-code).

### Building code

A shape is built from code in two ways, and each fills different names: as a
callable's body, and alone, by `EVAL` or `BUILD`.

- **A callable's body.** A body written in place resolves the names written in
  it where it is written, which is where it is built: in
  `FN :{name :Str} -> Str = #(GREET name)`, `name` is the parameter and `GREET`
  a candidate list read in the callable's scope, as in any body. That is the
  grant a defining expression gives the quote written as its body, and it
  reaches only the names written in that quote.

  A body may instead be any part evaluated to code — a name, a member read, a
  call — read where the callable is born, as an argument is. Its code is
  composed with the callable: the code's own binders bind first; the
  callable's parameters, its `FOR ALL` names and an operator's operands among
  them, fill its holes and its `\` marks; the scope the callable is written in
  fills its `\` marks and never a hole; and a name left over is refused as
  [unbound](#errors) where the body is built. With `k` bound beside them,

  ```koan
  LET body = #(w * \k)
  EXPR #(SCALE w :Number) -> Number = body
  ```

  binds `w` to the parameter and `\k` to that `k`. The body is read eagerly, so
  code that names, through `$`, the callable it becomes the body of is an
  [eager cycle](#errors), while a `\` name reaches the callable through its
  scope, `\(COUNTDOWN it)`, as a written body's free name does.

  - **Traced code.** Where the body is a lone name the load traces to a
    written quote holding no splice — a name a `LET` binds to one, traced as
    an `EVAL`'s operand is — the body's shape is built where the program
    loads, over the quote's statements, keyed at the body part as a written
    body's is. Its `$` names resolve where the quote is written, as the quote's
    own do, and become the callable's captures; it is typed, and its refusals
    are the program's. The quote value keeps its own shape, carried type and
    needs: two shapes over one text, and each declaration naming the quote
    builds its own.
  - **Untraced code.** Any other body — a parameter's code, code computed or
    composed, a member read, a call — has a shape at the body part holding no
    statements, which types and registers the callable from its signature. Its
    code is read where the callable is born, an error there if it is no code,
    and built at the first call as composed code whose parameters are the
    callable's: found by a digest of its syntax and those parameter names as
    `BUILD` finds a composition, so a callable with no parameters shares
    `BUILD`'s. The call reports the composition's refusal and any name left
    unbound. Nothing searches the declaring scope at run time, so that scope
    fills only the `\` names and keys a code parameter's type lists, as an
    `EVAL` of the parameter offers them ([code parameters](#code-parameters)).
    The parameters are of unknown type inside the composition; the call checks
    the arguments and the result against the signature, as every call does.
- **`EVAL`.** `EVAL code -> <Type>` runs a shape built from the code alone. The shape
  depends on nothing else, since a `\` mark is filled by what the `EVAL`
  offers, so a written quote's is built once, where the program loads, and every
  `EVAL` of that quote runs it; code composed at run time has its own in a
  composition, found by the code's digest wherever it runs
  ([the build module](../build/README.md#where-facts-are-decided)). The
  build refuses nothing where the program
  loads: an error in it is kept and reported by the `EVAL` that runs it, so a
  quote is checked only where its code runs. The frame it runs in fills no hole, so a name left unbound is an
  unbound-name error. No shape keeps a defining scope for an `EVAL` to search,
  so an `EVAL` takes no hold on a frame and waits on no binder declared before
  it: its operand is an eager mention like any other. Its `\` marks are filled
  only through a [code parameter](#code-parameters)'s type, and any other mark
  left is unbound. An `EVAL` of an unbuilt view lays it flat for that `EVAL`
  alone.
- **`BUILD`.** `BUILD code` lays a view flat once — its syntax in the value's
  region, its `$` scopes numbered, one binding run — finds or builds its
  composition by the syntax digest, and answers the code at its exact type, its
  kind needing the `\` names its own binders leave open. Written code is built
  where the program loads, so `BUILD` of it, or of code already built, answers
  it unchanged. `BUILD` refuses nothing: code that does not type keeps its
  refusal, and the `EVAL` that runs it reports it. A program that runs composed
  code many times builds it once, and every `EVAL` after finds its composition.

```koan
EXPR #(GREET who :Str) -> Str = #(PRINT who)
EXPR #(TWICE body :Expression) -> Any = #(
  EVAL body -> Any
  EVAL body -> Any
)
TWICE #($(GREET "bob"))
```

prints `bob` twice: `$(…)` resolves `GREET` where the quote is written, at the
caller. `TWICE #(GREET "bob")` is an unbound-name error at the first `EVAL`, although
`TWICE`'s own scope sees the same `GREET`, because `TWICE`'s frame fills no
hole in code it receives. So is `TWICE #(PRINT x)`, whatever `TWICE`'s scope or
parameters declare, while `TWICE #(PRINT $x)` prints the caller's `x`. A
binder in `TWICE`'s body never binds a name in `body`.

### Code parameters

A parameter that takes code states the kind of code it takes and the names and
bucket keys that code may need — a key stored as a symbol and written with `_`
for each slot, `:(Block NEEDING #[(LOG _)])`, since a slot's name is invisible
to dispatch — spelled from the code's side: when code is
composed, its holes are its inputs and its binders its outputs. Offering them
is the consent of the side that builds, as `\` is the consent of the side that
writes. A quote's carried type is its code kind and the `\` marks no binder in
its own code fills, so `#((LET x = 5) (PRINT (x + \y)))` is a
`:(Block NEEDING #[y])`, and a quote needing nothing is its bare kind. `:Code`
admits every quote. Dispatch reads a carried type as it reads any other, so a
quote needing a name the parameter does not list is a non-match that falls
through. An `EVAL` of the parameter resolves the supplied names and keys where
the `EVAL` is written, statically, as it resolves a name written there, and a
callable whose body is the parameter resolves them where the callable is
written ([building code](#building-code)), so nothing is searched at run time. Spliced and sliced code is unbuilt, and an
unbuilt type lists no names, so a `NEEDING` parameter admits such code only
once [`BUILD`](#building-code) recomputes its type: a binder composed ahead of
`\it` binds it, and `it` leaves the list. A parameter typed `:(UNBUILT Block)`
admits every block, built or not, and an `EVAL` of it offers nothing.
Haskell's implicit parameters are the same consent by name: `?x` in a type,
supplied where the value is used.

A quote's carried type says nothing of the value its code returns, and nothing
of its names' types, so a parameter cannot select on either. The types are
those of the callee's own bindings, checked where the code is built.

### Quotes and functions

A function is built code and a quote is unbuilt. A function is opaque, is
checked where it is written, declares its parameters' types and its return
type, and captures every free name of its body where it is written. A quote can
be inspected and composed, is checked only where its code is built, and
captures only its `$` names. Code becomes a callable as the body of an `FN`:
the `FN`'s parameters fill the code's holes and `\` marks, the scope it is
written in fills the code's other `\` marks ([building code](#building-code)),
and its declared return type
is the annotation the code cannot carry.

An `FN` written in a quote has one shape, built with the quote's code, and
whatever differs between two code values of that quote — its `$` bindings, its
`USING` supplies, the names an `EVAL` offers — reaches the function as its
captures, so function equality counts it.

### Equality and knots

Code compares as a bisimulation, as circular data does
([equality](../values/README.md#equality-and-rendering)): syntax part by part
with spans ignored, a bound name followed through its binding under the
coinductive pair set, a hole by its symbol, and a `\` mark by its symbol and
its mark. A quote equals its copy, and two quotes of the same text whose `$`
names bind different values are unequal. A function reached through a binding
compares by its body's code digest, its type and its captures, as
[knots](../knot/README.md#equality-and-rendering)
say. Printing code never follows a binding, and prints each mark as written, so
printed code reads back with the same holes and marks.

A `$` name naming a fellow knot member is a deferred mention, so a quote
holding one is born in that member's knot as a function node is:
`LET echo = #(PRINT $echo)` is a one-node knot, and it equals its copy. A hole
or a `\` mark names no binding and adds no edge. A quote holding a splice is
composed when it is born, from values it reads there, so it is never a knot
node: one whose `$` name names a fellow member is refused at load, and a splice
operand naming one is an eager cycle. A fragment of a knot's quote resolves each
edge to its member's value word, so no code outside a knot holds an edge.

Equality reads a mark's scope and pin as syntax, so code spliced from two quotes binding
`$i` alike is unequal to one quote holding `$i` twice: an over-distinction, which
keys two equal things apart and is sound.

## Operator groups

An **operator run** is a slot-led node whose keywords alternate with slots, two
or more of them — `1 + 2 - 3`, `a < b <= c`, `A | B | C`. `a + b` is a plain
call and is never one. An operator run is rewritten **once**, here, where the
body shape holding it is built, into ordinary nodes built through
[`parse`](../parse/README.md)'s own node constructor. Nothing past this builder
ever meets an operator run: dispatch, the scheduler and the elaborator read the
rewritten nodes and know no operator group.

An **operator group** is a set of operator symbols under one `ReductionMode` —
fold-left, fold-right, unary, or pairwise with a combiner symbol and the
direction its pair results fold in. Its identity is its content: two
declarations of an equal member set under an equal mode are one group, so a
functor's `GROUP` is one group however often it is instantiated. The record is
the lattice's [`DeclaredGroup`](../../lattice/src/types/schema.rs), so a signature's
operator channel and a body's held group are the same type and compare with
`==`. [groups.rs](groups.rs) holds the model; [shape/build/rewrite.rs](shape/build/rewrite.rs)
holds the rewrite.

### How a symbol chains, and where a run may say so

Two questions, answered by two different things. **Claims decide *how*; frames
decide *where*.**

A **claim** is what the program's declarations say about a symbol, collected by
one position-blind pre-scan of all the code being built, before the first draft.
A symbol therefore chains one way for the whole program, wherever its
declarations sit, in this order:

1. a **builtin group** covering it — `{< <= > >=}` pairwise through `AND`
   folding left, `{+ -}` fold-left, `{* /}` fold-left, `{|}` unary, and `{&}`
   unary — the meet type's run, beside the union's. Nothing
   overrides one, and they are seen everywhere;
2. the claim a `GROUP` statement makes over it, or the `Unary` mark a
   `UNARY OP` makes. A bare `OP` declares an overload and claims nothing;
3. nothing: the symbol chains fold-left, alone.

A second `GROUP` over a claimed symbol is admitted only when its group is
*equal*, and is then the same record; a `GROUP` written out equal to a builtin
group says what the language already says and claims nothing. Any other overlap
— with a builtin group, with another statement's group, with a unary mark, in
either order — is `RedeclaresGroup`. The scan enters exactly the quotes the
builder reads where they are written — a callable's body, a head, an arm, a
signature's members — since those are this program's code; any other quote is
a quote value, whose code collects claims of its own, chained to the
program's, where its [code shape is built](#evaluated-code).

A **group frame** decides where an operator run may chain under a declared
group. Only two kinds of body hold a group, both the way a parameter is held, so
no position is ever compared:

- a **`GROUP`'s own body** holds the group it declares;
- a **`USING … SCOPE` body** holds the groups its operand surfaces, read off the
  same declaration [its names are](#names-that-arrive-at-run-time) — a `GROUP`
  binder's claimed record, or the bodyless `GROUP` heads of the `SIG` an
  ascription names.

Frames nest outward from the body holding the run. A surfaced group equal to one
already held, or to a builtin group, changes nothing and is dropped; one that
would give a member a second chaining is `RedeclaresGroup`. An operator run over
a *claimed* symbol no enclosing frame holds is `Unchained` — refused, rather
than quietly folded left, because its group exists and this is not where it was
surfaced.

Each symbol of a run resolves to a **cover** this way, and every symbol of one
run must agree, else `MixedGroups`: `a + b * c` has no one shape and must be
parenthesized. `==` and `!=` are covered by nothing — they take `Any`, belong to
no group, and are refused as members of one — so they join whichever pairwise
group the rest of the run chains under, and a run of them alone folds pairwise
through `AND`, left. Beside symbols that chain any other way they are
`MixedGroups`.

The frame is also what admits a binary `OP` stating a **result type of its own**:
only where its symbol chains pairwise, a builtin pairwise group included, since
a fold hands its own result back as the next operand. Elsewhere it is
`ResultOutsidePairwise`.

### The four rewrites

Over operands `o0 … on` and operators `k1 … kn`, each operand already rewritten:

- **fold left** — `(((o0 k1 o1) k2 o2) …)`, one nested binary keyworded node per
  operator; **fold right** — `(o0 k1 (o1 k2 (…)))`;
- **unary** — `k1 [o0 … on]`, one keyword-first call over a list literal. This
  is the form a union and a meet type take: `A | B | C` elaborates as that call,
  and only `A | B` is read as an infix pair. Koan has no precedence, so
  `A | B & C` is `MixedGroups`. An operator run inside a quote the builder
  reads — a head, a type guard, a union's payload, a `FOR ALL` bound, a
  signature's member — is rewritten too, and the quote rebuilt around it;
- **pairwise** — the adjacent pairs `o(i-1) ki oi`, folded through the group's
  combiner written infix, in the group's direction.

A pairwise run names each interior operand twice, so an operand that is not a
name, a type, a literal or a quote would evaluate twice. It is **hoisted**, in
source order, into an anonymous slot — a name spelled with a space, so no source
text can reach it — of a **synthesized block**, whose last statement is the
folded result. That block is an ordinary block shape held by an `Expression`
part, which is the one rule it asks of the evaluator: *an `Expression` part
carrying a nested block shape, which is not a form's body or arm, runs as a
block whose value is its last statement's.* When a whole statement is such a
run, the statement is the one-part wrapper around its block.

`!=` is never built. Wherever a pair or a bare infix node spells `a != b`, the
rewrite emits `NOT (a == b)` instead, so `!=` reaches no bucket and is the
opposite of `==` by construction — which is also why a declaration naming it is
`Derived`, and why a user's `==` must return `Bool`. Every node the rewrite
builds, bar the synthesized hoists, must spell no builtin bucket: an operator
whose chained node a later reader would walk as a form is `SpellsForm`.

Every node and part the rewrite builds carries a span of the source it was
built from, so an error in rewritten code points into the program as written: a
chained node spans its operands, a keyword the operator it came from, a hoist
its operand, and the synthesized block and its last statement the whole run.

A statement holding no operator run is returned unchanged, keeping its own part
addresses, so the shape of untouched code is the shape of the parse.
The shape table records, for each node the rewrite builds, the written node it
stands for (`Shapes::origin`): the node it rebuilt over a rewritten part, or
the operator run a chained node was built for. So a site a shape's statements
hold is found in the syntax as written, which is where a `USING` writes its
filled marks ([filled.rs](filled.rs)): every operand of one run resolves
alike, so a name hole fills each operand naming it, and a key hole fills the
run, its other keys left holes.

A run's length counts toward the
[syntax depth limit](../parse/README.md#the-syntax-depth-limit): the parse
counts each run as the most nesting any of these rewrites builds from it, so no
rewrite deepens a statement past the depth the parse stored for it, and the
parse's check bounds every walk over the rewritten body.

### Evaluated code

A quote's code is rewritten where its code shape is built. Its claims chain to
the program's, so a `GROUP` inside it is held to the program's declarations and
chains that code's runs only, and its frame holds the builtin groups and the
groups the code holds itself — none around the quote or the `EVAL` that runs
it. A group frame is a frame, and frames fill nothing in code; a hole could not
see a user's operator overloads anyway.

## Errors

A shape that cannot be built is refused where it is built, with the first
error in walk order — save a quote value's code shape, which keeps its error as
its refusal (`BodyShape::refusal`) for the `EVAL` that runs it, and refuses the
program only for a `$` name nothing binds where the quote is written:

- a **rebind** — a name declared twice in one shape, parameters included;
- a binding that **shadows a builtin**, in either channel;
- an **unbound** name — no binding of it visible where the mention reads, or
  a hole or `\` mark left unbound in code a callable's body is built from
  ([building code](#building-code));
- an **eager cycle** — a component with an eager mention of a fellow member;
- a **mark outside a quote** — a `$`, a `\` or a `$..` splice no quote value
  holds;
- a **splice never code** — a `$..` operand whose static type can never be a
  list of code ([splicing](#splicing));
- a **splice in a knot** — a quote holding a splice whose `$` name names a
  fellow member of its binder's knot ([equality and knots](#equality-and-knots));
- an **unsurfaced** `USING` — an operand that does not say, where the shape is
  built, which names it surfaces;
- an **unlisted** read — a `MODULE` or `GROUP` body reading an outer name its
  `OVER` list leaves out ([resolution](#resolution));
- an **unsupported** form — `CLOSE` and `CLOSE OVER`, whose resolution has no
  rewrite home yet, and the reserved forms that exist only to diagnose a miss,
  `TYPE` among them, since a signature hides a type through a head parameter;
- a **quantified read** — a call-only name read anywhere but the head of a
  call, or a module's quantified member read through `$` or offered by an
  `EVAL` ([resolution](#resolution));
- a **quantified value** — the last statement of a callable's, a block's or a
  quote's body binding a quantified function by a keyworded form, which would
  be the body's value ([resolution](#resolution));
- an **unquoted** part — one its role reads as a quote or a container of
  quotes, written otherwise: a bare `EXPR` head, a bare arm set, a bare `SIG`
  body. The message says how the part is written;
- a **body never code** — a callable's body part, not written as a quote,
  whose static type can never be code, `FN :{} -> Number = 5`, or a block the
  pairwise rewrite built, whose value is a `Bool`. The message says to write
  the body as a quote or to name code;
- a **parameter outside its body** — a callable's body part, not written as a
  quote, reading a parameter of that callable, which only its body sees,
  `FN :{x :Number} -> Number = (x + 1)`. The message says to write the body as
  a quote;
- an **inadmissible** part — one read as written whose syntax fills none of its
  slot's types: a value guard such as `#{1: (a)}` as a
  `MATCH … UNDER`'s arms, `#[(PRINT 1)]` as a signature's members, `#{}` as a
  union's variants;
- **not a guard** — an arm set's key that is a nested quote, or a marked name
  as a label;
- a **repeated guard** written as syntax — a label, a value guard or `_`
  guarding two arms of one arm set;
- a **lone variant** — a `UNION` declaring one variant, whose message points at
  `NEWTYPE`;
- a **malformed** form — a quote where bare syntax is read (`LET #(x) = 1`,
  `MODULE m = #(…)`, `CATCH #(x)`), or a body that is not the shape its form
  declares;
- a **dict default** — a value dict holding a `_` key, refused until its default
  has a reading (see [Open work](#open-work)); an arm set's `_` is its default
  arm;

six from [keyworded uses](#keyworded-uses), each naming the key it is about
where it has one:

- a **closed bucket** — a definition or bucket declaration at the key of a
  closed builtin expression shape;
- **no keyword** — a definition whose key spells no keyword;
- a **ranked definition** — a definition head writing an integer in a slot;
- a **nested binder** — a binder or bucket declaration that is not its
  statement's own expression;
- a **ranking disagreement** — two rankings of one key that meet;
- **no candidate** — a keyworded use with no builtin overload and no visible
  registration;

six [dispatch](../dispatch/README.md#the-overlap-check) finds once the shape is
built and its types can be read — an **overlap**, a user overload taking
operands a builtin overload at its key already takes; **no admitting
candidate**, a keyworded use every candidate of which
[static selection](../dispatch/README.md#static-types) drops, naming the key and
its arguments' static types; an **ambiguity**, a keyworded use every candidate
of which always admits and none of which ranks first, naming the same; and a
**return never satisfied**, a callable body whose static type meets its declared
return at `Never` — beside the refusals of its
[instance sites](../dispatch/README.md#static-types), an annotated binder, an
ascription or a call by name that can never be satisfied, a member read of a
signature lacking the member (**no member**) or of a quantified member over an
unpinned head parameter (**unpinned member**), a type capture a module's `OVER`
list leaves out, and the load walk's other checks; **no admitting arm**, a
`MATCH` every arm of which the load drops, and **ambiguous arms**, a `MATCH`
whose arms all always admit with none ranking first, each naming the
scrutinee's static type ([matching](../dispatch/README.md#matching));

five the [walk's type half](../elaborate/README.md#the-type-channel-at-load)
finds — a **type** that does not elaborate, carrying the elaborator's refusal;
a **repeated guard**, two guards of one `MATCH … WITH` arm set that type to
one handle, naming both; and, where a `MATCH … UNDER`'s clause closes
([a union's variants](../elaborate/README.md#a-unions-variants)), a clause
that is **not a union**, a label naming **no variant** of it, and a variant
**uncovered** by any arm where no `_` arm is written;

and six more from [operator groups](#operator-groups), each naming the symbol it
is about:

- **unchained** — an operator run over a symbol whose group no enclosing body
  holds;
- **mixed groups** — an operator run whose symbols chain under two different
  groups;
- **redeclares group** — a `GROUP`, or a `USING` surfacing one, that would give
  a symbol a second chaining;
- **result outside pairwise** — a binary `OP` stating a result of its own whose
  symbol does not chain pairwise;
- **spells form** — an operator run whose chained node would spell a builtin
  bucket;
- **derived** — a declaration naming `!=`, which is always the opposite of `==`.

Every error carries the [`SourceRef`](../source.rs) it was found at, and
renders led by it as `path:line:col`. That is the part the error is about when
the part carries a span — the unbound name, the operator, the inadmissible
part — else the nearest spanned part or node around it, so an error about a
list's item points at the list. A node-level error — an unsupported or
malformed form, a group claim — points at the node. A rebind points at the
second binding and names the first. A parameter is bound where the node
declaring it is written: the `FN`, `EXPR` or `OP`, the `MATCH` or `TRY` whose
arm binds `it`, the `USING` that surfaces it. An eager cycle is found at the
statement of the member declared first. The walk records a part by its address, so a part's location is
found by searching its statement, on the error path only
([build/locate.rs](shape/build/locate.rs)): a shape that builds locates nothing.

Each renders with the names a user needs spelled through the symbol interner,
and an inadmissible part's slot type through the type registry's renderer —
`:(LIST OF Declaration)` for a signature's members. A refused selection and a
refused view are worded by `selection_refused` and `view_refused`
([shape.rs](shape.rs)), which [dispatch](../dispatch/README.md#errors)'s run
faults call too, so the load and the run say one thing.

## Memory

An activation is `Drop`-free and laid down in its frame's region, so a frame's
death releases it with the region. It is its body's segment beside a header
of four pointers — its closure bindings, the builtin table, for a block the
enclosing activation, and what its frame has decided in its own activation so
far — the callable it runs, if any, one slot array over the shape's slot
count, and one digest memo, a `Cell` empty until a `:|` or a call's hand-off
fills it, per slot the load marks digested. It names no shape and no facts: a
step reads both through its door by the segment, at its own borrow, so the
activation can park across steps. It holds no pointer into itself: its
decisions lie in its frame's region, its builtin table
outlives every frame, its closure bindings live in the callable value, which
the caller keeps alive across the call, an enclosing activation lives in the
same frame, and its slots hold values. An activation is therefore copied by
copying its bytes. Each kind of body has its own constructor — a program's
with neither closure bindings nor an enclosing activation, a callable's with
the callable and its closure bindings, a module's or a quote's code's with its
closure bindings alone, a block's beside an enclosing activation whose builtin
table and callable it shares — so no other combination can be built.

A shape and everything it holds — declared-name runs, mentions, captures,
components, nested shapes — rest in program storage and are `Copy`. A shape
holds no cell, so it is covariant in `'graph` and `Sync`; the
[load-time facts](#load-time-types) rest in records beside it. A written
quote's code shape is built into program storage once, where the program
loads; composed code's, in its [composition](composition.rs)'s own storage,
over a copy of the code's syntax. A body's **sites** number each part and
expression node its statements hold, at any depth, in program order
([`sites`](shape/sites.rs)): an index per shape, plain data a parked state
holds and resolves back to its part through the shape, and never compared
across shapes.

What a koan graph keeps per habitat half is `scope`'s record, the
[region header](header.rs) (`RegionHeader`, whose record is a `HalfHeader`):
the half's type branch, and the [decision memo](memo.rs) of the cells whose
results are built in the half beside how many entries it has laid.
`values` reads the branch alone, through the projection `RegionHeader`
defines, and the record is laid by the first door that interns into the half
or lays a memo, so a half that needs neither stays untouched. What a step
reads of shapes and facts it reads through its door, never through a header
([the build module](../build/README.md#references-between-frames-and-the-arena)).

## The import rule

`scope` names `crate::values`, `crate::type_lattice`, `crate::symbols`,
`crate::memory`, `crate::parse` and `crate::source`, and no scheduler type. From `type_lattice` it names the
operator-group vocabulary — `DeclaredGroup` and its `ReductionMode` — so a
signature's operator channel and a body's held group are one record rather than
two that must be kept in step. The scheduler reaches scopes through its
embedder, save the region header it names, and scopes never reach the
scheduler. The compiler
cannot hold a module to that, so [`tests::boundary`](tests/boundary.rs) reads
the module's source and fails on any other `crate::` path, on an owning heap
type outside the one error that lists names, and on a retired lifetime name.

## Open work

- [Dict defaults](../../roadmap/rewrite/dict-defaults.md) — a value dict's `_`
  default, which lifts the dict-default refusal.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — `CLOSE
  OVER`, and a warning for an unmarked keyworded use in a quote.
