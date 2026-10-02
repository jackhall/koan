# Scopes

Koan's lexical environments: the layer that answers what a name means at the
point it is read, over the values in [`values`](../values/README.md) and the
types in [`type_lattice`](../type_lattice/README.md).

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
  gives them.

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
  closure binding is never a placeholder.
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
  header pointers, the callable, and a read view of the slots, with no door that
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
a callable's body and an `EXPR` head are a quote; an arm set, a union's
variants, a `FOR ALL` group and a `SIG` body are a list or dict of quotes; a
binder name, an in-place body, a type expression and a `TRY` or `CATCH` operand
are bare; and each admits one of its slot's code types. A type expression's and
an in-place operand's slot type is the value it denotes, so only their spelling
is checked. One static check does it, against the table's own types through
[`admits_part`](../values/admission.rs) over the program's registry — the one
rule a raw part is admitted by anywhere — so no position list and no second
admission rule sits beside the table; the readers after it assume a well-formed
part. A part no slot type admits is `Inadmissible`, which names the slot's type
through the lattice's renderer. A callable's body shape is built over its quote's body and keyed by the
quote part, so `Site::of_body` finds it as it finds any body. An `EXPR` head's
names are read through its quote. An arm set is a dict of guard quotes to arm
quotes, and each arm is a block shape binding `it`. A `MATCH … WITH` guard is a
type — a type name, a `:(…)` or a `:{…}`, typed `Dict(TypeCode, Block)`, whose
names are mentions of the shape holding the `MATCH`, typed where the program
loads ([load-time types](#load-time-types)) — and a `MATCH … OVER` or `TRY`
guard a label, typed `Dict(Name, Block)`; a value guard is `Inadmissible`. A
`SIG` body's members and a bodyless `GROUP`'s heads are the statements of a
list's quotes, each walked by its own builtin shape's roles.

**An arm knows it is one.** An arm's block shape carries an `Arm`
(`BodyShape::arm`): its guard as written — the key quote, a type under
`MATCH … WITH` and a label under `MATCH … OVER` or `TRY` — or none for the `_`
default arm, and whether the arm's last statement is in tail position. It is
exactly where the `MATCH` or `TRY` is: at the root of a body's last statement,
binding nothing, in a tail body. A callable's body is a tail body and an arm's
is when its own tail is; the program, a module, a `USING` body and a
synthesized block are not.

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
for each lexical variable of an enclosing body that a call in it solves from or
an instance site in it is made at: the static pass adds one where a
[contribution](../dispatch/README.md#static-types) or an instance's solution
names such a variable, with
the coordinate of the enclosing activation its birth reads
(`BodyShape::type_captures`), and a callable between the two passes it on
through a type capture of its own. So a body reads a type name it never writes,
and a closure holds one only where a call or an instance site in it reads it. Births read the
builder's captures, then the type captures, so the closure is `capture_count()`
slots long. A type capture is never merged with a builder capture of the same
name.

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
  [static pass](../dispatch/README.md#static-types) instantiates it at the type
  it is wanted at. Such a read is eager whatever its position, since making the
  instance needs the member's value. A `$pick` in a quote's code is refused
  `QuantifiedRead`, and a bare `$pick` refuses the program rather than the code;
  an `EVAL` refuses to offer either kind of name, since an offer passes the
  name's value in;
- any other binding of a quantified `FN`, a `LET` outside a `MODULE` or `GROUP`
  body included, resolves as an ordinary name: the static pass instantiates it where
  it is bound, or refuses the load.

A quantified `FN` literal is written anywhere; outside a call's head, the
static pass instantiates it where it is written. A body whose value is read — a
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
the [type lattice](../type_lattice/identity.md#recursive-groups-identity-is-the-scc-not-the-declaration)
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
a member that reads itself — and the class of every mention, and four facts the
layer above reads off a binder's slot: the callable body each binder births —
`BodyShape::births`, set when the binder's right-hand side is a callable shape
at its root, its shape is a combined one, or it is a `MODULE` or `GROUP` binder,
whose module body carries no `form` — beside it `BodyShape::birth_site`, where
that body sits in the binder's own node, which a caller that must ask for the
body by site names it by — `BodyShape::form`, the builtin
shape node a callable body sits in, where its signature is read,
`BodyShape::rhs`, each `LET` binder's right-hand side part, where a data member
is read, and `BodyShape::declarations`, each type binder's whole declaration
node — a `NEWTYPE`, `UNION`, `SIG` or a `LET` of a type name — where the
declaration door reads which declaration it is and where its declared part sits,
off the node's own builtin shape. A caller ties a component of value binders
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

A statement containing `EVAL` is ordered like any other: the code an `EVAL`
runs reads no name of the body around it by symbol ([quotes](#quotes)), so its
operand is its only mention.

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
[priority class](../type_lattice/solving.md#priority-classes). A definition
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

A shape carries a **write-once cell** for every type fact
[the elaborator's load pass](../elaborate/README.md#the-type-channel-at-load)
fixes before the program runs. The builder lays each cell down empty — `scope`
sits below `elaborate`, and a shape seals before anything in it can be
elaborated — and the pass fills it, once, where the program loads:

- each **type expression** the shape records (`BodyShape::type_expressions`, by
  site): a `:(…)` or `:{…}` in value position, a type part of an expression
  shape that births no callable — a `MATCH`'s result and union clause, a
  `TRY`'s result, an ascription's type — and each `MATCH … WITH` guard,
  with its arm set and written index. A callable's signature, a declaration's definition and a type
  `LET`'s right-hand side are not recorded: they are typed with their callable or
  binder, and a type nested in a recorded one is part of it;
- each **type binder**, beside its declaration node;
- each **registration**, its expression shape;
- a callable body's own **callable type**, the lexical variable each name of
  its own `FOR ALL` group is in the body (`BodyShape::group_levels`), and the
  solution a `FN FOR ALL` the load instantiated is born at, closed or rigid
  (`BodyShape::born_instance`), written by dispatch's static pass;
- each lexical variable a body declares, by level, beside the slot or capture
  its activation holds the bound type at (`BodyShape::declared_variables`),
  which the static pass reads to find where a use reads a variable, and the
  type captures that pass adds (`BodyShape::type_captures`);
- a quote's code shape's **typing refusal**, which `BodyShape::refusal` reports
  as it reports the code's own error.

A cell holds `Unknown`, a closed value, or a rigid value beside the
`Variable`s — a variable's level and the coordinate its value is
read at — that the run substitutes. The vocabulary lives here, beside the shape
that holds it: `Static`, `Variable`, and the callable-typing records the
elaborator fills, `Callable`, `Registered`, `ParameterBinding`, and
`Elaboration`, so a shape error can carry an elaboration's refusal. A reader
holding the shape reads its cell by site or slot: the evaluator, the body runner,
a callable's birth and the overlap check.

One more cell per shape holds the **value channel**: the `Statics` dispatch's
[load pass](../dispatch/README.md#static-types) fixes once the type channel has —
a static type, an interval, for each part the evaluator reads as a value (by
site), each statement (by index) and each slot (by index), and one `Narrowing` per keyworded
use, parallel to its candidate list: the one candidate selected, or each
candidate kept beside its verdict, *always* or *maybe*; the site of each
**settled** `:!` or annotation, whose value's static type lies under its type,
so the run checks nothing there; the solution each name read at an
[instance site](../dispatch/README.md#static-types) is instantiated at, closed
or rigid; each
keyworded use's **contributions**, parallel to its candidate list, a static type
per argument the call solves from, `Unknown` where it reads the carried type;
and each call by name's contributions, by its argument's site, a static type
per parameter by name. It lives here for the same reason the type channel's
cells do, and the shape reads it by site (`value_type`, `narrowing`, `settled`,
`instance_at`, `contributions`, `named_contributions`), statement
index or slot; a shape the
pass has not fixed — a quote's code it refused — has no static types, every
candidate of every use in it is *maybe*, and no ascription in it is settled.

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
  no coordinate names a member. The registrations it surfaces are parameters
  the same way, and candidates for a keyworded use in the body. Only the binding is left to run time, which is
  [the module layer](../knot/module/README.md#entering-a-using--scope-block)'s.

  An operand ascribed to a signature surfaces each bodyless `EXPR` and `OP`
  head the signature declares as such a registration — one per bucket key the
  head's definition would register at, two for a `UNARY OP` — laid out among
  the parameters at an index no statement takes and ranked by the head's own
  written ranks, or an operator's chaining, so a ranking that disagrees with
  another at its key is refused as a definition's is. Each records where its
  head, its `SIG` and the ascription naming it are written
  (`Registration::surfaced`), which is how
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
`EVAL`. One rule holds the pieces together: **code fills holes, frames never
do.** A binder anywhere in the code a quote is composed into binds a hole in
it; a callable's parameters, locals and scope never reach code it merely
receives; and a name bound with `$` is never re-resolved.

### Holes and marks

Inside a quote, a name or a keyworded use is in one of three states:

| | a name | a keyworded use |
|---|---|---|
| a hole, unmarked | `x` | `GREET "bob"` |
| resolved where the quote is written | `$x` | `$(GREET "bob")` |
| resolved where its code is built | `\x` | `\(GREET "bob")` |

- **A hole** binds only to a binder in the same code — a `LET` before it in the
  quote, or one in code composed with the quote — or to a builtin value or
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
  build supplies ([building code](#building-code)); `\(…)` does the same for
  the one keyworded use it wraps, again covering only that use. A mark keeps
  its `\` through every composition until something binds it.
- **`code USING src`** binds the holes `src` surfaces — a record's fields, a
  module's members, and a keyworded hole to the module's registrations at its
  key — and returns code with the others still holes, as
  [`USING … SCOPE`](#names-that-arrive-at-run-time) makes the names its operand
  surfaces its body's parameters. It applies as often as a program likes, so a
  template's holes can be filled in stages. A field naming no hole is ignored,
  as record width subtyping ignores a field a slot does not name, so templates
  can share one context record; a hole an earlier `USING` filled is no hole, and
  is never rebound.

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
inside `#(…)` is that quote's. Neither mark has a reading outside a quote
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
spreads its value when the code runs, as `..` spreads outside a quote. `$`
still never evaluates, since splitting a list runs no koan code, so the operand
of `$..` is a name, and each element is code: a value element would be spliced
syntax or a nested quote according to its kind. `\..xs` has no reading, since
a `\` name binds where a body is built, to a slot whose value arrives with each
call. A spread is one level deep, as Lisp's `,@xs` and Julia's `$(xs...)` are,
and a spliced part [composes](#composition) as any part does.

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

Taking a fragment out of code is code's slice, as it is a list's. A hole names
no binding, so a fragment taken away from the binder that filled it holds a
hole again: `PRINT x` taken out of a block that also holds `LET x = 4` has `x`
a hole. A fragment keeps its `$` bindings wherever it goes.

### Building code

A shape is built from code in two places, and each fills different names.

- **A callable's body.** A body written in place resolves the names written in
  it where it is written, which is where it is built: in
  `FN :{name :Str} -> Str = #(GREET name)`, `name` is the parameter and `GREET`
  a candidate list read in the callable's scope, as in any body. That is the
  grant a defining expression gives the quote written as its body, and it
  reaches only the names written in that quote. Code that arrives in a body —
  code named as the body, or code composed into a written one — has its holes
  filled only by binders in the composed body, and the callable's parameters
  and scope fill only its `\` marks. Code named as the body of a callable with
  parameters `w` and `h` is written `#(\w * \h)`, and a hole left in it is
  refused as [unbound](#errors) where the callable is built.
- **`EVAL`.** `EVAL code -> <Type>` runs a shape built from the code alone. The shape
  depends on nothing else, since a `\` mark is filled by what the `EVAL`
  offers, so a written quote's is built once, where the program loads, and every
  `EVAL` of that quote runs it; code composed at run time builds its own once,
  kept where the value lives. The build refuses nothing where the program
  loads: an error in it is kept and reported by the `EVAL` that runs it, so a
  quote is checked only where its code runs. The frame it runs in fills no hole, so a name left unbound is an
  unbound-name error. No shape keeps a defining scope for an `EVAL` to search,
  so an `EVAL` takes no hold on a frame and waits on no binder declared before
  it: its operand is an eager mention like any other. Its `\` marks are filled
  only through a [code parameter](#code-parameters)'s type, and any other mark
  left is unbound.

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
the `EVAL` is written, statically, as it resolves a name written there, so
nothing is searched at run time. Composition and slicing recompute a carried
type: a binder composed ahead of `\it` binds it, and `it` leaves the list.
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
the `FN`'s parameters supply the code's `\` marks, and its declared return type
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
compares by its shape and captures, as [knots](../knot/README.md#equality-and-rendering)
say. Printing code never follows a binding, and prints each mark as written, so
printed code reads back with the same holes and marks.

A `$` name naming a fellow knot member is a deferred mention, so a quote
holding one is born in that member's knot as a function node is:
`LET echo = #(PRINT $echo)` is a one-node knot, and it equals its copy. A hole
or a `\` mark names no binding and adds no edge.

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
the lattice's [`DeclaredGroup`](../type_lattice/schema.rs), so a signature's
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
  a hole left in code a callable's body is built from ([quotes](#quotes));
- an **eager cycle** — a component with an eager mention of a fellow member;
- a **mark outside a quote** — a `$` or `\` no quote value holds;
- an **unsurfaced** `USING` — an operand that does not say, where the shape is
  built, which names it surfaces;
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
  quotes, written otherwise: a bare function body, a bare arm set, a bare `SIG`
  body. The message says how the part is written;
- an **inadmissible** part — one read as written whose syntax fills none of its
  slot's types: `42` as a body, a value guard such as `#{1: (a)}` as a
  `MATCH … WITH`'s arms, `#[(PRINT 1)]` as a signature's members, `#{}` as a
  union's variants;
- a **malformed** form — a quote where bare syntax is read (`LET #(x) = 1`,
  `MODULE m = #(…)`, `MATCH x -> #(Number) WITH …`, `CATCH #(x)`), or a body
  that is not the shape its form declares;
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

four [dispatch](../dispatch/README.md#the-overlap-check) finds once the shape is
built and its types can be read — an **overlap**, a user overload taking
operands a builtin overload at its key already takes; **no admitting
candidate**, a keyworded use every candidate of which
[static selection](../dispatch/README.md#static-types) drops, naming the key and
its arguments' static types; an **ambiguity**, a keyworded use every candidate
of which always admits and none of which ranks first, naming the same; and a
**return never satisfied**, a callable body whose static type meets its declared
return at `Never` — beside the refusals of its
[instance sites](../dispatch/README.md#static-types), an annotated binder or a
call by name that can never be satisfied, and the static pass's other checks;

two the [elaborator's load pass](../elaborate/README.md#the-type-channel-at-load)
finds — a **type** that does not elaborate, carrying the elaborator's refusal,
and a **repeated guard**, two guards of one `MATCH … WITH` arm set that type to
one handle, naming both;

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
`:(LIST OF Declaration)` for a signature's members.

## Memory

An activation is `Drop`-free and laid down in its frame's region, so a frame's
death releases it with the region. It is a header of four pointers — its shape,
its closure bindings, the builtin table and, for a block, the enclosing
activation — beside the callable it runs, if any, and one slot array over the
shape's slot count. It holds no
pointer into itself: its shape lives in program storage, its builtin table
outlives every frame, its closure bindings live in the callable value, which
the caller keeps alive across the call, an enclosing activation lives in the
same frame, and its slots hold values. An activation is therefore copied by
copying its bytes. Each kind of body has its own constructor — a program's
with neither closure bindings nor an enclosing activation, a callable's with
the callable and its closure bindings, a module's or a quote's code's with its
closure bindings alone, a block's beside an enclosing activation whose builtin
table and callable it shares — so no other combination can be built.

A shape and everything it holds — declared-name runs, mentions, captures,
components, nested shapes — rest in program storage and are `Copy`, save the
[load-time type](#load-time-types) cells, which are written once, where the
program loads, and never after. A cell holding a `'graph` record makes a shape
invariant in `'graph`, which `'graph` already is everywhere it is named. A written
quote's code shape is built into program storage once, where the program
loads.

## The import rule

`scope` names `crate::values`, `crate::type_lattice`, `crate::symbols`,
`crate::memory`, `crate::parse` and `crate::source`, and no scheduler type. From `type_lattice` it names the
operator-group vocabulary — `DeclaredGroup` and its `ReductionMode` — so a
signature's operator channel and a body's held group are one record rather than
two that must be kept in step. The scheduler reaches scopes through its
embedder, and scopes never reach the scheduler. The compiler
cannot hold a module to that, so [`tests::boundary`](tests/boundary.rs) reads
the module's source and fails on any other `crate::` path, on an owning heap
type outside the one error that lists names, and on a retired lifetime name.

## Open work

- [Dict defaults](../../roadmap/rewrite/dict-defaults.md) — a value dict's `_`
  default, which lifts the dict-default refusal.
- [Code splicing](../../roadmap/metaprogramming/code-splicing.md) — how several parts
  are spliced into a quote at once, and a kind for built code.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — `CLOSE
  OVER`, and a warning for an unmarked keyworded use in a quote.
