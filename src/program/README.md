# The program

A loaded program as one owning value, and the body runner that performs it.
[`CellSubstrate`](substrate.rs) holds program storage, the symbol interner, the
type registry, the program's record and the
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
registry borrows the bump beside it, so a value holding both owners beside them
is self-referential.
[`self_cell`](https://docs.rs/self_cell) carries the `unsafe` that takes;
koan's production code writes none. Its dependent is one lifetime and no type
parameters, so the substrate is a concrete type over koan's own families rather
than a generic `cellgraph` type.

- **The owner** is the permanent tier and nothing else: program storage, the
  bump the type registry is built over, and the interner. It borrows nothing,
  and `self_cell` boxes it and only ever lends it shared.
- **The dependent**, `Running<'graph>`, is everything that names `'graph`:
  - the **graph**, by value. Reclaiming one of its recycling regions needs
    exclusive access, which a shared borrow of the owner cannot give.
  - the graph's **root**, [below](#the-top-level).
  - the **type registry**, laid down in the owner's bump rather than held by
    value, so a record laid down there can borrow it at `'graph`. A registry
    held in the dependent could not be borrowed by a sibling, and would reach a
    step only through a per-call channel on the step context. It takes a bump
    of its own rather than program storage's `Writer` because it is a
    [collections arena](../memory/README.md#two-tiers-and-why-the-boundary-falls-where-it-does)
    client — it grows tables as it interns. Resting there is sound because the
    registry owns nothing on the global heap — its
    [verdict table](../type_lattice/README.md#storage-one-region) is a fixed
    cache in the registry's own bump — so its destructor never needs to run.
  - the **program record**, [below](#the-program-record), in program storage.
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
one call leaves in the graph or interns in the registry the next call finds.

**Setup runs in the builder.** `CellSubstrate::load::<L>` does all of it inside
`self_cell`'s builder closure, which sees the same `'graph` every later call
does: it parses the source into program storage, lays the registry in the owner's
bump, has
`L` lay the builtin table down, builds the program's shape over that table, types
its type channel through
[the elaborator's load pass](../elaborate/README.md#the-type-channel-at-load),
has `L` check it and record what it types, takes the root, and lays the record down, beside the
[output sinks](#faults-and-output) the embedder hands `load`. It returns a `Result`, and
`LoadError` carries the parse error, or the rendering of the `ShapeError` that
stopped it. A shape error borrows program storage and names symbols and types
through the interner and registry, all of which a refused builder drops with
its owner, so it is rendered on the error branch, while they stand; a load that
succeeds renders nothing. The
`LoadError`'s `Display` is the whole diagnostic, led by `path:line:col`. The
table comes before the shape because a shape resolves a builtin name to an
index into it.

## The program record

[`Program`](record.rs) is what a loaded program's steps read, at `'graph`: the
top level's shape, the builtin table, the registry and interner, the
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
step an evaluation runs, and `check`, which may refuse the program's shape once
it is built and its types can be read, as dispatch's
[overlap check](../dispatch/README.md#the-overlap-check) does, and may record
what it learns on the shape through program storage's writer, as dispatch's
[static types](../dispatch/README.md#static-types) do. A record of function pointers ranked over `'graph`
cannot name the bundle's projections (rustc #100013); a method generic over
`'graph` is handed the one the program loads at. Dispatch implements it; the
tests implement a [miniature](tests/evaluator.rs).

## The bundle

[`KBundle`](bundle.rs) is the [step bundle](../scheduler/README.md#the-continuation)
a program's steps run over, with its three families:

- **`KBirth`**, what a cell is born holding, which crosses and is covariant:
  `Program`, the top level's root work; `Call`, a callee, a record of its
  arguments by name and how the call reached it — by keyword or by name; `Eval`, a quote's code and a record of the names its
  `EVAL` offers; `Evaluate`, a node, the view it is read through and the
  [contract](#frames-contracts-and-tails) it owes, if any; `Block`, a
  synthesized block's shape and the view it sits in; and `Inspect`, the view
  the top level leaves at rest. The family's `covariant!`
  witness is what checks that an activation's view is covariant in its brand.
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
or an inspection is born under the cell it views, where pinning costs nothing.
A birth that is copied rebuilds what it carries: a `Call`'s callee and
arguments, and an `Eval`'s code and offered names, are deep-copied through
[`copy_severed`](../values/README.md#crossing), the crossing's door for a value
held inside a copied operand of another family, so the copy is still one the
graph priced.

## The top level

A top-level binding lives in the region of the **root**: a storage-only slab
cell taken at load, never entered, and released with the substrate. The slab
holds the root and nothing else, so a program with more statements than the
slab cap runs to completion — a call subtree is tree cells and tenants under
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
exactly; born as `KBirth::Eval` it lays the code's activation down in the
frame's own cell over a closure run it assembles in the code shape's capture
order — each `$` name from the bindings the code carries, each hole from those
a `USING` supplied, each `\` name from the offered record — every edge resolved
to the member it names, so the run holds value words only.

**How a frame binds depends on how the call reached it.** A keyworded call's
arguments were admitted by the [selection](../dispatch/README.md#selection) that
chose the callee, which also put each type parameter's solution in the record
by name, so the frame trusts both. A call by name's arguments were written by
the caller, so the frame **admits** every argument against its parameter's
declared type under one collector — `:(FN :{x :Number} -> Str) cannot be called
with :{x :Str}` when one does not fit — and, for a **quantified** callee, solves
the group from that collector itself; a type parameter the caller writes into
the record names no parameter, and misnames the call. A **`:Type` parameter** —
a type-channel parameter that is no `FOR ALL` name — is an argument like any
other: the frame binds it to the type value the call passed, by keyword or by
name. Either way each `FOR ALL` slot is then bound to a type value holding its
solution, looked
up **by its own name** through the callee's
[quantifier map](../knot/README.md) — the slots arrive symbol-sorted, not in the
order the `FOR ALL` group was written. A name the map says canonical form dropped
binds that variable's bound. A callee that is no function, an argument record
that misnames a parameter, an argument that does not fit its parameter, and a
group the arguments cannot solve are each a
[fault](#faults-and-output). Born as `KBirth::Block` it lays a block's
activation down beside the view it sits in, in the asker's own cell, and yields
its last statement's value.

It performs the units in the order the shape emitted them
([Units](../scope/README.md#units)), each after every unit it reads, over a
depth-first drain that finishes each before the next begins. So every read
finds its slot bound, a tie never names a pending binder, and a program whose
statements have effects and no data dependency takes source order without
interleaving. Per unit:

- **A component of type binders** is bound to the handles the
  [load pass](../elaborate/README.md#the-type-channel-at-load) fixed for it when
  they are closed, and otherwise declared through
  [the elaborator's door](../elaborate/README.md#declarations), over the
  activation, and bound in the same step.
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
- **A statement that binds nothing** is one evaluation, asked with `Reads` —
  save a frame's last unit, which the runner [tails](#frames-contracts-and-tails)
  into, and a block's last statement, asked with `Forwards` so the value is
  built where the block's result goes. A body whose last unit is a binder
  yields that binding's value.

The only children the runner asks for are evaluations, through
`Program::evaluate`, each handed the view of the runner's activation. The
placement is the level's: at the top level an evaluation is a `Fresh` tree
child, so everything it allocates beyond its value dies with it while the value
is built in the root, its home, from the start — or crosses into it at the
verdict's price, splicing its region in when pinned and reclaimed when copied.
In a called body an evaluation is a `Shares` tenant of the frame, so what it
builds is at the frame's `'here`, binding a slot is a plain write, and a reader
reads it where it lies.

Transients inside a step — the parts a tie named, the values supplied back to
it — go on a local bump for that step, never a `Vec`.

### Frames, contracts and tails

A called frame owes its caller a value satisfying its callee's declared return,
with the frame's own type-parameter solution substituted: a
[`Contract`](record.rs), which names the callee so a miss can say whose return
it was. The frame's value is **retyped** to the declared return — a container to
the declared type, a tagged value to the union member naming its constructor —
and a value that does not satisfy it is a fault
(`:(FN :{} -> Number) returned Str, which does not satisfy Number`).

When a frame's last unit is a statement that binds nothing, the runner does not
wait on it: it **tails** into the evaluator, a `Shares` successor in the frame's
own place, handing it the contract, and the evaluation owes the frame's caller
the value. An evaluation whose selected call's declared return satisfies the
contract tails again, into the callee's frame, so a tail recursion N deep holds
a constant number of cells; any other value is held to the contract where the
evaluation finishes. An `EVAL`'s frame owes no contract, and a block never
tails.

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

**The placement bit.** [`call`](body.rs) is what an evaluator asks for to call a
function: a frame running the callee's body, placed by the bit its return type
derives. `placement_of` makes a return of `Number`, `Bool` or `Null` `Fresh`,
since no value of those shares bytes with an argument, and every other return
`Shares`. A builtin's native step states its own placement in the request it
makes.

**Running code.** [`eval`](body.rs) is what an evaluator asks for to run code
under `EVAL`, beside `call`: a `Shares` frame running the code's shape, which
was built where the program loaded, so nothing builds a shape at run time. The
evaluator hands it the names the `EVAL` offers, read at the coordinates the
shape recorded for its operand (`BodyShape::offers`), as a record. It refuses
before anything is spawned, with a `CodeRefused`: `Shape` for code whose shape
kept an error, and `Unbound` for the first name hole no `USING` filled, `\` name
the `EVAL` does not offer, or keyworded hole some use in the code selects from
alone. A keyworded hole nothing filled that every use of it can do without binds
the empty list of functions. That the operand is code at all is what `EVAL`'s
overload admitted. The frame
reads nothing of the scope the `EVAL` is written in but those names, so code
fills no hole from the frame that runs it
([building code](../scope/README.md#building-code)).

## The scheduler is a view

A [`Scheduler`](../scheduler/README.md#the-drain) borrows the graph and owns
none, so `run`, `inspect` and `Running::scheduler` each make a fresh drain over
the substrate's graph for the length of one call. A call whose drain stalls is
accepted as it stands: the view drops what is on its stack and the graph keeps
the cells already born, under the root they were born under. A stalled
substrate still runs a later root work, and is otherwise only ever dropped.

## Imports and tests

Outside `#[cfg(test)]` this module names `crate::elaborate`, `crate::knot`,
`crate::memory`, `crate::parse`, `crate::scheduler`, `crate::scope`,
`crate::symbols`, `crate::type_lattice` and `crate::values`.
`tests/boundary.rs` reads the source to hold the rule there.

The suites run over the [miniature evaluator](tests/evaluator.rs), which records
what it evaluates and where it builds. [`tests/programs.rs`](tests/programs.rs)
runs whole programs under the drain: independent statements in source order
without interleaving, a forward capture running its binder first, more
statements than the slab cap, a binding built in the root from the start, a
frame's result crossing into the root at the verdict's price, a called body's
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
return that misses it, a self-call in tail position holding its cells constant,
and one whole program with all of them.
[`tests/substrate.rs`](tests/substrate.rs) loads two programs through a helper,
moves them into a `Vec`, runs each, and reads a binding back in a separate call
through a resumed root work; it also checks both load errors, an inspection
before the program ran, and a stalled substrate. The two-program test,
`a_whole_program`, `an_eager_part_is_supplied_by_site_in_one_wake`,
`a_call_binds_each_type_parameter_to_its_solution`,
`a_lambda_returned_from_a_frame_keeps_its_captures` and
`a_self_call_in_tail_position_holds_its_cells_constant_however_deep` are on the
[Miri slate](../../observe/miri_slate.md).

## Open work

- [Faults and call traces](../../roadmap/rewrite/faults.md) — faults apart
  from values, the frames they keep, and the trace an uncaught one prints.
- [Catching errors](../../roadmap/rewrite/catching.md) — `TRY` and `CATCH`,
  which build an `Error` from a fault and release its frames.
- [A refused program stays loaded](../../roadmap/rewrite/refused-programs-stay-loaded.md)
  — a refused load kept, so its shape error renders on demand rather than once.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — a
  receipt run laid down anew per park, and the bytes a `Shares` tail hop leaves
  in its host.
