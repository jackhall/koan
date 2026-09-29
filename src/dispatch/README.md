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
dispatch, the builtin library, quotes, `EVAL` and `USING` over code, and
uncaught faults. [Matching](../../roadmap/rewrite/matching.md) owns `MATCH`,
[catching errors](#catching) `TRY`, `CATCH` and `Result`, and
[module programs](../../roadmap/rewrite/modules.md) own the module expression
shapes; a node dispatch has no reading for is a fault.

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
  record or a tagged value over one, reading through every tagged layer to the
  field; over a type, giving the type its record declares the field with, so
  `Point.y` is `Str` and `v :Point.y` is a slot; over a union type labelled by a
  type name, giving the variant; and over a module, a fault until
  [module programs](../../roadmap/rewrite/modules.md). `FROM` projects a record
  to the fields it names. `EVAL` runs code (below), and `USING` fills a code's
  holes through the [`USING` door](../knot/README.md#a-quote).

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

**Static selection** narrows that list where the shape is built, by the
arguments' [static types](#static-types). A candidate whose slots meet them at
`Never` can never admit what the run passes, and is dropped; a use left with
none refuses the load, as a statically typed language refuses it. A use left
with one candidate that admits the static types outright selects it there, and
the call runs it without admitting its arguments ([`chosen`](select.rs)), a
quantified candidate still solving its group from the carried types — a solve
that fails is the same no-overload miss full selection raises. Otherwise the
call admits what is left, as above, and chooses what selection over the full
list would.

**A keyworded call binds by the registration.** The argument record binds each
slot to the parameter the registration names for it — or packs every slot into
`operands` for a unary operator — and carries each type parameter the call
solved, by name, as a type value, which the frame binds rather than solving
again. A builtin's native runs in the evaluation's own step, save `EVAL`'s,
which asks for a frame. A call by name hands its record over as written, and the
[frame](../program/README.md#the-body-runner) admits each argument against its
parameter's declared type and solves the callee's group against them jointly,
so naming the callee may admit what a keyworded call of the same function
refuses. The call says which it is (`CallKind`), so only a keyworded call's
record is trusted to carry type parameters.

## Static types

[`statics`](statics.rs) runs as `Language::check`, after the overlap check,
over the program's shape and every shape nested in it, a quote's code included.
It gives every value expression and value binder a **static type**: a bound
under which every type the run carries there lies, and `Any` where the load
cannot bound it. It reads each node through the evaluator's own reading of it,
so the load types a node as it evaluates, and it builds on what
[the type channel's load pass](../elaborate/README.md#the-type-channel-at-load)
fixed:

- a literal has its own type, a list, dict or record literal the type its parts'
  static types build, a quote its code type, and a type expression the type of
  the type value it denotes;
- a name has its binder's type: a parameter its declared type, a local its
  right-hand side's, a registration its function type, a type name its type
  value's; a block's `it`, an arm's `it`, a name a `USING` surfaces and a quote's
  hole are `Any`;
- a block has its last statement's type, a `FN` its function type, and a bucket
  declaration `Null`;
- a keyworded call has its selected candidate's declared return, or the join of
  every kept candidate's — `Any` where one of them is a candidate the load
  cannot read: a spread, a hole, a registration of unknown shape;
- an application has the identity its construction builds when its head is a
  type the load knows, and its callee's declared return when the head is a
  function; a union variant construction and `ATTR` over a record are `Any`.

A quantified callee's return is read through its bounds, never solved from the
static argument types. A static type may hold the rigid variables of the
**region** it is read in — the program, a callable body whose declaration writes
a `FOR ALL` group, a quote's code — numbered as the type channel numbers them. A
capture crossing into a region, and every comparison the pass makes, reads a
type through [`bound_above`](../type_lattice/README.md#substitute-then-ask), so
no variable is compared outside the region that binds it.

**Narrowing.** Each keyworded use drops every candidate some slot of which meets
its argument's static type at `Never`. A candidate the load cannot read is kept.
An argument whose static type is `Never` never arrives, so its use narrows
nothing and its own static type is `Never`. A use left with no candidate refuses
the load (`ShapeError::NoAdmittingCandidate`):

```text
no overload of `_ + _` admits (Str, Number)
```

A use left with one closed candidate that admits the arguments' static types is
`Selected`; one that dropped something is `Kept`, and any other is `Full`.

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
the load. The check is a meet, not an order: a body
whose static type is `Any` loads, and the run holds what it returns to the
contract.

What the pass fixes rests in each shape's write-once
[value-channel cell](../scope/README.md#load-time-types), which the call reads.
Inside a quote's code a refusal is kept on the code shape, and the `EVAL` running
it reports it, as it reports the type channel's. Debug builds check both halves
on every run: a finished value's carried type lies under its node's static type,
and a narrowed or selected call runs what selection over the full list would.

## Tails under a contract

An evaluation a frame [tails into](../program/README.md#frames-contracts-and-tails)
owes the frame's [`Contract`](../program/record.rs). When it selects a
registration whose declared return, substituted by the call's solution,
satisfies the contract — or calls an unquantified function by name whose return
does — it hands its cell to the callee's frame by a tail rather than spawning
it, so a keyworded self-call in tail position holds a constant number of cells
however deep it recurses. Any other value it finishes with is held to the
contract: retyped to the declared return, or a fault naming the callee.

## Running code

`EVAL code` checks the code's shape for overlaps, as a loaded program's is —
its types were fixed where the program loaded, the quote's code included — and
asks the program's `EVAL` door for a frame running it over what the `EVAL`
offers: each name its operand's `NEEDING` list names, read where the `EVAL` is
written, and each bucket key as the list of functions a use at the key written
there would list, builtins included. A refusal — an overlap, the code shape's
own error or its typing refusal, an unbound name or a required keyworded hole —
is a fault.
`EVAL` over anything but code is a no-overload miss like any other.

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
| a `USING` ranking | `MOVE _ TO _ is ranked two ways` |
| a dict key that is no scalar | `:(LIST OF Number) cannot be a dict key` |
| a node with no reading | `nothing evaluates <node>` |

A refusal a layer below renders keeps its own wording: a construction misfit
(`Some cannot wrap Str: its representation is Number`), a return miss, misnamed
arguments, an argument a call by name does not fit
(`:(FN :{x :Number} -> Str) cannot be called with :{x :Str}`), an unsolvable
group, a callee that is no function, a type expression that does not
elaborate, and an `EVAL`'s refusal.

A no-overload miss or a return miss the load can already see is no fault: it
refuses the load, located at `path:line:col` ([static types](#static-types)).
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
across scopes, the no-overload miss at load and at run, fall-through, builtin
tie-wins, shadowed `==` and `PRINT`, the overlap refusal at load and at `EVAL`,
and a statically selected quantified candidate whose carried solve fails;
[statics](tests/statics.rs) — the static type of each kind of node and binder,
a capture within and across a region, narrowing to one, several and none, both
load refusals in a program, an uncalled body and a quote's code, and an argument
that never arrives;
[rankings](tests/rankings.rs) — declarations, their idempotence, each
disagreement site, a ranking as part of the shape type, and a written-order
module failing a ranked signature member; [programs](tests/programs.rs) — the
builtin library, combined definitions, lambdas, type parameters, contracts,
record access, construction, error values, and a program nested to the
[syntax depth limit](../parse/README.md#the-syntax-depth-limit) in each nesting
shape run on a [`STACK_BYTES`](../program/README.md#the-stack) thread, one
level more refused at load; [quotes](tests/quotes.rs) —
unmarked uses, `USING` fills, both marks and `NEEDING` keys; and
[tail](tests/tail.rs), a keyworded tail recursion holding its cells constant,
which is on the [Miri slate](../../observe/miri_slate.md). Every runnable
tutorial snippet is checked against its shown output by
`tools/verify_snippets.py` through the binary.

## Open work

- [Matching](../../roadmap/rewrite/matching.md) — `MATCH`, its arm selection,
  and matching on values.
- [Faults and call traces](../../roadmap/rewrite/faults.md) — faults apart from
  values, and the trace an uncaught one prints.
- [Catching errors](../../roadmap/rewrite/catching.md) — `TRY`, `CATCH`, `Error`
  and `Result`.
- [Module programs](../../roadmap/rewrite/modules.md) — the module expression
  shapes, `ATTR` over a module, and a `USING … SCOPE` body's registrations.
- [Solving dropped type parameters](../../roadmap/rewrite/solving-dropped-type-parameters.md)
  — a type parameter canonical form drops, which a call binds to its bound.
- [Solving a group from static types](../../roadmap/rewrite/solving-from-static-types.md)
  — a quantified callee's return typed at the solution its arguments' static
  types give.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — the
  overlap check skipping a quantified registration, and a warning for an
  overload never selected.
