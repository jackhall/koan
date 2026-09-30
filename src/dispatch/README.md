# Dispatch

The [`Language`](../program/README.md#the-program-record) koan's programs run
under: the builtin table a program loads over, the step every evaluation runs,
and the checks a loaded shape must pass, which type it statically as they go. [`Koan`](../dispatch.rs) is the one
implementation; the [program](../program/README.md) layer below it knows
nothing of what a node means, and the [binary](../main.rs) loads every program
under it.

What a keyworded use may select is fixed where its shape is built
([keyworded uses](../scope/README.md#keyworded-uses)); how two candidates rank
is a relation of the type lattice
([priority classes](../type_lattice/README.md#priority-classes)). Dispatch is
what runs between the two: it evaluates a call's slots, admits and ranks the
candidates by the arguments' carried types, and runs the one that wins.

## What dispatch runs

Literals, names, containers, record access, construction, functions, keyword
dispatch, the builtin library, quotes, `EVAL` and `USING` over code, `:!` over
any value but a module, and uncaught faults.
[Matching](../../roadmap/conditionals/matching.md) owns `MATCH`,
[catching errors](#catching) `TRY`, `CATCH` and `Result`, and
[module programs](../../roadmap/rewrite/modules.md) own the module expression
shapes, `:|`, and `:!` over a module; a node dispatch has no reading for is a
fault.

## What a node is

The [evaluator](evaluate.rs) reads a node off the shape that holds it — the
statements the shape owns, [rewritten](../scope/README.md#the-four-rewrites),
never the parse — as one of:

- a **leaf**: a literal, lowered; a name, read through its mention's
  coordinate; a quote, born through the [quote door](../knot/README.md#a-quote);
  a type expression, the type value of its
  [load-time type](../scope/README.md#load-time-types) — as it is when closed,
  its variables substituted with what their coordinates read when rigid — or,
  where the load left it unknown, elaborated through the view; a list, dict or record
  literal, lowered whole when every part is a literal and otherwise built from
  its parts' values;
- a **block** a pairwise operator run hoisted an operand into, run through the
  program's block door, whose value is its last statement's — which is how the
  shared operand evaluates once;
- a `FN`, born through the [lambda door](../knot/README.md#a-lambda), so a
  callable no binder names is born where it is evaluated, with the captures it
  reads there;
- an **ascription** `<value> :! <Type>`: the operand, evaluated, checked against
  the type its type part denotes — read as a type expression leaf's is — unless
  the load [settled](#static-types) it, and
  [retyped](../values/README.md#the-type-memo-and-satisfies) to it, so what
  follows dispatches on the ascribed type: after
  `LET Loose = :((LIST OF Any) | Null)`, `[1] :! Loose` carries `LIST OF Any`.
  A value that does not satisfy the type is a fault; a module operand is a fault
  until [module programs](../../roadmap/rewrite/modules.md) run its view door;
- an **`EVAL`** `<code> -> <Type>`: the operand, evaluated, run as code
  ([below](#running-code)) in a frame owing the type its type part denotes —
  read as an ascription's is — as a called frame owes its return. An operand
  that is no code is a fault;
- a **bucket declaration**, whose value is `null`;
- a **keyworded call**: a node the shape holds a candidate list for;
- an **application** `(head argument)`: a construction when the head is a type —
  a newtype, a family or a union variant, through `Tagged::construct`, the one
  construction rule — and otherwise a call by name of the head over the
  argument record.

A part a node needs is read in place when it is a literal, a name or a quote,
and otherwise asked for — a `Shares` tenant of the evaluation's cell, kept —
all at one park. **A fault is passed through**: an evaluation that receives
one from a part finishes with it, unchanged, before doing anything else.

## The builtin table

[`builtins::table`](builtins.rs) binds no value name. Its type names are the
lattice's builtin types and `Error`. Its **overloads** are function values —
each a [builtin node](../knot/README.md#a-builtin-overload) over the expression
shape it is registered at, whose `id` names its native — grouped by bucket key,
so every keyworded use at a key lists them first among its candidates. Every
overload ranks its slots in written order.

- `PRINT` over `Any` renders its operand, hands the text to the program's
  `print` sink, and returns the string it printed.
- `+ - * /` over `Number`, in IEEE arithmetic, and `< <= > >=` over `Number`,
  returning `Bool`.
- `AND` over `Bool` and `NOT` over `Bool`. There is no builtin `OR` and no unary
  minus.
- `==` over `Any`, structural; operands that cannot be compared are an error
  value. `!=` has no bucket: the shape rewrites `a != b` to `NOT (a == b)`.
- `|` and `&` over types, the infix pair and the unary list form an operator run
  of them is rewritten to, each returning a type value.
- The overloads of the [builtin expression shapes](../parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
  whose slots dispatch evaluates, typed as their entries type them: `ATTR` over a
  record or a tagged value over one, reading through every tagged layer at the
  representation its identity names to the field, which it hands back at the
  type the record's type names it at; over a type, giving the type its record declares the field with, so
  `Point.y` is `Str` and `v :Point.y` is a slot; over a union type labelled by a
  type name, giving the variant; and over a module, a fault until
  [module programs](../../roadmap/rewrite/modules.md). `FROM` restamps a record
  at the projection of its carried type onto the fields it names, each once,
  sharing the record's runs; a name that type does not name is an error value,
  as it is to `ATTR`. `USING` fills a code's holes through the
  [`USING` door](../knot/README.md#a-quote).

**Each native has a [type rule](rules.rs).** From its arguments' static types,
read at both ends, and each name operand as written, the rule gives the type
each argument needs and the call's return interval. The load types every call
of a builtin through its rule and judges the candidate against the needed
types. The run reads no rule and retypes no native's value: a native reads its
operands through [the door](../values/README.md#the-type-memo-and-satisfies),
so what `ATTR` and `FROM` return already carries the type their rules give over
the carried types, and the evaluator checks the carried type against the static
one in debug builds. A native
with no rule of its own takes its declared slots and a return at most its
declared one. `FROM`'s rule is the [projection](#static-types) of its record's
static type, and `ATTR`'s over a record the named field's type there. A
property law pins every rule: over argument intervals within others, its return
lies within theirs; handed no names, its return lies around its return over
any; and over its declared slots, its
return lies under its declared return. A return whose upper end is `Never` lies
within every interval, and a rule over an argument whose lower end lies outside
its need returns `Never`.

**A shadowable builtin is derived, not listed.** A builtin whose operands are
all `Any` — `==` and `PRINT` — admits every argument, so any user overload
narrower than `Any` beats it at the first class, and is selected in its place for
its type. Every other builtin wins what it ties with, and the
[overlap check](#the-overlap-check) keeps a user overload from tying with it.

## Selection

A keyworded call evaluates its slots — an `ATTR` label written bare is read as
the name it is, not evaluated — and reads each operand's **carried type**, a bare
label's the code kind of its name. [`select`](select.rs) then:

1. **admits** each candidate whose registered shape — a builtin's own type, or
   the shape a registration's function carries — admits the carried types
   class by class, solving a quantified candidate's group as it goes; a spread
   candidate, a list a `USING` or an `EVAL` supplied, contributes each function
   in it;
2. **ranks** the admitting candidates by the lattice's per-class verdicts, which
   the registry records the first time a pair meets, so dispatch compares no
   slot types of its own;
3. runs a **lone survivor**; where several survive, a **builtin** among them
   wins, and otherwise the call is an **ambiguity** error, whichever scopes the
   survivors were declared in — no scope shadows another's overload. No
   admitting candidate is a **no-overload** error naming the arguments' types.

A typed argument a candidate does not admit is a non-match that falls through to
the others, never a bind-time error. The candidate list is fixed per site, and
each call admits the candidates on it and reads the recorded verdicts.

**Static selection** gives each candidate a verdict where the shape is built, by
the arguments' [static types](#static-types): *never*, *always* or *maybe*. A
*never* candidate can never admit what the call passes, and is dropped; a use
left with none refuses the load, as a statically typed language refuses it. A
*maybe* candidate an *always* one strictly outranks at the first class, both
closed, is dropped too: whenever it admits, elimination drops it at that class.
A use left with no *maybe* candidate is ranked there when it holds one
candidate or every one is closed. Its winner is selected, and the call runs it
without admitting or ranking ([`chosen`](select.rs)), a quantified candidate
still solving its group from the carried types; a use none of whose candidates
ranks first refuses the load as an ambiguity. Any other call admits its *maybe*
candidates, takes each *always* one as admitted, ranks as above, and chooses
what selection over the full list would. Several rigid candidates are ranked at
the call: a relation between shapes over a lexical variable holds at every run,
but its absence need not — `INNER y :Elt` strictly outranks `INNER y :Number`
over a rigid `Elt` bounded by `Number`, and ties with it where `Elt` is
`Number`.

**A keyworded call binds by the registration.** The argument record binds each
slot to the parameter the registration names for it — or packs every slot into
`operands` for a unary operator — and carries each type parameter the call
solved, by name, as a type value, which the frame binds rather than solving
again. A builtin's native runs in the evaluation's own step. A call by name
hands its record over as written, and the
[frame](../program/README.md#the-body-runner) admits each argument against its
parameter's declared type and solves the callee's group against them jointly,
so naming the callee may admit what a keyworded call of the same function
refuses. The call says which it is (`CallKind`), so only a keyworded call's
record is trusted to carry type parameters.

## Static types

[`statics`](statics.rs) runs as `Language::check`, after the overlap check,
over the program's shape and every shape nested in it, a quote's code included.
It gives every value expression and value binder a **static type**: an
[interval](../type_lattice/README.md#the-unifier-collects-it-does-not-bind)
within which every type the run carries there lies — its upper end `Any` where
the load cannot bound it from above, its lower end `Never` where it cannot bound
it from below. A static type is **exact** where its ends meet: the run carries
that very type, each lexical variable in it bound as the run binds it. The pass reads each node through the evaluator's own reading of
it, so the load types a node as it evaluates, and it builds on what
[the type channel's load pass](../elaborate/README.md#the-type-channel-at-load)
fixed:

- a literal, a quote, a `FN` and a type expression the load knows are exact, at
  their own type, their code type, their function type, and the type of the type
  value they denote; a list, dict or record literal has the ends its parts' ends
  build, and a construction the ends its payload's build;
- a name has its binder's static type: a parameter is exactly its declared type
  where the retype makes it so — a list, dict or record type, a family or its
  application, a newtype or a union's variant — since its frame
  [retypes](../program/README.md#the-body-runner) the argument to it, and at
  most its declared type otherwise, a union keeping each variant's own type; a
  local has its right-hand side's, a
  registration is exactly its function type, and a type name its type value's; a block's `it`, an arm's `it`, a name a
  `USING` surfaces and a quote's hole are at most `Any`;
- a block has its last statement's type, and a bucket declaration is `Null`;
- an ascription `e :! T` is, like a parameter, exactly `T` where the retype
  makes it so and at most `T` otherwise. An operand whose static type is `Never`
  never arrives, and the ascription is `Never`; a type the load leaves unknown
  makes it at most `Any`;
- a keyworded call is its selected candidate's return — a builtin's as its
  [rule](#the-builtin-table) gives it, and a registration's exactly its return
  where the retype makes it so and the group is unquantified or solved to
  points, since the frame retypes its value to it, and at most it otherwise —
  or at most the join of every kept candidate's return: `Any` where one of them
  is a candidate the load cannot read, a spread, a hole or a registration of
  unknown shape;
- an application is the identity its construction builds when its head is a
  type the load knows, and its callee's return when the head is a function —
  exact by the same rule where the callee's static type is exact, and at most
  that return otherwise, since a parameter of function type may bind a function
  declaring a smaller return, and no function is retyped; a union variant
  construction is at most `Any`;
- `ATTR` over a record is the named field's type in the record's static type,
  read at each end: exactly that type where both ends name the field at one type
  the retype makes exact, since the run reads the field's value at it, and at
  most the upper end's otherwise — `Any` where the upper end names no such field.
  A field's value keeps its own variant of a union, so an `ATTR` over an exact
  record is exact only where its field's type is. A lower end lacking the field
  makes the candidate *never*, since every record the run can carry lacks it
  too;
- a call in tail position is typed as any other call: its node never finishes,
  since the frame it tails into returns at the enclosing contract, so no
  carried type is read against it;
- `FROM` over a written field list is the projection of its record's static
  type, read at each end: the record of the listed fields at the upper end's
  types, `Any` where it names none, above, and the same over the lower end's
  below, so a call over an exact record is exactly its projection. A lower end
  lacking a listed field makes the candidate *never*, so
  `EXPR #(GET r :{a :Number}) -> Any = #(#[b] FROM r)` refuses the load. Over a
  list the load cannot read, `FROM` is at most `:{}`, and the run restamps its
  record at the projection of the record's carried type;
- an `EVAL <code> -> <Type>` is its declared return as a registration's call
  is: exactly `<Type>` where the retype makes it so, since the run retypes its
  value to it, and at most `<Type>` otherwise. An operand whose static type is
  `Never` never arrives, and the `EVAL` is `Never`; a type the load leaves
  unknown makes it at most `Any`. Where the load traces the code
  to a written quote — its operand, or a name `LET` binds to one — it types the
  code's last statement as the code runs there, read through
  [`bound_above`](../type_lattice/README.md#substitute-then-ask) since the code
  roots a chain of its own, and checks that type against the declared return as
  the return check below checks a body's. An `EVAL` fills only the code's
  `\` keys, so an unmarked keyworded use's hole holds nothing as the traced code
  runs, and the load types the code a second time that way: after
  `LET q = #(1 + 2)`, the code `EVAL q -> Number` runs is `Number`, while the
  code's own cell, which a `USING` may fill, keeps the hole as a candidate the
  load cannot read and types `1 + 2` at most `Any`.

"At most" leaves the lower end at `Never`. A static type whose upper end is
`Number`, `Str`, `Bool` or `Null` is exact, since no value carries a type
strictly under one.

**Generic calls.** A quantified callee's group is solved from the arguments'
static types to an
[interval](../type_lattice/README.md#the-unifier-collects-it-does-not-bind) per
variable, which holds every solution a call can reach, and its return is
[read through the intervals](../type_lattice/README.md#substitute-then-ask): a
variable's upper end at a covariant position, its lower end at a contravariant
one. So under `EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)`, `ONLY 1` is
`Number`. Where every argument whose slot names a variable is exact and holds
no lexical variable, the solve over the static types is the call's own, and
each variable is solved to a point. A parameter exact at a container type is
such an argument: under
`EXPR FOR ALL #[Elt] #(FLAT rows :(LIST OF (LIST OF Elt))) -> :(LIST OF Elt)`,
`FLAT rows` over a parameter `rows :(LIST OF (LIST OF (Number | Str)))` is
selected at load, and solves `Elt` to `Number | Str` whatever the argument's
contents. An exact argument over a lexical variable
is no such solve: the load solves through the variable's bound, where the call
solves through the type the run binds it to. A call binds each variable to one
type, the
[least instance](../type_lattice/README.md#the-unifier-collects-it-does-not-bind)
of the pair its carried types solve it to, so it still solves its group from
the carried types.

**Lexical variables.** A static type may hold the
[lexical variables](../elaborate/README.md#the-type-channel-at-load) of the
names a run binds. A body runs under one call of each body enclosing it, so a
variable is one type wherever that body reads it, a capture included: `Elt` lies
under `Elt`, and a parameter `x :Elt` fills a slot `:Elt` with nothing to solve.
Against any other type a variable lies under what its bound lies under, and a
meet reads both sides through `bound_above`. A quote's code roots a chain of its
own, so a static type crossing into it, a `$` name's, or leaving it, an
`EVAL`'s, is read through `bound_above` too.

**Verdicts.** Each keyworded use gives each candidate one of three, as
[`judge_by_class`](../type_lattice/README.md#priority-classes) judges it:

- *never* — some slot meets its argument's upper end at `Never`, or does not
  lie above its argument's lower end, since the carried type lies above that
  end: beside `EXPR #(WHICH x :(LIST OF Number))`, `WHICH x` over a parameter
  `x :(LIST OF Any)` is *never*;
- *always* — every class admits whatever the call carries within its
  arguments' static types: a slot that is a variable of its own class alone
  admits under the variable's bound, and a class whose arguments naming its own
  variables are exact and hold no lexical variable admits when its static
  solve does;
- *maybe* — any other, and every candidate the load cannot read.

A slot naming a variable an earlier
[class](../type_lattice/README.md#priority-classes) solved is read through that
variable's interval: at its least instance for *always*, at its greatest for
*never*. So under `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`,
`PAIR 1 WITH 2` is *always*, while `PAIR a WITH b` over parameters
`a :(Number | Str)` and `b :(Number | Str)` is *maybe*: `a` may carry `Number`
where `b` carries `Str`. An argument whose static type is `Never` never arrives,
so its use drops and ranks nothing and its own static type is `Never`. A use
left with no candidate refuses the load (`ShapeError::NoAdmittingCandidate`):

```text
no overload of `_ + _` admits (Str, Number)
```

A use whose last candidate a builtin's need dropped refuses it in the native's
own words (`ShapeError::NoField`):

```text
:{a :Number} has no field b
```

A use left with no *maybe* candidate, holding one candidate or only closed
ones, is ranked as a call ranks. A lone survivor, or a builtin among several, is
selected; several survivors with no builtin among them refuse the load
(`ShapeError::Ambiguous`):

```text
ambiguous call of PICK _: 2 overloads admit (Number) and none ranks first
```

The cell records the candidate selected, or each candidate kept beside its
verdict.

**The return check.** A callable body whose static type — its last statement's
— meets its declared return at `Never` refuses the load
(`ShapeError::ReturnNeverSatisfied`), since every call of it would fault on its
contract:

```text
this body returns Number, which can never satisfy its declared return Str
```

A body whose static type is `Never` never arrives, and is not checked. A
callable declared to return `Never` is checked like any other, so its body must
itself be `Never`-typed — every other body meets `Never` at `Never`, and refuses
the load. The check is a meet of upper ends, not an order: a body whose static
type is at most `Any` loads, and the run holds what it returns to the contract.
An `EVAL`'s traced code is checked against its declared return the same way,
both read through `bound_above`, and refuses the load at the `EVAL`
(`ShapeError::EvalNeverSatisfied`): after `LET q = #("a")`, `EVAL q -> Number`
is refused with

```text
this `EVAL`'s code returns Str, which can never satisfy its declared return Number
```

An `EVAL` whose operand's upper end meets `Code` at `Never` refuses the load
too (`ShapeError::NotCode`):

```text
this value is Number, which can never be code for `EVAL` to run
```

**The ascription check.** An ascription is checked the same way: one whose
operand's upper end meets its type at `Never` refuses the load
(`ShapeError::AscriptionNeverSatisfied`), located at the ascription node, both
read through `bound_above`:

```text
this value is Str, which can never satisfy its ascription Number
```

One whose operand's upper end lies under its type is **settled**: every value
the run carries there satisfies it, so the run retypes without checking. The
order is rigid-aware, so an operand typed by a lexical variable settles an
ascription to that variable. The cell records each settled ascription by site.

What the pass fixes rests in each shape's write-once
[value-channel cell](../scope/README.md#load-time-types), which the call reads.
Inside a quote's code a refusal is kept on the code shape, and the `EVAL` running
it reports it, as it reports the type channel's. Debug builds check both halves
on every run: a finished value's carried type lies within its node's static
type, and a call that dropped, selected or took a candidate as admitted runs
what selection over the full list would.

## Tails under a contract

An evaluation a frame [tails into](../program/README.md#frames-contracts-and-tails)
owes the frame's [`Contract`](../program/record.rs). When it selects a
registration whose declared return, substituted by the call's solution,
satisfies the contract — or calls an unquantified function by name whose return
does — it hands its cell to the callee's frame by a tail rather than spawning
it, handing the frame the contract it owes, so a keyworded self-call in tail
position holds a constant number of cells however deep it recurses, and a chain
of tail hops returns at the outermost declared return. Any other value it
finishes with is held to the contract: checked against the callee's own return,
a miss naming it, and retyped to the contract's outermost return.

## Running code

`EVAL <code> -> <Type>` checks the code's shape for overlaps, as a loaded program's is —
its types were fixed where the program loaded, the quote's code included — and
asks the program's `EVAL` door for a frame running it over what the `EVAL`
offers: each name its operand's `NEEDING` list names, read where the `EVAL` is
written, and each bucket key as the list of functions a use at the key written
there would list, builtins included. A refusal — an overlap, the code shape's
own error or its typing refusal, an unbound name or a required keyworded hole —
is a fault.
The frame owes `<Type>` — the type the type part denotes, read as an
ascription's is — as a called frame owes its return: its value is checked
against it and retyped to it, and a tail in the code hands the contract on
([tails](#tails-under-a-contract)). A miss is a fault naming the code,
`` `EVAL`'s code returned Str, which does not satisfy Number ``. An operand
that is no code is a fault, `` Number is not code for `EVAL` to run ``.

## Errors

A refusal while a program runs is a [fault](../program/README.md#faults-and-output):
one of a closed set of kinds, each carrying the fields its message renders from
when it is printed or caught, and an uncaught one ends the program with
`error: <message>` and its trace. [`errors`](errors.rs) words what only
dispatch decides:

| Situation | Message |
|---|---|
| no admitting candidate | `no overload of _ + _ admits (Str, Number)` |
| an ambiguity | `ambiguous call of PICK _: 2 overloads admit (Number) and none ranks first` |
| a missing field | `:{x :Number y :Str} has no field z` |
| a missing union member | `:(Some \| None) has no member Many` |
| an incomparable `==` | `<T> and <U> cannot be compared` |
| `ATTR` over a module | `reading a module's member arrives with modules` |
| `:!` over a value that misses its type | `:(LIST OF :(Number \| Str)) does not satisfy its ascription :(LIST OF Number)` |
| `:!` over a module | `ascribing a module arrives with modules` |
| `EVAL` over a value that is no code | `` Number is not code for `EVAL` to run `` |
| a `USING` ranking | `MOVE _ TO _ is ranked two ways` |
| a dict key that is no scalar | `:(LIST OF Number) cannot be a dict key` |
| a node with no reading | `nothing evaluates <node>` |

A refusal a layer below renders keeps its own wording: a construction misfit
(`Some cannot wrap Str: its representation is Number`), a return miss, misnamed
arguments, an argument a call by name does not fit
(`:(FN :{x :Number} -> Str) cannot be called with :{x :Str}`), an unsolvable
group, a callee that is no function, a type expression that does not
elaborate, and an `EVAL`'s refusal.

A no-overload miss, an ambiguity, a return miss, an ascription miss, an `EVAL`
of what is never code or an `EVAL` whose code can never meet its return the load
can already see is no fault: it refuses the load, located at `path:line:col`
([static types](#static-types)).
The run meets only those whose static types are too wide to tell — an argument
read out of a record field, say, whose static type is `Any`.

## Catching

`TRY` and `CATCH` are where a fault becomes a koan value. Each runs its
operand as a block, whose shape records that its fault goes to them rather
than to the enclosing frame, so a `TRY` body is never in tail position. The
value is the builtin union `Error`: one variant per fault kind, over a record of
that kind's fields and `frames`, the trace an uncaught fault prints. Its
variants are not parameterized, so a field whose type varies with the refusal,
such as a missing key, is `Any`. A field may hold a value: the fault kept the
frame it lies in, and the catcher copies it out as a return is copied. Once the
`Error` is built, the catcher releases the frames the fault kept.

`TRY` selects an arm by the variant as `MATCH … UNDER Error` does, binding `it`
to its record, and a body that yields no fault yields its value. `CATCH` yields
`Result.Ok` of its body's value or `Result.Error` of the `Error`.

## The overlap check

A user overload may not take operands a builtin overload at its key already
takes, since the builtin would win every such call and the overload would be
dead there. [`check::overlaps`](check.rs) runs where a shape is built and its
types can be read: over a loaded program's shape and every body nested in it,
short of a quote's code, as `Language::check`, and over a quote's code where an
`EVAL` runs it. It reads each registration's
[load-time](../elaborate/README.md#the-type-channel-at-load) expression shape: a
closed, unquantified one at a key with builtin overloads — whatever declared
types its signature names, so an alias of `Number` is checked as `Number` is —
overlaps a builtin overload whose operands are not all `Any` when every slot pair
meets above `Never`, which refuses the shape (`ShapeError::Overlaps`). A
registration whose shape is rigid or unknown is never checked, and one a builtin beats
at some operand type is simply never selected there — nothing reports it, since
koan has no warning channel ([unplanned work](../../roadmap/rewrite/README.md#unplanned-work)).

## The import rule

Outside `#[cfg(test)]` this module names `crate::elaborate`, `crate::knot`,
`crate::memory`, `crate::parse`, `crate::program`, `crate::scheduler`,
`crate::scope`, `crate::symbols`, `crate::type_lattice` and `crate::values`;
[`tests::boundary`](tests/boundary.rs) reads the source to hold it there.

## Testing

The suites load whole programs under `Koan` and read back what they wrote to
either sink, one line per write ([tests.rs](tests.rs)):
[selection](tests/selection.rs) — every ranking example, ambiguity within and
across scopes at load and at run, the no-overload miss at load and at run, fall-through, builtin
tie-wins, shadowed `==` and `PRINT`, the overlap refusal at load and at `EVAL`,
and a quantified candidate that may admit whose carried solve fails;
[statics](tests/statics.rs) — the static type of each kind of node and binder,
a capture along a chain, a generic call by keyword, verdicts leaving one,
several and no candidate, the load refusals in a program, an uncalled body and
a quote's code, an `EVAL` whose traced code can never meet its declared return,
and an argument that never arrives;
[generic](tests/generic.rs) — generic code at load: a nested callable
substituted by level and agreeing with its load-time type where it is born, a
quote's code typed twice interning nothing, a nested group's parameters and
return check, exact static types, crossings into and out of code, an `EVAL`'s
declared type, a quantified candidate selected only over exact arguments, a
*maybe* an *always* outranks dropped, a use ranked at load and one ranked at the
call, and returns read through a group's intervals, by keyword and by name;
[rankings](tests/rankings.rs) — declarations, their idempotence, each
disagreement site, a ranking as part of the shape type, and a written-order
module failing a ranked signature member; [programs](tests/programs.rs) — the
builtin library, combined definitions, lambdas, type parameters, contracts,
record access, construction, error values, and a program nested to the
[syntax depth limit](../parse/README.md#the-syntax-depth-limit) in each nesting
shape run on a [`STACK_BYTES`](../program/README.md#the-stack) thread, one
level more refused at load; [quotes](tests/quotes.rs) —
an `EVAL` run to its declared return, a bare return spelling and an `EVAL`
declaring none, unmarked uses, `USING` fills, both marks and `NEEDING` keys;
[ascription](tests/ascription.rs) — `:!` retyping member by member, a tagged
value, a cyclic value, a miss and a module operand at run, a declared parameter
retyping by keyword and by name with the solution substituted, a tail chain
returning at the outermost contract, an `EVAL`'s value held to its declared
return — retyped, a miss, a tail out of its code, a non-code operand and a
`FOR ALL` name's return — and at load an ascription's and a
container or nominal parameter's exact type, a union's kept at most, a settled
ascription, a refused one, a generic use over an exact parameter selected, a
candidate *never* over an argument's lower end, a call exact at its callee's
return — by keyword, by name, in tail position — and one the load cannot solve
exactly, and `FROM`'s projection over an exact record;
[rules](tests/rules.rs) — the law every native's type rule obeys over drawn
argument intervals and names, and what `FROM`'s and `ATTR`'s rules make
the load type and refuse and the run carry;
[surface](tests/surface.rs) — printing, `==`, `ATTR`, `FROM` and a field read
through a newtype or a family seeing only what the carried type names, one node
seen at two types, a widened literal's element, and `FROM` sharing its record's
runs; and [tail](tests/tail.rs), a keyworded tail recursion holding its cells constant,
which is on the [Miri slate](../../observe/miri_slate.md). Every runnable
tutorial snippet is checked against its shown output by
`tools/verify_snippets.py` through the binary.

## Open work

- [Matching](../../roadmap/conditionals/matching.md) — `MATCH`, its arm selection,
  and matching on values.
- [Faults and call traces](../../roadmap/conditionals/faults.md) — faults apart from
  values, and the trace an uncaught one prints.
- [Catching errors](../../roadmap/conditionals/catching.md) — `TRY`, `CATCH`, `Error`
  and `Result`.
- [Module programs](../../roadmap/rewrite/modules.md) — the module expression
  shapes, `ATTR` over a module, and a `USING … SCOPE` body's registrations.
- [Solving dropped type parameters](../../roadmap/gradual-typing/solving-dropped-type-parameters.md)
  — a type parameter canonical form drops, which a call binds to its bound.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — the
  overlap check skipping a quantified registration, and a warning for an
  overload never selected.
