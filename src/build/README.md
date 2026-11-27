# build

Where a program's text is decided: the load, the one walk it runs, the store
that walk keeps its unit and closing records in, the folds over them, and the
fold of those records into the text of the base's
[binding graph](../../design/binding-graph.md). What a binding the load could
not see decides is a **`BoundInstance`** of that graph: made by evaluating
the questions the load stored, never by typing the text again. The load makes
the instances its text reaches from the root, and a run cell makes the rest
as content arrives.

This module owns what the walk's records are folded into: the load's store
([`LoadStore`](load.rs)), the folds that lay caching records, contracts and
the names each entry and statement seam binds ([`fold`](folds.rs)), and the
graph's text and the instances made of it ([`lay`](lay.rs)). The solver's
driver is [the one walk](#the-one-walk): one fact function per unit over a
shape built from text alone. Its neighbours keep what they own:
[elaborate](../elaborate/README.md) the per-expression typing rules and the
walk's [type half](../elaborate/README.md#the-type-channel-at-load);
[scope](../scope/README.md) the shapes and name resolution, and the facts'
vocabulary — unknowns, frontiers, a binding, the `Store` trait, the laid
layer `BuildLayer`, a composition's facts, the stored site question and the
rigid answer, and the read door `Facts`;
[dispatch](../dispatch/README.md) selection, the walk's driver and value
half, the evaluator reading what the walk recorded, and koan's
[fact domain](../dispatch/graph/README.md) over the graph;
[`bindgraph`](../../bindgraph/README.md) the graph itself and where its
instances rest; [memory](../memory/README.md) program storage, which holds
the parse, the builtin table and what the load writes for the written
program; [program](../program/README.md) the load door, the composition door
and the frame's bind sites; the [scheduler](../scheduler/README.md) the
depot of compositions, its budget and its eviction.

## Where facts are decided

- **The load** runs once, before the graph of cells stands, over the written
  program. It writes the shapes, its base layer — every unit record and every
  body's closing record — the folds over them and an index of every closed
  content tree they compute, all in program storage, and the **frozen
  load**'s types, laid in the owner's registry and read by everything after;
  the agent tier is laid beside it at the load's end. It then lays the
  program's binding graph and makes the instances its text reaches from the
  root, frozen with the load. A refusal in one of them refuses the load
  ([refusal](#refusal)).
- **A composition** is a depot slot holding the shape of code composed at
  run time, the facts that wait on nothing but its syntax and the binding
  graph laid over them: a chain root in the one-chain sense of
  [quotes](../scope/README.md#quotes), built from the code alone over a copy
  of its syntax and keyed on the syntax digest
  ([`composed`](../program/compose.rs)). The drain finds it by digest at each
  `EVAL` of the code and each call of a callable born in it, rebuilding it on
  a miss.
- **A run cell** reaching a bind site finds or makes its node's instance
  under what its slots bind, through the instances of its base's graph
  ([instances at run](#instances-at-run)).

Each chain has one **base**: the load for a chain under the root, a
composition for one under it. Written code has one base, whose segments are
program-long; composed code adds one per distinct syntax, its segments
numbered per shape. Segments and sites compare within one base alone. No
depot slot rebuilds shapes: written code's shapes live in program storage,
composed code's in its composition. The raw parse stays in program storage.

**Lingo.** A **type fact** is a
[load-time type](../scope/README.md#load-time-types) or verdict, and the
solver's grain is the **unit** that computes it: data the build writes in its
unit record, in program storage for the written program and in a
composition's bump for composed code. A **depot slot** holds a composition; a
**run cell** is a cell of the graph of cells. An unqualified "cell" or "node"
is avoided.

## Type facts and the solver

A type fact is a static type as an interval, or a verdict, and the typing
rules are monotone: law 6 of the [lattice](../../lattice/src/types/laws.md)
makes every rule map both ends, so a fact only narrows and a verdict moves
only from *maybe* to *always* or *never*. A fact is asserted only where it
holds under every refinement of the unknowns it reads.

### The one walk

A [shape](../scope/README.md#units) is a function of the text, the builtin
table and the operator claims, and lays each body out in **units**: its
**entry unit**, which binds every slot the body holds before a statement
runs — its parameters, its own `FOR ALL` group, its `:Type` parameters, a
block's surfaced names, a code shape's captures — then one unit per statement
or component, in the order the run takes them. One **fact function** computes
every fact of a unit, in five steps:

1. **paths** — each path the unit's statements spell that its chain has not
   resolved yet, an inline root typed there once the paths its own types
   spell are resolved;
2. **declarations** — a component of type binders, through the declaration
   door;
3. **type expressions and registrations** — each type expression and guard,
   each registration the unit binds with its overlap check, and the type of
   each callable the unit births;
4. **value statics** — each statement's node, a component of value binders
   seeded first; a quote's code the unit births is walked before them, so an
   `EVAL` reads it typed;
5. **binder facts** — each binder's static type, and each module binder's
   content tree.

A reader that meets a path the walk holds no resolution for stops its fact
with a request: the walk resolves that path, refusing one met while it is
itself being resolved, and types the fact again, dropping what the stopped
attempt gathered. So no fact is typed over a path the walk has not resolved.

A path's root lies in an earlier unit, in the unit's own component as a
fellow read at its seed, or is written in place, so every read in the five
steps is positional. An **inline body** — an arm, a block, a `USING … SCOPE`
body, a `MODULE` or `GROUP` body — runs its entry and units through the same
steps inside the unit that runs it, whose unit record holds its facts. A
callable is a unit sequence of its own, walked right after the unit that
births it; a quote's code is one too, walked inside its birth unit. Each
**owning body** — the program, a callable, a quote's code — ends in a
**closing record**: its return check, exits, class, summary and recursion
flag, and for a quote's code the type an `EVAL` runs it at and its
typing refusal. Written code's refusal stops the walk; a quote's code that
does not type keeps its refusal in its closing record, and the `EVAL` running
it reports it.

The type half's rules are [elaborate's](../elaborate/README.md#the-type-channel-at-load),
the value half's and the driver [dispatch's](../dispatch/README.md#static-types).
A unit's facts rest in its **unit record**, off the shape, which the run and
the suites read through [`LoadFacts`](../scope/README.md#load-time-types). A
unit's steps walk only its own entries, which the shape lists
([`holds`](../scope/README.md#units)), and the walk logs where each unit
writes into its owning body's facts, so each unit record lays exactly those;
an inline body's facts are laid whole as it ends. So the load is linear in a
body's size, never a scan of the whole body per unit: a path whose root is
written in place (`(ID b).Carrier`) names its statement, and a part's source
is looked for in the statement the walk is typing first, the whole body only
where a lazy read lands elsewhere.

### Recursion

A call **unfolds** into its callee's summary only where the callee's strongly
connected component, over the calls selection keeps, has closed and is
trivial. The walk computes the components by Tarjan's bookkeeping on its
demand stack: a callable enters it as its body is walked, a kept call into a
callable on the stack lowers the caller's link and unfolds nothing, a kept
call into one not yet walked walks it there, and a body whose link is its own
index closes its component, marking every member recursive where it has more
than one or calls itself. So only a closing record is read on demand, and
whether a call unfolds never turns on declaration order. A recursion the
graph's calls compose into a type built over itself is refused when the graph
is laid ([refusal](#refusal)).

### The store

The walk runs over one trait, [`Store`](../scope/facts.rs): the unit record of
a unit and the closing of a body, each beside its frontier, the writes of a
unit record and a closing, the derived names it lays, and the wording of a
refusal. It interns through a chain its caller hands it. The store records no
edge: the walk reaches it only through its façade, which keeps the gathering
of each open unit. A read of an unknown adds it to the innermost gathering,
and a read of an
ended unit record or closing adds one read of its frontier, cut to the
reader's chain ([the cut](#frontiers-and-seams)); only an unknown declared on
the reader's chain is gathered. A unit's **frontier** is its gathering when it
ends: its own unknowns, each beside its mark ([stored
questions](#stored-questions)), sorted and deduplicated, a named mark dropped
where the same unknown holds an open one, beside each frontier it read, once
at its widest cut and in the order first read
([`Frontier`](../scope/facts.rs)). The unknowns it depends on, transitively,
are its own and each read's past its cut. A read whose cut keeps none of its
frontier's unknowns is dropped, so a frontier is empty exactly where it holds
no unknown. Nothing lays them flat, so a read costs one entry however many
unknowns its frontier holds.

Every value a unit or closing record keeps is laid where it rests, through the
store's `lay`, so nothing is relayed; transient work stays in the walk's
scratch.

[`LoadStore`](load.rs) is the load's, and a composition's: the walk runs from
its base's root, every unit once, and the store lays every unit and closing
record in the storage it is handed. In tests it counts each unit a walk types,
by the shapes it walks ([tests](tests.rs)), so a test can hold the load to
typing each unit it reaches once and no run to typing one again.

**One laid layer.** What the walk lays is one type wherever it rests,
`BuildLayer` ([load-time types](../scope/README.md#load-time-types)): its unit
records and closings beside their frontiers, the held-fact index, the closed
trees it computed, the refusal it reached, and under a composition the terms
its unit and closing records name. One lay, `BuildLayer::of`, lays it from
the walk's sorted writes. The load's **base layer** rests in program storage
beside the shape, the folds and the closed-tree index, freed with the
program; a composition's rests in the composition's own storage.

### Stored questions

Every judgement over a run-bound name goes through a **door**
([`judge.rs`](../elaborate/judge.rs)), so a unit record is a function of the
text and its door answers. A door hands back a discrete answer — a verdict, a
bool, a kind, a stamp, the branch a rule takes — or a type written over the
names: the variable itself, a
[projection](../../lattice/src/types/vocabulary.md#the-node-vocabulary), or a
[derived name](#derived-names); never a type read off a variable's end. Each
door answers by the **rigid answer** ([`rigid.rs`](../scope/rigid.rs)), one
rule per question kind: the closed door's answer where every type it judges
is closed, else an answer only where no binding within the names' ends
changes it. Each unknown a unit gathers is **open** or **named**: open where a
door judged it and reached no answer that holds at every binding — a *maybe*
verdict, a fit that may fail, a branch the binding decides — and named where
the unit record only names the variable.

A rule with a discrete door is laid whole as a **stored site question**
([`site.rs`](../scope/site.rs)) in the record of the unit owning its site: its
**asks**, the questions it may ask, written over names; a **decision
function** from their answers to its outcome, a `SiteAnswer` that holds no
type; and one **payload** per outcome, what the outcome writes, laid over
names — a candidate's type, an arm's, a refusal's template. The walk runs the
decision function with each ask answered rigidly, and an instance runs it with
each ask answered under its binding, so the load and an instance cannot
drift. A refusal keeps the types it was judged over the same way, and is
worded by reading them, the binding written in. No statement is typed again
under a binding: a question is the grain of work after the load.

### Unknowns

Every lexical variable is an unknown, a `USING … SCOPE` block's surfaced names
and a quote's holes, `\` marks and `$` names among them, and so is a module
binder: every fact reading a module binder's content tree — a view, a path
resolution, an unfold over it — waits on it. A [derived name](#derived-names)
is no unknown: a fact reading one waits on the names its term reads. A binding
gives a parameter's content, which is its path class
([paths](../dispatch/README.md#paths)), a `FOR ALL` group's solution and a
`:Type` argument at frame entry; a lexical variable's type at the statement
that binds it; and a path variable's hop at the statement that binds its
root. A surfaced name binds at its `USING` statement, which surfaces it as
the statement runs; a quote's code is the seam of its own holes,
marks and `$` names. A module binder binds to the exact type it carries: for a
parameter, the application of its declared signature the retype stamps, every
head parameter pinned. A value's carried type is never an unknown: a static
type that reads no unknown is final at the load, however wide.

An unknown is named by its **lexical address**, the segment and the
coordinate the shape binds it at, never by its lattice node, which every body
numbering its chain alike shares. A unit's own binders are what it fixes, not
unknowns it waits on. A mention of a fellow member of its own
component, itself included, in the component's own statements or in a block
or `MODULE` or `GROUP` body they run inline, is no unknown: it reads the
fellow's annotation, the type its birth declares — at most `Module` for a
`MODULE` or `GROUP` body — or `[Never, Any]`, records no edge, and so reads
the same whatever order the members are typed in. A callable body nested in a
member is walked after the component and reads the fellow's fact. The facts
form no cycle outside a component.

### Derived names

A rule may need a type written over run-bound names that no lattice node
spells: a piece of a variable whose bound does not guarantee it; the result of
a read or a call whose case a binding decides; a meet; a nominal
declared over a run-bound name; a solve a run would not reproduce; the type a
module binder's content carries. Such a type
is a **derived name**: a rigid variable named by its term's content, between
closed ends that hold every binding's type, beside its **term**, which is the spelling
or the closed rule it stands for, written over the names it reads. A variable's
bound holds no opaque carrier, so an end that would hold one is widened past it:
the lower end to `Never`, the upper to `Any`; the carrier stays in the term. A binding
gives a derived name the type its term gives, applied by the closed rule a
load with that binding declared runs at its site, so a unit record naming one
is read under any binding with no walk, and a mark on one is a mark on each
name its term reads. Under

```koan
EXPR FOR ALL #[Elt] #(AT xs :Elt) -> Any = #((xs 0))
PRINT (AT [5, 6])
```

the read asks which container `Elt` is bound to, and types the read as a
derived name decided by that question: on its list answer the read's type is
the piece `(ELEMENT OF Elt)`, so `Elt := LIST OF Number` gives it `Number`.

A rule hands back the same type at load and under every binding: closed where
its operands are, else written over the names, or a derived name. A type that
depends on a question's answer is a **decided** derived name: the question,
beside one type per answer, each written over names by the load, so a binding
gives it the type on the answer the closed door gives. A type no question
decides, a piece or a meet, is typed by the rule's closed function over its
operands. The answer chooses only a discrete outcome beside: a refusal, which
candidate a call selects, which sub-rule runs. A call that keeps several
candidates under a binding is typed by one decided name per candidate,
joined, so it gives exactly the survivors' join.

A derived name is named by its term's content, as a class is, so two sites
retyping to `Elt` share one name and every walk that meets a term names it
alike. The walk lays each name it meets once, in its base's layer. A binding
gives a derived name what the rule gives its term with the binding written
in: a type where the binding closes the term, else the name of the term it
leaves.

A module binder's binding is written into the path variables of its classes
(**module substitution**), and its tree over each content tree that is a leaf
on the binder (**tree substitution**). So a path or a view over a module
binder is named, and a binding of the binder writes its content in.

A read of a module binder whose content the run builds is named by what its
content tree reads through: each leaf on a module binder; each call an answer
selects among, by the `LET` binder holding it and the names its candidates'
types read; and the binder at the tree's root, whose binding gives the tree
whole. A call's candidates are judged with the binding written in, and the
one selected unfolds as a load with that binding declared unfolds it; where
none is selected, the holder's binding gives the call's content. Two `LET`
binders holding one call hold one tree, so a binding of either gives both
that content. A path whose tree holds a leaf no binder names is the class of
the path through its root, as a parameter's path is
([paths](../dispatch/README.md#paths)). A module binder the run builds whose
static type is not one type over the names its content reads through — itself,
its tree's names and the other holders of its tree — is typed, as a `:Module`
parameter is, by the derived name of the type its module carries, under its
static type's upper end, so every judgement over it is a question a binding
answers.

A derived name is a lattice node, a rigid variable beside the lexical and
path variables, which every relation treats as a lexical one. The lattice
holds its term's digest, never its term: `substitute`, *fits*,
`judge_by_class` and the unifier are what the rules call, and this module
lives downstream of them.

## Frontiers and seams

- **A frontier.** Each unit record carries its frontier: the unknowns its
  facts read, each named or open ([stored questions](#stored-questions)), and
  a read of the frontier of every unit or closing record they read, cut to its
  chain. Frontiers share structure, a DAG at unit grain: a chain of k units,
  each reading the one before, lays O(k) entries in all. A unit whose
  frontier is empty, holding no unknown, is final at the load.
- **The cut.** A read keeps the unknowns of its frontier whose segment is at
  least the cut's: those declared in the cut's body or a body around it. It
  rests on three facts. A body's nested bodies take the segments before its
  own ([`Segment`](../scope/shape.rs)), so along one chain segments fall from
  the root inward. A frontier's unknowns lie on the chain of the body whose
  unit or closing record holds it. And the cut lies on that chain too: it is
  the innermost body on the reader's chain enclosing that body, and as a body
  leaves the chain, a read cut inside it is cut at its encloser instead.
  Segments compare within one shape table, and no frontier spans two.
- **A seam.** A unit's seam is the latest **bind site** among its frontier's
  unknowns ([`Shapes::bind_site`](../scope/shapes.rs)), sites ordering as the
  binding does: an outer body before an inner one, and in one body its entry
  before its units in order. A parameter, `FOR ALL` name, `:Type` parameter or
  a quote's code's capture binds at its body's entry; a local or a type binder
  at its unit; a block's parameter at the unit holding the block. No list of
  seams is stored: [the folds](#the-folds) read them off the frontiers, each
  frontier's latest bind sites read once, through a memo shared by every
  frontier that reads it ([the folds' tests](tests/folds.rs)). A unit that
  reads an entry-bound unknown and a later-bound one seams at the later site.
- **Code finishes building before it runs.** A body's call never precedes
  the binding of a variable it captures: the shape orders each unit after
  every unit that binds a slot it reads, a deferred mention waits on its
  binder, and a component with an eager mention of its own member is refused.
  So a frame never waits on a binding its own continuation would make.

## The folds

Once the walk ends, the load folds its unit and closing records
([`fold`](folds.rs)), a pure function of the shapes and those records. Every
input a fold needs from the lattice the walk records, so the folds read no
type:

- **Caching.** Each body's caching record says whether its frame caches, as
  its closing class says
  ([where each body interns](../../design/type-tiers.md#where-each-body-interns)).
- **Entry and seams.** The parameters a callable's, a block's or a quote's
  code's entry binds are those some unit beneath it holds in its frontier, its
  entry aside, whose seam is that entry; the slots any body binds at a
  statement that any frontier holds are its statement seams.
- **Contract.** Each lexical variable a contribution or a solution names is
  composed by every callable, `MODULE` body or quote's code between the body
  reading it and its home, marked in that body's
  [layout](../scope/README.md#load-time-types). A quote's code also composes
  each variable some unit inside it holds in its frontier, so its entry binds
  what its value carries.

## The binding graph's text

Once the folds are laid, the load reads its records as the description the
[binding graph](../../design/binding-graph.md) is laid from
([`lay`](lay.rs)), lays it through [the crate](../../bindgraph/README.md),
and keeps the facts the graph needs beside it (`GraphFacts`). A composition
does the same over its own records when it is composed.

- **Bodies** are the bodies that own units: the program, each callable and
  each quote's code, nested as their shapes are. A body's statements are its
  units in order, then one for its closing record. Before a unit's statement
  come those the run binds first: one for each part the unit holds in place,
  inner first then left to right, then the inline bodies the unit runs. An
  inline body — a block, a `MODULE` body — is laid in its encloser as an
  owning body is: a statement for its entry, then one per unit, each after
  the parts it holds. So no statement binds two units' names.
- **Site names** are the unknowns a question or a call's mapping reads: a
  type name by its level, and a module binder's two names, the type its
  module carries and its content. Each binds where its unknown binds. A
  derived name other than a decided one is no name: a question reading it
  reads the names its term is written over. A path class's variable is no
  name either: a question reading it reads the names its recipe reads, the
  content name only where the class reads the binder's identity, under a
  view or another tree over the binder.
- **Answered names** are the decided derived names, one per segment reading
  each: its deciding question laid at the segment's closing, and an
  `AnswersName` edge on each of its arms giving that arm's type.
- **Questions** are the stored site questions some binding may decide
  otherwise: each ask with no answer at every binding is a `NameQuestion`
  over the names it reads. A question whose every needed ask holds one answer
  at every binding is decided by the text, and laid as no question.
- **Calls.** Each candidate of a use's selection question passes its callee
  what its laid mapping says, on the candidate's arm; a call the text
  selects passes it at the call's site.
- **Holders** ([`held`](lay/held.rs)). A `LET` binder whose right-hand side
  is a call, or an `EVAL` of a module's kind, is a **holder**: a bind site of
  its own, which owns the questions reading it. So is a part held in place,
  the call or `EVAL` written inline. Where an answer through the call gives
  its type, its **arrival** — a callee's result, an answered name typed
  through each of the call's edges, or a closed term where the text selects a
  callee whose result is closed — is what an **onward call** on the holder's
  statement gives it, so the instance that follows the call makes the
  holder's too. Where none does, the run binds the holder as the value lands.
- A body's entry is **open to callers** outside the text where its name
  escapes, where it is a quote's code, and where an `EVAL` offers it to the
  code it runs. A quote's code is **staged**: what it asks is owned no
  earlier than its entry, where an `EVAL` runs it. Composed code's own body is
  open to callers inside a root that binds nothing.

The answer function is the stored question's own decision function, each ask
a binding may change asked through the crate's doors and every other answered
as at the load. A **layout** places each node's handle in its body's slab,
the words a frame of the body holds.

## Instances at run

Each base's instances are kept beside its graph (`Instances`): the crate's
`Bound` over the frozen part the load made, its views kept in a bump lent it,
and its answers resting there within a budget. The program's are the agent's;
a composition's rest in its depot slot and are dropped with it. A step
borrows the instances of its base's graph through its door for one call at a
time (`BaseInstances`).

A frame holds one slab of handles, one per node owning a question in its
body, at the width the load fixed. As it reaches a bind site — its entry, a
holder, an `EVAL`'s result, a `USING`'s surfaced names — it reads what its
slots and captures bind ([`instanced`](../program/body.rs)), finds the
node's instance filed under that view or makes it by the evaluation the load
runs, and writes its handle into the slab. Its readers read a discrete answer
through the handle, and close a type of a base record by writing the frame's
binding in; what a binding closes a type to over content that lasts as long
as the load is kept beside the instances, so a loop closes it once. A
statement seam makes no instance, and neither does a part held in place that
no node sits at: at each the frame takes its binding anew from its slots and
held slots, so a type read through the part closes under what the part
holds, as it does through its `LET` spelling's binder. A
closure carries the handle words of each frame around it as they stood at its
birth, so a frame answers a question an encloser's node owns through those
words.

## Refusal

Refusal is as eager as possible and lazy only where the input arrives at run
time.

- **Eager:** a closed type that does not elaborate, a shape error and an
  overlap refusal in written code refuse before anything runs. A written
  quote's code is elaborated eagerly and its refusal, or its overlap,
  recorded, then reported where an `EVAL` runs it. Laying the graph refuses a
  recursion whose calls compose into a type built over itself, naming the
  `FOR ALL` name, or the name declared `Any`, it grows, and asking for a
  declared recursive type. An instance the load makes that answers a question
  with a refusal refuses the load: the refusal is worded under that
  instance's binding, then names the call that entered it on a line of its
  own, `in <call text> at <location>`, as a faulting call's frame line names
  its call.
- **Lazy:** a run cell's instance that answers a question with a refusal
  faults the frame at the bind site that owns the question, worded under the
  frame's binding as a load with it declared words it; and a composition
  checks its syntax when it arrives, keeping its refusal so every frame born
  under it faults alike.
- **Which refusal.** Within an instance, the refusal a load walking the text
  meets first (`GraphFacts::refusal_order`); across the instances the load
  makes, an instance's own before its callees', and among callees the one
  whose call the walk meets first.
- A refused written program's step stops at the refusal, which is rendered
  while its names and types still stand.

## Lifetimes

| holder | lives from | to | holds |
|---|---|---|---|
| program storage | the load | the program's end | the shapes, the base layer, the folds, the closed-tree index and the program's binding graph with the instances the load made; the frozen load beside it holds the root tier |
| the agent's instances | the load | the program's end | the instances run cells make of the program's graph, their answers within a budget, and the types a binding closed over lasting content |
| composition | the first `EVAL` or call missing on its syntax | its eviction, once no frame holds it | a copy of the code's syntax, its shapes, facts, folds, terms, graph and instances |
| run root | the load | the program's end | top-level bindings |
| run cell | its birth | its disposal | its frame's slots and slab of handles, values, region branch, scratch |

## Properties

Each is a law in the dispatch suite's property tests, generated over programs
and bindings; a fresh load of the program with the binding declared is the
oracle.

- **A stored question decides as the walk did.** At the load, each stored
  question's decision function under each ask's rigid answer gives the
  walk's outcome, its route included ([stored](../dispatch/tests/stored.rs)).
- **A stored question decides as a fresh load.** Under a binding, a stored
  question decides as the fresh load with the binding declared does at its
  site, and its taken arm's payload, applied, is what that load fixes there.
- **The rigid answer is every binding's.** A rigid answer holds under every
  binding within the names' ends ([graph](../dispatch/graph/tests.rs)).
- **One owner.** Each question is owned at the latest bind site among the
  names its reads depend on, and inside a quote's code no earlier than the
  code's entry ([owners](../dispatch/tests/owners.rs)).
- **An instance answers as the stored question.** Each instance the load
  makes, from the root and through every call and onward call it follows
  ([made](../dispatch/tests/made.rs)), and each a frame makes at a bind site
  under a binding the law stages ([framed](../dispatch/tests/framed.rs)),
  answers every question its node owns as the stored question decides under
  the names the instance binds.
- **A run's instance answers as the stored question.** Running a generated
  program that calls `SUBJECT` with values, each instance the run's frames
  make at `SUBJECT`'s bind sites answers every question it owns as the stored
  question decides under the type names the frame bound off its own slots,
  captures and held slots. The draws include a call passing a module where
  a `FOR ALL` name is declared, so the run binds that name to a signature
  type. A program the load refuses runs no frame; the stored laws hold its
  refusal.
- **Monotone.** Binding more of a program's names, one at a time in any
  order, only narrows each ask of a stored question: an answer reached with a
  name open holds for every binding of it, with one carve-out: once a name is
  bound to a type no value carries, a verdict on an ask that reads that name
  may go from *always* to *never*, since no call over such a value ever runs.
- **Confluent.** Making a frame's instances along a widening chain of
  bindings, its names in any order, reads at every step of the chain as a
  fresh store making them under that step's binding at once: nothing a store
  filed before a step changes the step's answers.
- **No unit is typed after the load.** The load types each unit it reaches
  once, and a run types none of the program's: only a composition's own load
  types units, of its own shapes ([graphs](../dispatch/tests/graphs.rs)).
- **Levels are the text's, and path names their content's.** No unit
  record changes a level the shape numbers: a spelled path keeps the level
  the shape numbers it at, and a derived name, a class and a path no type
  spells are named by their content, so every walk names them alike. Two
  builds of one text agree on every text fact.
- **A derived name holds its binding's type.** Its term, applied to a binding,
  gives a type within its ends: the type a load with that binding declared
  gives its site, or the name of the term the binding leaves open.
- **Order does not matter.** Reordering independent declarations, or
  independent statements of a callable's body, changes no fact but where each
  statement sits.
- **Termination.** The load's facts range over the types of bounded depth
  over the root's types, a finite set: join, meet and the solve never deepen
  a type, and the facts form no cycle, since a parameter's and a return's
  static type come from the declaration, a local's from an earlier unit, and a
  mention inside a cyclic component is a leaf. Every `NameQuestion` has
  finitely many answers, so a node has finitely many instances whatever the
  run binds; a recursion that would make them without limit is refused. The
  depot's budget bounds the bytes of the compositions no frame holds.

## In the literature

The facts are a monotone data-flow framework (Kildall, *A unified approach to
global program optimization*, 1973; Kam and Ullman, *Monotone data flow
analysis frameworks*, 1977), solved in one pass in unit order rather than by
worklist, since outside a component every fact a unit reads lies in an
earlier unit; the components are Tarjan's (*Depth-first search and linear
graph algorithms*, 1972). A unit's frontier is the solver's answer as a
constraint it could not discharge (Pottier and Rémy, *The essence of ML type
inference*, 2005). The interval reading of a static type and the sources
behind it are in [gradual typing](../../design/gradual-typing.md#in-the-literature).

## The import rule

Outside doc comments and `#[cfg(test)]`, `build` names `memory`, `scope`,
`type_lattice` and `values`, and `source::SourceRef` for where a call is
written, beside the `bindgraph` crate, and nothing else in the crate
([`tests::boundary`](tests/boundary.rs)). `program` names `build`: its load
runs the language's walk into a [`LoadStore`](load.rs), folds the unit and
closing records, and lays the binding graph, and a composition does the same.
Nothing in `scope`, `elaborate` or `dispatch`'s walk names `build`; the walk
reads facts through `scope`'s `Store` trait, and the scheduler hands a step
its door and no more.

## Open work

- [What the agent tier keeps](../../roadmap/one-build/agent-tier-keeps.md)
  — where a type a frame closes stays warm across hops.
- [An exhaustive check](../../roadmap/one-build/exhaustive-check.md) — an
  opt-in load that also starts from every entry, keeping only refusals.
- [A module passed through an alias, a capture, a view or a member read](../../roadmap/one-build/captures-and-views-traced.md)
  — a call's mapping written through each, so the load makes the callee's
  instance.
- [A callee read by a member path or a call's result](../../roadmap/one-build/callees-read-by-path.md)
  — a callee the text does not name reached by a call's edge.
