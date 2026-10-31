# The program

A loaded program as one owning value, and the body runner that performs it.
[`CellSubstrate`](substrate.rs) holds program storage, the symbol interner, the
frozen load of the types the load builds and the agent tier above it, the
program's record and the
[cell graph](../scheduler/README.md) that runs it, and it can be returned from a
function, moved, kept in a struct and dropped like any other value. An embedder
that keeps a program across calls — a language server, a REPL — keeps one of
these, and repeats no setup order of its own.

The layer above supplies what a program *means*: a [`Language`](record.rs),
which lays the builtin table down, names the step every evaluation runs, and
checks a built shape; [dispatch](../dispatch/README.md) is koan's.
Everything here is the same for any language over it — where a binding lives,
which cell performs a statement, in what order a body's units are performed —
and nothing here names an expression form.

## Owner and dependent

The graph's cells and the AST borrow program storage at `'graph`, and the
type tiers borrow the bumps beside it, so a value holding both owners beside them
is self-referential.
[`self_cell`](https://docs.rs/self_cell) carries the `unsafe` that takes;
koan's production code writes none. Its dependent is one lifetime and no type
parameters, so the substrate is a concrete type over koan's own families rather
than a generic `cellgraph` type.

- **The owner** is the permanent tier and nothing else: program storage, the
  bump the types' load is built over, the bump the agent tier grows in, and
  the interner. It borrows nothing,
  and `self_cell` boxes it and only ever lends it shared.
- **The dependent**, `Running<'graph>`, is everything that names `'graph`:
  - the **graph**, by value. Reclaiming one of its recycling regions needs
    exclusive access, which a shared borrow of the owner cannot give.
  - the graph's **run root**, [below](#the-top-level).
  - the **program record**, [below](#the-program-record), in program storage,
    which holds the **type tiers** (`TypeTiers`): the frozen load and the
    agent tier, each laid down in an owner's bump rather than held by value,
    so the record and the graph can both borrow them at `'graph`. The graph
    hands them to every step and crossing; the record keeps them for a reader
    outside a step. They take bumps of their own rather than program
    storage's `Writer` because each grows as a
    [collections arena](../../lattice/README.md#the-bump-tier) client. Resting
    there is sound because neither owns anything on the global heap — their
    node indexes and
    [verdict tables](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)
    lie in the same bumps — so no destructor needs to run.
  - what the top level **left at rest** when it last ran, for a later root work
    to resume from.
  - the program brand and a borrow of the interner, so a call reaches every
    piece at `'graph` without touching the owner.

Anything else that carries `'graph` and has to outlive a single call belongs in
the dependent too.

## Nothing outside names `'graph`

`'graph` is invariant, so `Running` is declared `#[not_covariant]` and the
substrate exposes no signature that names it. The running state is reached
only through `CellSubstrate::with`, whose closure is quantified over a fresh
lifetime: nothing borrowed at `'graph` escapes a call, and two substrates'
states can never be mixed. Each call's `'graph` names the same storage, so what
one call leaves in the graph the next call finds. No call interns into the
frozen load, and no call records a name in the load's symbol table: a name
recorded after the load lands in the interner's overlay, and the run's types
live in the agent tier or in the branches of the habitats that carry them
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)),
and leave with them.

**Setup runs in the builder.** `CellSubstrate::load::<L>` does all of it inside
`self_cell`'s builder closure, which sees the same `'graph` every later call
does: it parses the source into program storage, lays a root builder in the
owner's bump and builds every load-time type through one chain over it, has
`L` lay the builtin table down, builds the program's shapes over that table,
has `L` run [its walk](../build/README.md#the-one-walk) over them into the
load's store — the [type half](../elaborate/README.md#the-type-channel-at-load)
and the value half unit by unit — and folds the records, lays the
`LoadFacts` the program and every activation read them through, freezes the types' load so no step
can intern into it and the interner's load table so a later name lands in its
overlay, lays the agent tier in its own bump, and lays the record down beside the
[output sinks](#faults-and-output) the embedder hands `load`. All of that runs
before the graph stands: the scheduler's
[load door](../scheduler/README.md#the-drain) runs the load, then stands the
graph up over one slab root, the run root, handing it the load's facts and
the door its misses [settle](#settlements-and-activations) through. `load`
takes the build arena's budget beside the slab cap
([the scheduler](../scheduler/README.md#the-build-arena)); the interpreter
passes its default. It returns a `Result`, and
`LoadError` carries the parse error, or the rendering of the `ShapeError` that
stopped it. A shape error borrows program storage and names symbols and types
through the interner and the load's chain, all of which a refused builder drops with
its owner, so it is rendered on the error branch, while they stand; a load that
succeeds renders nothing. The
`LoadError`'s `Display` is the whole diagnostic, led by `path:line:col`. The
table comes before the shape because a shape resolves a builtin name to an
index into it.

## The program record

[`Program`](record.rs) is what a loaded program's steps read, at `'graph`: the
top level's shape, the builtin table, the type tiers and the interner, the
evaluator's step, the output sinks and the sealed `Error` type. It rests in program storage beside the table, written through
`cellgraph`'s [`Storage`](../memory/README.md#two-tiers-and-why-the-boundary-falls-where-it-does)
writer, so every step names it by a `'graph` borrow that crosses at no price.
The builtin table is there for the same reason: it is covariant in its cell
brand, so it shortens to any activation's, and an activation at any brand names
it with no door — a call pays nothing to reach a builtin. A table in the root's
region would cost a redeem and a pinned placement in every cell that owns an
environment.

**`Program::evaluate` is the one door every evaluation is asked through.** It
takes a node — a whole statement or one part of one — and the view of the
activation it is read in, and hands back a `Work` the drain can create, born
holding `KBirth::Evaluate`. The caller picks the placement and the `Use`; what
the node means is the evaluator's. That keeps evaluating an expression
dispatch's, and everything in this module below it free of expression forms.

**`Language` is a trait**, with three methods generic over `'graph`: `builtins`,
which lays the table down through program storage's writer, `evaluator`, the
step an evaluation runs, and `facts`, the language's
[walk](../build/README.md#the-one-walk) over the program's shapes into any
store it is handed — the load's, or a test's over the load's records — which
may refuse the program, as dispatch's
[overlap check](../dispatch/README.md#the-overlap-check) does, and records
what it decides in the store, as dispatch's
[static types](../dispatch/README.md#static-types) are. Its default walks
nothing, so a language without one loads with no facts and its run reads
every type where it runs. A record of function pointers ranked over `'graph`
cannot name the bundle's projections (rustc #100013); a method generic over
`'graph` is handed the one the program loads at. Dispatch implements it; the
tests implement a [miniature](tests/evaluator.rs).

## The bundle

[`KBundle`](bundle.rs) is the [step bundle](../scheduler/README.md#the-continuation)
a program's steps run over, with its three families:

- **`KBirth`**, what a cell is born holding, which crosses and is covariant:
  `Program`, the top level's root work; `Call`, a callee, a record of its
  arguments by name, how the call reached it — by keyword or by name — and,
  for a call by name the load recorded contributions for, the type each
  parameter is solved from, one per parameter in symbol order, none where it
  reads its argument's carried type;
  `Eval`, a quote's code, a record of the names its `EVAL` offers and the
  contract its declared return makes; `Evaluate`, a node, the view it is read
  through and the [contract](#frames-contracts-and-tails) it owes, if any; `Block`, a
  synthesized block's shape and the view it sits in; `Composed`, a `USING` or
  a `BUILD` over code composed at run time, which under the code's
  composition fills its holes from a source or, with none, carries it at the
  exact type its shape gives it; and `Inspect`, the view the top level leaves at rest. A `Call`
  also carries, for a hop under its frame's own settlement, the level of that
  settlement, which a callable born in the frame records in its stamp; what
  the call hands its frame through kept parts (`Passing`), which every call
  lays; and, once the wake has kept each of those values, what the frame keeps
  of its callee and arguments (`Keeps`) — beside, in a `Call` or an `Eval`,
  where its asker's decision memo rests. An `Evaluate` and a `Block` carry
  what their frame runs **within**: whether it [routes](#caching-bodies), the
  arena node it runs under and the stamp of its chain, the homes of its
  decision memo, under a composition the code's root, and the table of what
  its frame keeps ([frames](#frames-contracts-and-tails)). The family's
  `covariant!` witness is what checks that an activation's view is covariant
  in its brand.
- **`KState`**, what a cell holds at a step and parks with: the birth on a
  first step, the body runner between units, or an evaluator's resumption.
- the **scratch** family, the sites a refused tie named, one per evaluation the
  runner asked for, in the order asked.

The body runner's activation binds, so it is invariant and cannot cross. It
rides the parked state, which never crosses: a park stores it in the cell's own
continuation and the wake hands it back at the same cell's brand. Riding the
scratch state instead would hold the root's scratch bump off its reset for the
program's life, keeping every discarded top-level value and every receipt run
until the end.

**A view is never copied.** A birth holding a view is weighed at no bound, so
the verdict always pins it: it borrows another cell's region, and an evaluation
or an inspection — a frame's hand-off included — is born under the cell it
views, where pinning costs nothing and pledges nothing.
A birth that is copied rebuilds what it carries: a `Call`'s callee and
arguments, and an `Eval`'s code and offered names, are deep-copied through
[`copy_severed`](../values/README.md#crossing), the crossing's door for a value
held inside a copied operand of another family, so the copy is still one the
graph priced.

## The top level

A top-level binding lives in the region of the **run root**: a storage-only
slab cell taken at load, never entered, and released with the substrate. The
slab holds the run root and nothing else, so a program
with more statements than the slab cap runs to completion — a call subtree is tree cells and tenants under
the root, a module activation is data, and a binding lives in the root.

`Running::run` births the body runner as the root work, a **tenant** of the
root. Its `'here` is therefore the root's region: it lays the top level's
activation down there, and a top-level bind is a plain write. The top level is
a frame like any other, and the drain creates and retires every cell it runs.
A root that was itself the runner would be a cell the drain must not retire.

When the body ends, the runner **leaves** `KBirth::Inspect` — its activation's
view — at rest in the root through `Step::leave`, and `run` keeps the `Resting`
it gets back. `Running::inspect` resumes a later root work from it, again a
tenant of the root, which reads a top-level binding where it lies and leaves
the view at rest again. That is how a REPL or a test reads a binding after the
drain, in a separate call from the one that ran it.

## The stack

A program's load and run need a stack of a known size: the walks over parsed
syntax recurse once per nested part, and a debug build's frames are many times
a release build's, while a platform's main-thread stack is whatever the
platform chose. `STACK_BYTES` ([substrate.rs](substrate.rs)), 64 MiB, is the
stack a program nested to the parser's
[depth limit](../parse/README.md#the-syntax-depth-limit) loads and runs in under
a debug build, and a host runs a program on a thread of that size. The
interpreter binary does, re-raising a panic on that thread as its own.

## The body runner

[`run`](body.rs) is one step that performs the top level, every called body,
the code every `EVAL` runs, and every block a pairwise operator run hoists an
operand into. Its state is the activation, the next unit, and the stage it parked at.
It claims nothing ahead, ties and binds each unit itself, and no unit has a
cell of its own: a cell per unit would cost a create, an enter, a release and a
receipt slot per statement per call, for statements that mostly never park.
Born as `KBirth::Program` it lays the top level's activation down; born as
`KBirth::Call` it lays the callee's activation down in the frame's own cell and
binds each value parameter from the argument record, which must name them
exactly, **retyped** to the parameter's declared type with the call's
type-parameter solution substituted, a digested parameter taking the digest
memo the caller handed for it where the value it binds carries the type the
caller's did; born as `KBirth::Eval` it lays the code's activation down in the
frame's own cell over a closure run it assembles in the code shape's capture
order — each `$` name and filled hole from the binding the code carries at
its scope and name, each `$(…)` key of composed code as the list of the
registrations it carries at that scope whose key it is, grouped here, where
the type chain is, a keyworded hole nothing filled as an empty list, each `\` name from the offered record — every edge resolved to the
member it names, so the run holds value words only. An unbuilt
[code view](../knot/README.md#a-code-view) is laid flat before its frame is
asked for.

**How a frame binds depends on how the call reached it.** A keyworded call's
arguments were admitted by the [selection](../dispatch/README.md#selection) that
chose the callee, which also put each type parameter's solution in the record
by name, so the frame trusts both. A call by name's arguments were written by
the caller, so the frame **admits** every argument against its parameter's
declared type under one collector — `:(FN :{x :Number} -> Str) cannot be called
with :{x :Str}` when one does not fit — and, for a **quantified** callee, solves
the group from that collector itself. The load records a call by name's
[contributions](../dispatch/README.md#static-types) one per parameter
in symbol order, so the frame finds a parameter's entry by how many parameters
sort before it. The load records them only where its argument names no field
but the parameters of the callee it saw, so a call whose contributions or
record are not exactly as long as the running callee's parameters is no call
the load admitted: it misnames before any entry is read. A parameter with a
contribution is admitted at that type,
resolved where the call runs, and every other at its argument's carried type,
so the solve follows the declarations as a keyworded call's does; a type
parameter the caller writes into the record names no parameter, and misnames
the call. An
[instance](../knot/README.md#an-instance) carries the solution the load made it
at, so its frame takes that solution and solves nothing. A **`:Type` parameter** —
a type-channel parameter that is no `FOR ALL` name — is an argument like any
other: the frame binds it to the type value the call passed, by keyword or by
name. Either way a value parameter is an ascription: its argument is
[retyped](../values/README.md#the-type-memo-and-satisfies) to the declared
type, the solution substituted, so the body dispatches on what the parameter
declares rather than on the contents' further precision, and `SHOW [1]` under
`SHOW x :(LIST OF Any)` sees a `LIST OF Any`. Each `FOR ALL` slot is then bound
to a type value holding its solution, looked
up **by its own name** through the callee's
[quantifier map](../knot/README.md) — the slots arrive symbol-sorted, not in the
order the `FOR ALL` group was written. The group keeps every name, so each is
bound to its solution, a name no argument reaches to its bound. A callee that is no function, an argument record
that misnames a parameter, an argument that does not fit its parameter, and a
group the arguments cannot solve are each a
[fault](#faults-and-output). Born as `KBirth::Block` it lays a block's
activation down beside the view it sits in, in the asker's own cell, and yields
its last statement's value. A block is entered as its birth's `Entered` says
(`entered_block`). A `USING … SCOPE` body is entered on a module: its
parameters are bound to what the module surfaces
([entering a block](../knot/module/README.md#entering-a-using--scope-block))
before its first unit runs, and since its shape and the module cannot disagree,
a refusal there is an invariant break. A `MATCH` arm is entered on its `it`,
which its one parameter binds ([matching](../dispatch/README.md#matching)).

**A frame called through a barrier** — a function an opaque view carries
([members are born coerced](../knot/module/README.md#members-are-born-coerced)) —
crosses it both ways. The argument record crosses each stacked barrier
inwards, unsealing what is sealed under a carrier, and the function behind them
is then bound as a call by name, so its frame admits the arguments and solves
its group; an argument that cannot cross is the call's misfit,
`… cannot be called with …: it is not sealed under Carrier`. The frame's value,
once held to its contract, crosses the barriers outwards, innermost first, and
a value that cannot is a fault. Once out, it is retyped to the return the
barrier shows its caller, so a field the view's signature hides is no field of
it. Such a frame never tails, since its value crosses where the frame ends.

**A callee has two types**: the one it shows, `callee.ktype()`, and the one its
body was checked against, `Knotted::declared`. They differ where a function sits
under a [header](../values/README.md#the-type-memo-and-satisfies) — retyped at a
function-typed slot, or read through a signature — or was born at an annotation
that retypes it ([the tie](../knot/README.md#the-tie)), which keeps its declared
signature in its typing record. A call by name of such a callee admits and
retypes the arguments at the parameters it shows, then binds them at the ones it
declares, a call by name past the shown type that solves the node's own
group; a function type's parameters are a width subset of what a
slot of it may show, so the node binds only the parameters it declares. A
keyworded call's selection admitted its arguments at the shape shown already,
and where that shape is quantified it carries the solution of the shape's
group in the argument record, a type field per name of the group — a value, so
a birth's copy adopts it — which the frame reads to solve the shown return, and
which the node, binding only its own parameters, never sees. Where neither type
is quantified the shown parameters lie under the declared ones, so a call by
name past a header admits its arguments once. Its value is retyped to the
return it shows through the contract, whose
`retype` is the shown return; where either type is quantified the order does
not promise the node's return lies under the shown one, so the contract checks
the value against both (`checked`).

It performs the units in the order the shape emitted them
([Units](../scope/README.md#units)), each after every unit it reads, over a
depth-first drain that finishes each before the next begins. So every read
finds its slot bound, a tie never names a pending binder, and a program whose
statements have effects and no data dependency takes source order without
interleaving. Per unit:

- **A component of type binders** is bound to the handles the
  [load's walk](../elaborate/README.md#the-type-channel-at-load) fixed for it,
  or a layer the frame reads fixed under its binding — a nominal an
  activation's walk declared, interned through the frame's chain — or raises
  the refusal every run of it meets.
- **A module binder's body runs inline.** Its activation is laid down in the
  running region, the enclosing body's place is kept in an `Outer` written
  beside it, and when the body's units are done the binder is tied over the
  finished activation and the enclosing body resumes past it. A module
  activation takes no cell of its own; it is data, placed by the ordinary
  crossing verdict in the region of whatever keeps it.
- **A cyclic component, or one whose members all birth a callable**, is
  [tied](../knot/README.md#the-tie) once, and every member's slot is bound from
  the knot. A tie refused on eager parts names them all at once: the runner
  asks for one evaluation per part at one park, keeping their sites in scratch,
  wakes once when the last receipt slot fills, and ties again with each value
  supplied by site.
- **A lone data binder** is one evaluation of its right-hand side, asked with
  `Keeps`, and bound on the wake.
- **An annotated binder**, `LET <name> <type> = <value>`, binds its value held
  to the type as an ascription holds its operand: checked against it unless the
  load [settled](../dispatch/README.md#static-types) it, and retyped to it. A
  miss is a [fault](#faults-and-output),
  `:{v :Any} does not satisfy its annotation :{v :Number}`. A tied member is
  held the same way, and is already born at its annotation, so its slot binds
  the bare member.
- **A statement that binds nothing** is one evaluation, asked with `Reads` —
  save a frame's last unit, which the runner [tails](#frames-contracts-and-tails)
  into, and a block's last statement, asked with `Forwards` so the value is
  built where the block's result goes — `Keeps` in a
  [caching body](#caching-bodies). A body whose last unit is a binder
  yields that binding's value.

The only children the runner asks for are evaluations, through
`Program::evaluate`, each handed the view of the runner's activation. The
placement is the level's: at the top level an evaluation is a `Fresh` tree
child, so everything it allocates beyond its value dies with it while the value
is built in the root, its home, from the start — or crosses into it at the
verdict's price, splicing its region in when pinned and reclaimed when copied.
In a called body an evaluation is a `Shares` tenant of the frame, so what it
builds is at the frame's `'here`, binding a slot is a plain write, and a reader
reads it where it lies — save the hand-off of the frame's last statement, a
tree child of the frame ([below](#frames-contracts-and-tails)).

Transients inside a step — the parts a tie named, the values supplied back to
it — go on a local bump for that step, never a `Vec`.

### Frames, contracts and tails

A called frame owes its caller a value satisfying its callee's declared return,
with the frame's own type-parameter solution substituted: a
[`Contract`](record.rs), which names the callee so a miss can say whose return
it was. The frame's value is **retyped** to the declared return, member by
member as [any retype](../values/README.md#the-type-memo-and-satisfies) is, and
a value that does not satisfy it is a fault
(`:(FN :{} -> Number) returned Str, which does not satisfy Number`).

When a frame's last unit is a statement that binds nothing, the runner does not
wait on it: it **tails** into the evaluator, handing it the contract, and the
evaluation owes the frame's caller the value. The evaluation is the frame's
**hand-off**, a tree child of the frame that inherits its provenance, so its
view of the frame's activation pins free and its own successor is born beside
the frame, never under it
([tail hops](../scheduler/README.md#tail-hops)). The frame is released after
the hand-off's first step and disposes when the hand-off does. An evaluation whose selected call's declared return satisfies the
contract tails again, into the callee's frame, so a tail recursion N deep holds
a constant number of cells. Whether it does is one answer,
[`Contract::kept_by`](record.rs), for a keyworded call at its solved group and
for a call by name, where only an unquantified callee keeps the contract, since
its frame has not yet solved; any other value is held to the contract where the
evaluation finishes. The hop hands the frame it reaches the contract it owes,
and the caller asked for the outermost callee's return: that frame's value is
checked against its own callee's return, which a miss names, and retyped to the
outermost return of the chain — the contract's `retype`. So beside
`INNER -> :(LIST OF Number)`, an `OUTER -> :(LIST OF Any)` tailing into it
returns a `LIST OF Any`. An `EVAL`'s frame owes the type the `EVAL` declares
as a called frame owes its return; its contract names no callee, so a miss
names the `EVAL`'s code (`` `EVAL`'s code returned Str, which does not satisfy
Number ``). A frame called through a barrier never tails. A block tails only as
a tail `MATCH` arm: the `MATCH`'s evaluation hands the arm's block off owing its
own contract, and the block tails its last statement as a frame does
(`Runner::tail_body`), so a recursion through tail arms holds a constant number
of cells. Every other block runs as a child and never tails.

**A frame keeps what it was handed.** A frame called with no barrier before its
callee lays a table in its region at its first step (`FrameKept`): a
[kept](../scheduler/README.md#kept-and-held-values) carrier at the true home of
its callee and of each parameter's argument — taken from the `Keeps` the wake
laid beside its birth, and kept in the frame where its birth was copied — and
of each binder whose evaluation delivered a value pinned at a home of its own.
A name read of the frame's body that lands on a slot the table fills is held
beside that carrier. Every name a callable body reads from outside it, a
top-level name included, is a
[capture](../knot/README.md#closure-bindings-and-edges) of its callee, so a
read that lands on one is held beside the kept callee, one capture segment
down, and a module member read beside its module's kept part, one member
segment down. So a frame that returns its parameter, a binder bound bare to
one, a top-level name, a module's member, or a field of any of them hands it
back from where it rests rather than copying it into its caller. A `MATCH`
arm's `it` is held where its scrutinee rests, as a restamp of the scrutinee's
kept part or one payload segment below it: the arm's block carries it in a
chain beside the arms around it (`ArmKept`, on `Within`), which a name read
consults before the frame's table.

**Every call hands on a skeleton.** A call lays a `Passing` beside its birth:
the arguments held beside a kept part, by name, the callee through the kept
part it was read through, the argument record's field names, and, for a tail,
which arguments the frame built to outlive the hop. A call by name gathers a
record literal's fields itself, so each is handed through its own kept part.
The wake keeps each value at its true home
([the scheduler](../scheduler/README.md#kept-and-held-values)). A spawned
frame's spawner is above it, so its birth pins whole, and it keeps each value
from the kept part it was handed through, or from the birth where it has none:
a helper that returns a part of its argument hands back a value homed where
its caller's argument rests, not in the calling frame. A hop's successor
crosses each value on its own, rebuilds the argument record in its own region
over them, each restamped at the type the skeleton held it at, and keeps a
carrier of each; the skeleton and the predecessor's record die with the hop
([tail hops](../scheduler/README.md#tail-hops)).

**What a loop keeps.** A binder of the frame body's — or a tail arm's block's —
[threaded set](../scope/README.md#the-threaded-set), and each argument part of its
last statement in it, is asked for with `Use::Threads`, so it is built in the
loop's cell where the frame's provenance names one: what a tail call hands on
that the frame built outlives the hop, built over what it reaches where that
rests, and everything else the frame and its hand-off build is reclaimed at
the next hop. A callee a tail hands on bare — a function the frame tied, an
instance the call made, a spread candidate — threads too: it is copied into
the loop cell once, and the loop's frames keep it, and every capture read
through it, there. A loop's memory is therefore a constant times what its
latest hop threads, plus one hop's region; a loop that threads nothing and
hands every callee through a kept part builds no loop cell at all.

### Caching bodies

A body the load found [caching](../dispatch/README.md#body-classes) lays its
values in the region its writer lays into and its types in a scratch tier
([where each body interns](../../design/type-tiers.md#where-each-body-interns)).
Each step picks one chain at its entry ([`run`](body.rs), and the evaluator's
`evaluate`):

- the **caching chain** in a caching body, and in the own body of a caching
  frame that routes, which holds its scratch for its life at its first step.
  It reads the frozen load, the agent tier, the write home's region and the
  scratch, and writes the scratch;
- the region chain in every other body.

The `routes` bit of the `Within` on each evaluation and block the frame asks
for ([`KBirth`](bundle.rs)) carries the frame's routing down, and the hand-off
of the last statement clears it. A frame under a settlement under the root
reads an invariant site's types where the settlement rested them, in the agent
tier, and adopts nothing there; a frame under none, or under a composition,
mints them into its scratch, its warm tier and its cold tier at once.

A caching body asks for its blocks and its yielding statement with `Keeps`
rather than `Forwards`, so a value leaves it only through an **exit**
([`Exit`](exit.rs)), where every type the value reaches is adopted into the
region first: a call's callee, arguments, contributions and the contract a
tail hands on, an `EVAL`'s code, offered record and declared type, a value
owed a frame's contract, the frame's end, and — in a frame that routes — the
hand-off of its last statement, which adopts the binders the shape lists
that statement reading, and the frame's contract. The walk stops at a part
whose [memo bits](../values/README.md#weight) say its types already resolve
there, handing over nothing for it, and marks what it walked settled, so a value
the frame received or passes on is walked once
([type tiers](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part)).

### Settlements and activations

A call, whatever its placement, hands the drain a `Seek`
([`seek`](settle.rs)): the arena node the callee was born under, read off its
closure bindings, and, where the callee's body record lists keys and the
asker can read load content at each, the key — the callee's base and segment
and the content digest at each keyed name
([the build module](../build/README.md#keys-which-bindings-make-a-settlement))
— the image of the types that content carries, and what a miss settles.

Where the callee's body can birth a callable — a bit its shape seals and its
body record carries, set by a nested callable directly or through a nested
block or module body, never through a quote, whose callables the `EVAL` that
runs it births — the request is laid as the frame's **level**, once, in the
frame's own region, by the one function [`level`](settle.rs). The frame's
region does not exist while its asker runs, so the `Seek` carries the request
by value, beside its binding's image, and the drain hands both back to the
frame's first step once the frame is born under that settlement
([the scheduler](../scheduler/README.md#the-build-arena)). The frame relays the
image into its region and lays the level there at entry, so the level's types
resolve for a callable born under it whatever its asker was — under a
composition, a tail call's asker is the dead hand-off, no ancestor of the
frame. A hop under its frame's own settlement hands the frame's level on in its
birth. Nothing is laid in the asker, so a loop making a keyed call each hop
leaves nothing per hop in its asker's region.

A miss runs [`settled`](settle.rs): the
language's walk rooted at the callee's body over a layer on its parent's
facts, its types adopted into the agent tier under the root and kept as terms
under a composition, so every frame of that body under that binding finds its
invariant types warm. A frame born under a settlement routes: it holds its
scratch. Dispatch picks the door where it calls: a hop of the frame's own
callee, keyed, from a frame under a settlement its route marks a hop,
`tail_under` the frame's arena node; any other call `spawn_seeking` or
`tail_seeking`; an `EVAL`, a `USING` or a `BUILD` of composed code `spawn_composing`; an
evaluation or a block `spawn`; and a frame's last statement `hand_off`.

A frame's runner takes the arena node it runs under at its first step and
hands it, with its stamp — the frame's level over its callee's — in its
`Within`, to every evaluation and block it asks for, so a callable a tie or a
lambda births in the frame, its blocks included, is born under it, seeks
beneath it and replays its chain where it is gone. A frame under its own
settlement whose body births no callable holds no level, and its `Within`
carries no stamp; a frame under the arena node its callee was born under
carries its callee's. A call whose callee's
arena node is no longer live replays the callee's stamp before the frame
runs. The top level and a module's closure run under the root, and an `EVAL`
of composed code, or a call of a function whose body is untraced code, under
its composition.

At each binding event whose names its arena node's facts leave unbound —
its entry, a statement seam, a block's entry, an `EVAL`'s captures — the
runner decides in the frame's **activation**
([`activation_store`](activation_store.rs)): it reads the binding off its
slots, walks the body over what it read below, relays the records into the
frame's region and layers them on what every evaluation and block it asks for
reads. It first looks the event up in its decision memo, and walks only where
the memo holds no decision for those bindings
([the build module](../build/README.md#activations)). A refusal the walk
reaches, or one the frame's settlement kept, faults the frame worded as the
load words it.

### Faults and output

A running program's refusals are **faults**: an evaluation, a frame and a block
each finish with a value or a fault, and a fault is no koan value. User code
raises none — a function that can fail returns a `Result` — so nothing in the
language reads a fault between its raise and where it is caught, and it
travels as the runtime's own: one of a closed set of kinds, each carrying
structured fields — types, names, code, or where a value lies in a frame the
fault keeps — and no rendered text. Every unit that receives one — an
evaluation's outcome, an eager part a tie named — stops the body with it, and
so does every refusal the program is to blame for: a tie or a declaration
refused, a callee that is no function, arguments that do not fit it. A frame
finishes with the fault; a block does the same. `StepError::Refused` is left
for invariant breaks alone.

**A fault keeps the frames it passes through.** A frame that finishes with a
fault is not released: its region holds what the fault's fields locate and what
its trace names, until a `TRY` or a `CATCH`
[catches](../dispatch/README.md#catching) the fault or the host drops the
uncaught outcome holding it. Only frames live at the raise are kept, and a
[`release`](../../cellgraph/README.md#verbs) outlives its call until the tree
cells under it dispose, so the catcher releases them in any order. A frame a
tail hop replaced is gone; the frame that replaced it counts it, per callee.

At the top level, the runner renders an uncaught fault through the program:
`error: <message>` to the error sink, from the fault's fields, and beneath it
one line per live frame it passed through, innermost first, naming the callee
and the `path:line:col` of the call, with a callee tail hops reached more than
once shown as one line with its count. It marks the run uncaught and leaves its
view at rest as it always does, so the bindings bound before the fault stay
inspectable.

`load` takes an [`Output`](record.rs) of two sinks, `print` and `error`, which
the program record holds; the builtin `PRINT` writes through the first.
`Running::run` answers an `Outcome`: `Completed`, or `Uncaught` when a fault
reached the top level, holding the fault and the frames it kept until the host
drops it.

**One placement for a call.** [`call`](body.rs) is what an evaluator asks for
to call a function: a frame running the callee's body, always `Fresh`, in a
region of its own. A result that is its argument, or a part of one, needs no
shared region: the frame hands it back from where it rests
([frames](#frames-contracts-and-tails)). Tenancy is left to a frame's own
evaluations and blocks, and to `EVAL` and `USING` frames. A builtin's native
step states its own placement in the request it makes.

**Running code.** [`eval`](body.rs) is what an evaluator asks for to run code
under `EVAL`, beside `call`: a `Shares` frame running the code's shape —
a written quote's, built where the program loaded, or composed code's, built
in its [composition](../build/README.md#where-facts-are-decided), which the
drain finds by the code's syntax digest or builds over the copy the frame's
first step hands it. A written quote's refusals are found before anything is
spawned; composed code's, by its frame under the composition. The
evaluator hands it the names the `EVAL` offers, read at the coordinates the
shape recorded for its operand (`BodyShape::offers`), as a record. It refuses
with a `CodeRefused`: `Shape` for code whose shape kept an error, `Unbound`
for the first name hole, `\` name the `EVAL` does not
offer, or keyworded hole some use in the code selects from alone, and
`QuantifiedRead` for a `$` name at composed code's entry bound to a
quantified function and read other than as a call's head. An overlap the
code's registrations make is a load fact of a written quote, reported here,
and one waiting on a `$` name is checked in the frame's activation. A keyworded hole nothing filled that every use of it can do without binds
the empty list of functions. That the operand is code at all is what the
evaluator checks. The frame owes the type the `EVAL` declares, as a called
frame owes its return ([frames](#frames-contracts-and-tails)), and reads
nothing of the scope the `EVAL` is written in but those names, so code fills no
hole from the frame that runs it ([building code](../scope/README.md#building-code)).

**Running untraced code.** A call of a function whose body is untraced code
([building code](../scope/README.md#building-code)) runs that code as an `EVAL`
runs composed code, binding the callee's parameters: its frame is born under
the composition the function's typing record finds — by the digest of the
code's syntax and the parameter names, so a callable with no parameters shares
`BUILD`'s — and activates its root, binding each parameter as any call's frame
does, and its captures from the code's bindings and from the record the
declaring scope offered where the function was born. Before it runs, the frame
faults with the composition's kept refusal or with `unbound`'s, as an `EVAL`
frame refuses. It seeks no settlement keyed on its parameters and carries no
stamp, and its arguments and result are checked against the callee's declared
signature, as every call's are.

## The scheduler is a view

A [`Scheduler`](../scheduler/README.md#the-drain) borrows the graph and owns
none, so `run`, `inspect` and `Running::scheduler` each make a fresh drain over
the substrate's graph for the length of one call. A call whose drain stalls is
accepted as it stands: the view drops what is on its stack and the graph keeps
the cells already born, under the root they were born under. A stalled
substrate still runs a later root work, and is otherwise only ever dropped.

## Imports and tests

Outside `#[cfg(test)]` this module names `crate::build`, `crate::elaborate`, `crate::knot`,
`crate::memory`, `crate::parse`, `crate::scheduler`, `crate::scope`,
`crate::symbols`, `crate::type_lattice` and `crate::values`.
`tests/boundary.rs` reads the source to hold the rule there.

The suites run over the [miniature evaluator](tests/evaluator.rs), which records
what it evaluates and where it builds. [`tests/programs.rs`](tests/programs.rs)
runs whole programs under the drain: independent statements in source order
without interleaving, a forward capture running its binder first, more
statements than the slab cap, a binding built in the root from the start, a
frame's result crossing into the root at the verdict's price, a call returning
its argument handing back its storage, a called body's
evaluations as tenants of the frame, a component bound from one knot, eager
parts supplied by site in one wake, a recursion deeper than the slab cap, a
module body run inline, lambdas — born through the
[lambda door](../knot/README.md#a-lambda) after a later binding they read,
returned from a frame and called with their captures, held in a knot and
reading their fellow through an edge, and supplied to a tie as an eager part —
`EVAL` running code whose `$` name binds the caller's value, a hole unbound
whatever the callee declares, a parameter needing a name offered it, a function
built from code carrying its bindings as captures, a quote reading its own
binder, a binder before an `EVAL` statement run after it, a malformed quote
refused only when run, a required keyworded hole unbound, an argument a call by name does not fit
refused, a refused tie ending
the program uncaught, a frame's value retyped to its declared return and a
return that misses it, an argument retyped to its parameter's declared type, a
tail chain returning at the outermost contract, a self-call in tail position holding its cells constant,
and one whole program with all of them.
[`tests/substrate.rs`](tests/substrate.rs) loads two programs through a helper,
moves them into a `Vec`, runs each, and reads a binding back in a separate call
through a resumed root work; it also checks both load errors, an inspection
before the program ran, and a stalled substrate.
[`tests/stores.rs`](tests/stores.rs) holds the test door the dispatch suite
resumes a load through — a resumption over a **layer** of the load's records,
a **reload** under a binding from the start, and a **frame** whose binding is
read off the values its slots hold — beside the test stores themselves: a
layer reads its base where it wrote nothing, wakes exactly where a frontier
meets a newly bound unknown, and a frame binds a type value as a type and a
module value by its digest tree. The two-program test,
`a_whole_program`, `an_eager_part_is_supplied_by_site_in_one_wake`,
`a_call_binds_each_type_parameter_to_its_solution`,
`a_lambda_returned_from_a_frame_keeps_its_captures` and
`a_self_call_in_tail_position_holds_its_cells_constant_however_deep` are on the
[Miri slate](../../observe/miri_slate.md).

## Open work

- [Faults and call traces](../../roadmap/conditionals/faults.md) — faults apart
  from values, the frames they keep, and the trace an uncaught one prints.
- [Catching errors](../../roadmap/conditionals/catching.md) — `TRY` and `CATCH`,
  which build an `Error` from a fault and release its frames.
- [A refused program stays loaded](../../roadmap/rewrite/refused-programs-stay-loaded.md)
  — a refused load kept, so its shape error renders on demand rather than once.
- [One cell per frame](../../roadmap/rewrite/one-cell-per-frame.md) — a
  called body run in its frame's own cell, parking only on a spawned frame,
  with no hand-off.
