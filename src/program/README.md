# The program

A loaded program as one owning value, and the body runner that performs it.
[`CellSubstrate`](substrate.rs) holds program storage, the symbol interner, the
type registry, the program's record and the
[cell graph](../scheduler/README.md) that runs it, and it can be returned from a
function, moved, kept in a struct and dropped like any other value. An embedder
that keeps a program across calls — a language server, a REPL — keeps one of
these, and repeats no setup order of its own.

The layer above supplies what a program *means*: a [`Language`](record.rs),
which lays the builtin table down and names the step every evaluation runs.
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
`L` lay the builtin table down, builds the program's shape over that table,
takes the root, and lays the record down. It returns a `Result`, and
`LoadError` carries the parse error or the `ShapeError` that stopped it. The
table comes before the shape because a shape resolves a builtin name to an
index into it.

## The program record

[`Program`](record.rs) is what a loaded program's steps read, at `'graph`: the
top level's shape, the builtin table, the registry and interner, and the
evaluator's step. It rests in program storage beside the table, written through
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

**`Language` is a trait**, with two methods generic over `'graph`: `builtins`,
which lays the table down through program storage's writer, and `evaluator`,
the step an evaluation runs. A record of function pointers ranked over `'graph`
cannot name the bundle's projections (rustc #100013); a method generic over
`'graph` is handed the one the program loads at. Dispatch implements it; the
tests implement a [miniature](tests/evaluator.rs).

## The bundle

[`KBundle`](bundle.rs) is the [step bundle](../scheduler/README.md#the-continuation)
a program's steps run over, with its three families:

- **`KBirth`**, what a cell is born holding, which crosses and is covariant:
  `Program`, the top level's root work; `Call`, a callee and a record of its
  arguments by name; `Evaluate`, a node and the view it is read through; and
  `Inspect`, the view the top level leaves at rest. The family's `covariant!`
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
arguments are deep-copied through
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

## The body runner

[`run`](body.rs) is one step that performs the top level and every called
body. Its state is the activation, the next unit, and the stage it parked at.
It claims nothing ahead, ties and binds each unit itself, and no unit has a
cell of its own: a cell per unit would cost a create, an enter, a release and a
receipt slot per statement per call, for statements that mostly never park.
Born as `KBirth::Program` it lays the top level's activation down; born as
`KBirth::Call` it lays the callee's activation down in the frame's own cell and
binds each value parameter from the argument record, which must name them
exactly. A **quantified** callee's frame first solves its group: every declared
parameter type against the argument's carried type, under one collector, and
each type-parameter slot is then bound to a type value holding its solution,
looked up **by its own name** through the callee's
[quantifier map](../knot/README.md) — the slots arrive symbol-sorted, not in the
order the `FOR ALL` group was written. A name the map says canonical form dropped
binds that variable's bound. A group the arguments cannot solve binds nothing and
refuses the call, as an argument record that misnames a parameter does.

It performs the units in the order the shape emitted them
([Units](../scope/README.md#units)), each after every unit it reads, over a
depth-first drain that finishes each before the next begins. So every read
finds its slot bound, a tie never names a pending binder, and a program whose
statements have effects and no data dependency takes source order without
interleaving. Per unit:

- **A component of type binders** is declared through
  [the elaborator's door](../elaborate/README.md#declarations) and bound in the
  same step.
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
- **A statement that binds nothing** is one evaluation, asked with `Forwards`
  when it is a called body's last statement — so the value is built where the
  frame's result goes — and with `Reads` otherwise. A body whose last unit is a
  binder yields that binding's value.

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
it — go on a local bump for that step, never a `Vec`. A tie or a declaration
the runner cannot proceed on, a callee that is no function, or arguments that
do not name its parameters exactly end the step with `StepError::Refused`.

**The placement bit.** [`call`](body.rs) is what an evaluator asks for to call a
function: a frame running the callee's body, placed by the bit its return type
derives. `placement_of` makes a return of `Number`, `Bool` or `Null` `Fresh`,
since no value of those shares bytes with an argument, and every other return
`Shares`. A builtin's native step states its own placement in the request it
makes.

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
and one whole program with all of them.
[`tests/substrate.rs`](tests/substrate.rs) loads two programs through a helper,
moves them into a `Vec`, runs each, and reads a binding back in a separate call
through a resumed root work; it also checks both load errors, an inspection
before the program ran, and a stalled substrate. The two-program test,
`a_whole_program`, `an_eager_part_is_supplied_by_site_in_one_wake`,
`a_call_binds_each_type_parameter_to_its_solution` and
`a_lambda_returned_from_a_frame_keeps_its_captures` are on the
[Miri slate](../../observe/miri_slate.md).

## Open work

- [Dispatch](../../roadmap/rewrite/dispatch.md) — the evaluator itself, and a
  runner refusal turned into a koan error value rather than a stalled drain.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — a `Call`
  birth copied by a tail hop, and a receipt run laid down anew per park.
