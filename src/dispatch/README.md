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
([priority classes](../../lattice/src/types/solving.md#priority-classes)). Dispatch is
what runs between the two: it evaluates a call's slots, admits and ranks the
candidates by the arguments' types, and runs the one that wins.

## What dispatch runs

Literals, names, containers, record access, construction, functions, keyword
dispatch, the builtin library, quotes, `EVAL` and `USING` over code, modules —
a `MODULE` or `GROUP` binder, `:!` and `:|`, a member read and
`USING … SCOPE` — and uncaught faults.
[Matching](../../roadmap/conditionals/matching.md) owns `MATCH`, and
[catching errors](#catching) `TRY`, `CATCH` and `Result`; a node dispatch has
no reading for is a fault.

## What a node is

The [evaluator](evaluate.rs) reads a node off the shape that holds it — the
statements the shape owns, [rewritten](../scope/README.md#the-four-rewrites),
never the parse — as one of:

- a **leaf**: a literal, lowered; a name, read through its mention's
  coordinate — at an [instance site](#static-types) the load recorded, made
  into the instance through the knot's
  [instance door](../knot/README.md#an-instance); a quote, born through the [quote door](../knot/README.md#a-quote);
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
  reads there — a `FN FOR ALL` the load instantiated born as its instance;
- an **ascription** `<value> :! <Type>` or `<module> :| <Sig>`, its operator read
  with the node, so the load reads it as the run does: the operand, evaluated, checked against
  the type its type part denotes — read as a type expression leaf's is — unless
  the load [settled](#static-types) it, and
  [retyped](../values/README.md#the-type-memo-and-satisfies) to it, so what
  follows dispatches on the ascribed type: after
  `LET Loose = :((LIST OF Any) | Null)`, `[1] :! Loose` carries `LIST OF Any`,
  and a module ascribed a signature type is re-stamped at the application it
  fits. A value that does not satisfy the type is a fault. An operand under
  `:|` goes through the
  [view door](../knot/module/README.md#the-view-door) instead, as the load
  reads it too, which builds the
  view or refuses: `:|` over a value that is no module, a type naming no one
  application of a signature, a module that does not fit it, or a member that
  cannot take the view's type is a fault;
- an **`EVAL`** `<code> -> <Type>`: the operand, evaluated, run as code
  ([below](#running-code)) in a frame owing the type its type part denotes —
  read as an ascription's is — as a called frame owes its return. An operand
  that is no code is a fault;
- a **`USING <module> SCOPE <body>`**: the operand, evaluated, and the body run
  as a block [entered on the module](../knot/module/README.md#entering-a-using--scope-block),
  each surfaced name bound to the member it names and each surfaced key to the
  list of the functions the module offers there; its value is the body's last
  statement's;
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
  type name, giving the variant; and over a module, the member at its
  [layout](../knot/module/README.md#layout-order) index, a quantified member
  made into the instance the load solved at the read — a member the module
  lacks is a fault, and so is a quantified one read where the load recorded no
  type to instantiate it at. `FROM` restamps a record
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
its need returns `Never`. "Outside" is the judge's own lower-end test, the
lattice's `lower_end_outside`, and the rule names what the argument lacks from
the names it narrowed the need by: the first field, in the order written, a
record lacks, or the member a module read names.

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
   class by class, solving a quantified candidate's group as it goes, each
   slot that solves reading its argument's [contribution](#static-types)
   where the load recorded one; a spread
   candidate, a list a `USING` or an `EVAL` supplied, contributes each function
   in it;
2. **ranks** the admitting candidates by the lattice's per-class verdicts, which
   the registry records the first time a pair meets, so dispatch compares no
   slot types of its own;
3. runs a **lone survivor**; where several survive, a **builtin** among them
   wins, and otherwise the call is an **ambiguity** error, whichever scopes the
   survivors were declared in — no scope shadows another's overload. No
   admitting candidate is a **no-overload** error naming the arguments' types.
   That tie rule is one function, [`winner`](select.rs), which the load's
   selection ends in too, and both refusals are worded by one formatter,
   [`selection_refused`](../scope/shape.rs), so a load refusal and a run fault
   read alike.

A typed argument a candidate does not admit is a non-match that falls through to
the others, never a bind-time error. The candidate list is fixed per site, and
each call admits the candidates on it and reads the recorded verdicts.

**Static selection** gives each candidate a verdict where the shape is built, by
the arguments' [static types](#static-types): *never*, *always* or *maybe*. A
*never* candidate can never admit what the call passes, and is dropped; a use
left with none refuses the load, as a statically typed language refuses it. A
*maybe* candidate an *always* one strictly outranks at the first class, both
closed, is dropped too: whenever it admits, elimination drops it at that class
([`never_runs`](select.rs), which reads the lattice's `outranks`, the strict
win `select_by_class` eliminates by).
A use left with no *maybe* candidate is ranked there when it holds one
candidate or every one is closed. Its winner is selected, and the call runs it
without admitting or ranking ([`chosen`](select.rs)), a quantified candidate
still solving its group, as step 1 solves it; a use none of whose candidates
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
hands its record over as written, beside the contributions the load recorded
for it — one per parameter in symbol order, as a keyworded use records one per
slot — and the [frame](../program/README.md#the-body-runner) admits each
argument — at its contribution where it has one — against its parameter's
declared type and solves the callee's group against them jointly, so naming the
callee may admit what a keyworded call of the same function refuses. The call says which it is (`CallKind`), so only a keyworded call's
record is trusted to carry type parameters. A function a view carries behind a
[barrier](../knot/module/README.md#members-are-born-coerced) registers at the
shape the barrier shows its caller and binds its slots under the names the
function behind it registers; the [frame](../program/README.md#the-body-runner)
crosses the barrier and calls that function by name, so it carries no type
parameter.

## Static types

[`statics`](statics.rs) runs as `Language::check`, after the overlap check,
over the program's shape and every shape nested in it, a quote's code included.
It gives every value expression and value binder a **static type**: an
[interval](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind)
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
  application, a newtype or a union's variant, a function type or an expression
  shape, a code kind below `Code`, a kind, or a signature type pinning every head
  parameter — since its frame
  [retypes](../program/README.md#the-body-runner) the argument to it, and at
  most its declared type otherwise, a union keeping each variant's own type. A
  signature type leaving a head parameter unpinned is at most itself: each run
  pins the parameter at what its module fits, and two pins of one parameter are
  unordered. A
  local has its right-hand side's, an annotated local, `LET n :T = v`, what
  the ascription `v :! T` would have — a member of a cyclic component included,
  since the tie births it at its annotation — a
  registration is exactly its function type, and a type name its type value's; a block's `it`, an arm's `it`
  and a quote's hole are at most `Any`, and a name a `USING` surfaces is at
  most the member read of its operand it names;
- a block has its last statement's type, and a bucket declaration is `Null`;
- an ascription `e :! T` is, like a parameter, exactly `T` where the retype
  makes it so and at most `T` otherwise. A module operand exact at its own
  signature, at a closed `T`, is exactly the application the run re-stamps it
  at, which the load computes with the run's own `pinned_at_fit`, and the
  ascription refuses the load where it fits a signature type `T` not at all. An
  operand whose static type is `Never`
  never arrives, and the ascription is `Never`; a type the load leaves unknown
  makes it at most `Any`;
- a keyworded call is its selected candidate's return — a builtin's as its
  [rule](#the-builtin-table) gives it, and a registration's exactly its return
  where the retype makes it so and the group is unquantified or solved to
  points, since the frame retypes its value to it, and at most it otherwise —
  or at most the join of every kept candidate's return: `Any` where one of them
  is a candidate the load cannot read, a spread, a hole or a registration of
  unknown shape. A `USING … SCOPE` block's
  [surfaced head](../scope/README.md#names-that-arrive-at-run-time) is a
  registration whose return is at most the head's whatever the solve, since the
  module's own definition answers the call — save a lone closed head of the
  signature a header carries: every function the header offers there is
  retyped to it, and the call's frame retypes its value to the head's return
  under the selection's own solution, so the call is exactly that return where
  the retype makes it so and every interval is a point, and its verdict may be
  *never*. Under
  `SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]`, in
  `USING (m :! Boxes) SCOPE (…)`, `BOX 1` is exactly `LIST OF Number` and
  `BOX "s"` exactly `LIST OF Str`, each use solving the head's `Elt` afresh.
  Through a `:|` view the call is at most the head's return, though the run
  retypes it there too
  ([unplanned work](../../roadmap/rewrite/README.md#unplanned-work)).
  A surfaced key is a spread over every function the module offers there, so
  its verdict is at most *maybe*, and *never* only where one body definition or
  one closed signature head is the whole of the key;
- an application is the identity its construction builds when its head is a
  type the load knows, and its callee's return when the head is a function —
  exact by the same rule where the callee's static type is exact, a parameter
  of function type included, since its frame retypes the function to the
  parameter's type and a call through it retypes its value to the return it
  shows, and at most that return otherwise; a union variant construction is at
  most `Any`;
- `ATTR` over a record is the named field's type in the record's static type,
  read at each end: exactly that type where both ends name the field at one type
  the retype makes exact, since the run reads the field's value at it, and at
  most the upper end's otherwise — `Any` where the upper end names no such field.
  A field's value keeps its own variant of a union, so an `ATTR` over an exact
  record is exact only where its field's type is. A lower end lacking the field
  makes the candidate *never*, since every record the run can carry lacks it
  too;
- a `MODULE` or `GROUP` binder is exactly the signature the run ties where the
  load knows every member exactly — each value binder at a closed point or a
  closed scheme, each type binder and registration closed — and at most
  `Module` otherwise. Its body runs inline, so it is typed where its binder is;
- a `:|` ascription is a view, as the run reads it. A view refuses the
  load only where every run's view door faults: an operand that can never be a
  module (`ShapeError::NotAModule`, `Number is no module to ascribe` for
  `1 :| Any`), a closed type that is no one application of a signature
  (`ShapeError::NotASignature`, for `m :| Any` or `m :| (A & B)`), and an exact
  operand whose signature can never fit the ascribed one, which the meet test
  cannot say, since signatures meet to a set and never to `Never`. It is
  exactly the view's signature over an operand whose [content tree](#paths) is
  closed, since the load computes the carriers the run keys on that content, and
  at most the ascribed type otherwise. A call or member read whose tree is a
  view is likewise exactly the view's signature over a closed tree, and at most
  the view's application otherwise, and one whose tree is a closed retype is
  exactly the type the load computed for it;
- a member read `m.f` is the member's type in its operand's upper end, a
  signature, an application of one or a meet of those, each pin substituted,
  the meet of what each declares under a meet. The run reads the member through
  the signature its module carries and retypes it to its declared type there,
  so over an operand exact at a signature type — that type itself, never a
  variable's bound — the read is exactly the declared type where the retype
  makes it so; otherwise it is at most the member's declared type. A type
  member is its kind where its type is concrete, and the read is unknown where
  the upper end declares no such member. Where the member names a
  head parameter the application leaves unpinned and the operand is a name or a
  chain of member reads rooted at one, that parameter is read as the
  [path](#paths) through the operand, so `m.zero` and `m.succ` share one type;
  through any other operand the read is unknown. A lower end that is a
  signature lacking the member makes the candidate *never*, so `geometry.tau`
  refuses the load as `SIG (pi: Number) has no member tau`;
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
  [`bound_above`](../../lattice/src/types/solving.md#substitute-then-ask) since the code
  roots a chain of its own, and checks that type against the declared return as
  the return check below checks a body's. An `EVAL` fills only the code's
  `\` keys, so an unmarked keyworded use's hole holds nothing as the traced code
  runs, and the load types the code a second time that way: after
  `LET q = #(1 + 2)`, the code `EVAL q -> Number` runs is `Number`, while the
  code's own cell, which a `USING` may fill, keeps the hole as a candidate the
  load cannot read and types `1 + 2` at most `Any`.

"At most" leaves the lower end at `Never`. A static type whose upper end is
`Number`, `Str`, `Bool` or `Null` is exact, since no value carries a type
strictly under one. "Where the retype makes it so" is one predicate, read by the
load's `retyped_to` and by the retype itself
([the type memo](../values/README.md#the-type-memo-and-satisfies)), so a type
the load calls exact is the one the run stamps.

Every refusal for a value that "can never satisfy" a type — a return, an
ascription, an annotation, an `EVAL`'s code or operand, a view's operand, an
instance site's other argument — is one test, `never_satisfies`: both read through
their bounds, they meet at `Never`.

A static type's ends are
[parametric](../../lattice/src/types/identity.md#typed-handles), since they may hold
lexical variables, and are compared by *fits*; what a call reads at run time
is concrete. A binder of a quantified callable — a `MODULE` or `GROUP` body's member, or a
name a keyworded form binds — is typed by its scheme, which a call's head reads
and solves, and the statement binding it is at most `Any`. A part's or a
statement's static type is never a scheme.

**Instance sites.** A quantified function anywhere else — a name bound to one,
read where the [resolution rule](../scope/README.md#resolution) lets it be, or
a `FN FOR ALL` literal — is an **instance site**, typed with the type it is
**wanted** at:

- an annotated binder's right-hand side, at its annotation;
- an ascription's operand, at its type;
- a callable body's last statement, at the declared return — a `LET` there
  hands it to its right-hand side;
- a call by name's argument, at the callee's parameter record, which a record
  literal hands each field — a quantified callee's field at its slot, as a
  keyworded argument is wanted at its candidate's;
- a list, dict or record literal's parts, at its element, value or field type,
  only where the wanted type is that container's own;
- a keyworded argument that is itself an instance site, at the slot of each
  candidate, typed per candidate.

A member read `m.f` of a quantified member is an instance site too, where its
operand is a name or a chain of member reads rooted at one, so the load sees
the member's scheme without typing the operand. One whose scheme names a head
parameter its signature leaves unpinned reads that parameter as the
[path](#paths) through the operand, and is instantiated there like any other:
with `m :Mapper` declaring
`VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt)`,
`LET go :(FN :{x :(m.Carrier), y :Number} -> Number) = m.apply` loads. Behind an
opaque view's barrier the run builds the instance as a barrier of its own
around the same function (`barrier_instance`). Only where no path through the
operand is reachable — a name read across a quote's code — does the read refuse
the load as `UnpinnedMember` anywhere but a call's head; at a call's head the
run solves the call, as a call by name of a scheme. A read whose operand the
load cannot see into is no instance site, and the run faults where it reads a
scheme there.

A wanted type is never split: a union or `Any` fixes nothing. The site is
instantiated at the least instance of its scheme under the wanted type
([`instance_under`](../../lattice/src/types/relations.md#quantified-binders)), its
static type exactly that instance. A `LET` of a quantified `FN` outside a
`MODULE` or `GROUP` body is an instance site whether or not its name is read, wanted at
its annotation or, as a callable body's last statement, at the declared return;
so a bare `LET pick = (FN FOR ALL …)` at the top level refuses the load. A
name's solution is recorded by its site in the shape's cell, and a literal's in
its body's write-once born-instance cell. A solution may name a lexical
variable: in the body of
`EXPR FOR ALL #[Outer] #(WRAP a :Outer) -> :(FN :{x :Outer} -> Outer)`, `pick`
as the last statement solves `Elt` to `Outer`, its static type exactly the
instance over `Outer`. The cell then records where the site reads each variable
it names, as a contribution's does (below), and the run substitutes the type it
binds there where the name is read or the literal is born, so `WRAP 1` returns
a `FN :{x :Number} -> Number` and nothing is solved at run time. Each refusal
names the group's variables in group order:

```text
nothing fixes `Elt` here: a quantified function is read only at the head of a call, as the binding of a `MODULE` member, or where the type it is wanted at solves its group
:(FN :{x :Number} -> Number) does not fix `Elt`
:(FN FOR ALL #[Elt] :{x :Elt} -> Elt) has no instance under :(FN :{x :Number} -> Str)
```

— `ShapeError::Unfixed` with no wanted type or one that reaches a variable with
nothing, and `NoInstance` where the scheme does not fit the wanted type. A
quantified callee's slot is read through its group solved from its **other**
arguments first, each read as the call's solve reads it: one at a slot that
solves, whose argument contributes, exactly at its upper end. So under
`EXPR #(PAIR 1 WITH 1)` and
`EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH f :(FN :{x :Elt} -> Elt)) -> Elt`,
`PAIR b WITH pick` over a parameter `b :(Number | Str)` instantiates `pick` at
`Elt = Number | Str`, the type the call solves from `b`'s contribution, and so
does `h {x = b, f = pick}` by name under the same `FN FOR ALL`. A variable that a class before the slot's
[solves](../../lattice/src/types/solving.md#priority-classes) is taken as the call
solves it: class by class, from each class's solving slots at their arguments'
contributions, each class pinned to what the classes before it solved. Under
`EXPR #(APPLY 2 TO 1)` and
`EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt) -> Elt`,
`APPLY pick TO b` over a parameter `b :(Number | Str)` instantiates `pick` at
`Elt = Number | Str`. A contribution holding a lexical variable fixes the
variable where a run
[reproduces the solve](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind):
in the body of `EXPR FOR ALL #[Outer] #(WRAP xs :(LIST OF Outer)) -> Any`,
`APPLY pick TO xs` instantiates `pick` at `Elt = LIST OF Outer`, a solution
naming a lexical variable as above, while under
`#{Outer: :(LIST OF Number)}` with `xs :Outer` and `y :(LIST OF Elt)` the solve
reads `Outer` through its bound, which a run binding it lower does not
reproduce. A solving argument there that is itself an instance site, whose
contribution is unknown, or whose solve a run may not reproduce, fixes nothing.
Any other variable the slot shares with the other arguments must come out one
point the call reproduces — a closed one, or one of a reproducible solve — and
one only the slot names is read through `[Never, bound]`, so under
`EXPR FOR ALL #[Elt Out] #(MAP f :(FN :{x :Elt} -> Out) AT y :Elt) -> Out`,
`MAP pick AT 1` instantiates `pick` at `Elt = Number` and is `Number`, while
`MAP pick AT y` over a parameter `y :(Number | Str)` refuses — and loads under
`EXPR #(MAP 2 AT 1)`, `Elt` taken from `y`. An other argument
that does not admit its slot fixes nothing either, so it refuses the load as
`Unfixed` too — save one that can never fit its slot, its upper end meeting the
slot at `Never`, both read through their bounds: at a keyworded use that
candidate is dropped and left to the judge's own refusal, and a call by name is
refused `CallNeverSatisfied`. A candidate whose
slot gives no instance is *never*. The rest are judged with the instance as an
exact argument, and must agree on it (`AmbiguousInstance`). A lone candidate
dropped this way reports its own refusal, and several `NoInstanceAtCandidates`.

**Generic calls.** A quantified callee's group is solved from the arguments'
static types to an
[interval](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind) per
variable, which holds every solution a call can reach, and its return is
[read through the intervals](../../lattice/src/types/solving.md#substitute-then-ask): a
variable's upper end at a covariant position, its lower end at a contravariant
one. So under `EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)`, `ONLY 1` is
`Number`. Where every argument at a slot that solves is exact or contributes
its static type (below), and a run
[reproduces](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind)
the solve over them, that solve is the call's own, and each variable is solved
to a point. A parameter exact at a container type is such an argument: under
`EXPR FOR ALL #[Elt] #(FLAT rows :(LIST OF (LIST OF Elt))) -> :(LIST OF Elt)`,
`FLAT rows` over a parameter `rows :(LIST OF (LIST OF (Number | Str)))` is
selected at load, and solves `Elt` to `Number | Str` whatever the argument's
contents. An exact argument over a lexical variable is one too where the
solve hands it whole to a variable, so a point may name it: `BOTH m WITH m`
over `m :(LIST OF Outer)`, under
`EXPR FOR ALL #[Elt] #(BOTH x :(LIST OF Elt) WITH y :(LIST OF Elt))`, solves
`Elt` to `Outer` and is selected. A solve that reads the variable through its
bound is not, since the call solves through the type the run binds it to. A
call binds each variable to one
type, the
[least instance](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind)
of the pair its arguments solve it to.

**Contributions.** A call solves a quantified candidate's group from what the
load knows of each argument, and from what the run carries only where the load
knows nothing. At a slot that
[solves](../../lattice/src/types/solving.md#priority-classes) — one naming a variable
its own class is the first to name — an argument **contributes** the upper end
of its static type, or its carried type where that upper end is `Any`, since
`Any` constrains nothing; every other slot admits its argument's carried type,
as an unquantified slot does. So under
`EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, `PAIR a WITH b` over
parameters `a :(Number | Str)` and `b :(Number | Str)` binds `Elt` to
`Number | Str` whatever they hold, while `PAIR e WITH a`, `e` an `EVAL`
declared `-> Any`, solves `Elt` from what `e` carries and admits `a` against
it. A programmer moves an argument to the load's side with `:!`. A static type
that cannot fit its slot contributes all the same, so the solve fails at every
run and the load refuses the use: under
`EXPR FOR ALL #[Elt] #(FIRST xs :(LIST OF Elt)) -> Elt`, `FIRST m` over a
parameter `m :((LIST OF Number) | Null)` refuses the load, where an
unquantified slot of that type would be *maybe*. An instance argument and an
`ATTR` label written bare contribute their carried types. A static solve may
bind what the carried one would not, and the call runs what it binds; a
no-overload fault still names the carried types.

A contribution holding a lexical variable is read where the call runs, at the
type the run binds the variable to. The use records where it reads each
variable — a hop per block, and a
[type capture](../scope/README.md#load-time-types) in each callable or module
between the use and the body declaring the variable — so a callable nested in a
generic body solves as that body itself would, and a closure holds a type
capture only where one of its calls or instance sites reads it. The pass computes a
contribution only at a position some candidate's slot solves, whether or not
the load keeps that candidate, since a call runs what selection over the full
list would and a candidate the load judged *never* from a contribution must stay
*never* there. The cell
records each use's contributions, which the call resolves and hands selection
beside the carried types.

**Lexical variables.** A static type may hold the
[lexical variables](../elaborate/README.md#the-type-channel-at-load) of the
names a run binds. A body runs under one call of each body enclosing it, so a
variable is one type wherever that body reads it, a capture included: `Elt` lies
under `Elt`, and a parameter `x :Elt` fills a slot `:Elt` with nothing to solve.
Against any other type a variable lies under what its bound lies under, and a
meet reads both sides through `bound_above`. A quote's code roots a chain of its
own, so a static type crossing into it, a `$` name's, or leaving it, an
`EVAL`'s, is read through `bound_above` too. A candidate a use in the code
reaches across its root, a `$(…)` use's, keeps its registered shape where that
shape is closed and is unknown to the load where the shape names a variable of
the chain outside: read through its bounds, `#(GREET x :Outer) -> Outer` would
admit nothing.

**Verdicts.** Each keyworded use gives each candidate one of three, as
[`judge_by_class`](../../lattice/src/types/solving.md#priority-classes) judges it:

- *never* — some slot meets its argument's upper end at `Never`, or its
  argument's lower end does not fit it, since the carried type lies above that
  end: beside `EXPR #(WHICH x :(LIST OF Number))`, `WHICH x` over a parameter
  `x :(LIST OF Any)` is *never*;
- *always* — every class admits whatever the call carries within its
  arguments' static types: a slot that is a variable of its own class alone
  admits under the variable's bound, and a class whose arguments naming its own
  variables are exact, and whose static solve over them a run reproduces,
  admits when that solve does — where it fails, the candidate is *never*. A
  contributing
  argument is read as exactly its upper end, since the call solves from that
  type. Each admission reads
  [*fits*](../../lattice/src/types/relations.md#the-relations), so a module argument at
  most a signature declaring all of `Boxes`'s members and more is *always* at a
  slot `:Boxes`;
- *maybe* — any other, and every candidate the load cannot read.

A slot naming a variable an earlier
[class](../../lattice/src/types/solving.md#priority-classes) solved is read through that
variable's interval: at its least instance for *always*, at its greatest for
*never*. So under `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`,
`PAIR 1 WITH 2` is *always*, and so is `PAIR a WITH b` over parameters
`a :(Number | Str)` and `b :(Number | Str)`, since `a` contributes
`Number | Str` and `b` lies under it, while `PAIR a WITH e`, `e` an `EVAL`
declared `-> Any`, is *maybe*: `e` may carry a type `Elt` does not admit. An argument whose static type is `Never` never arrives,
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
ambiguous call of `PICK _`: 2 overloads admit (Number) and none ranks first
```

An instance argument is listed at its scheme, since no instance of it is made.
The cell records the candidate selected, or each candidate kept beside its
verdict, in one record per use beside the use's contributions (above), which
the call reads by its node's site (`BodyShape::use_statics`); a use the load
narrowed nothing at and recorded no contributions for has no record, and the
call selects over its whole list from the carried types.

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

One whose operand's upper end [fits](../../lattice/src/types/relations.md#the-relations)
its type is **settled**: every value the run carries there satisfies it, so the
run retypes without checking. *Fits* is rigid-aware, so an operand typed by a lexical variable settles an
ascription to that variable. The cell records each settled ascription by site.

An annotated binder, `LET <name> <type> = <value>`, is held to its type the same
way, refused `ShapeError::AnnotationNeverSatisfied` at its statement and settled
by its type part's site:

```text
this value is Str, which can never satisfy its annotation Number
```

**The call-by-name check.** A call by name of a callee the load knows exactly —
an unquantified function at a point, or a quantified one by its scheme — is
judged as a keyworded use of its lone candidate: an expression shape over the
callee's parameters, one slot each in symbol order, every slot in one class,
returning the callee's return, judged through the lattice's `judge_by_class`
over the argument's static type laid onto the slots. The lay-out reads the
argument at both ends: each slot takes the field of its name from the lower
end, or `Never` where that end is no record, and from the upper end, or `Any`.
Records are width-superset, so a lower end that is a record lacking a parameter,
or an upper end that is a record naming a field no parameter declares, can never
name the parameters exactly, and refuses the load
(`ShapeError::CallNeverSatisfied`), as does a judgement of *never*:

```text
:(FN :{x :Number} -> Str) can never be called with :{x :Str}
```

So under `LET which = FN EXPR #(WHICH x :(LIST OF Number)) -> Str`, `which {x = x}`
over a parameter `x :(LIST OF Any)` refuses the load as `WHICH x` does, while
under `LET f = FN :{x :Number, y :Str} -> Str`, `f (g {})` over a parameter
`g :(FN :{} -> :{y :Str})` loads: a record under `g`'s return may name `x`. A
field whose static type is `Never` never arrives, so the call is `Never`. A
callee the load knows only at most, such as a parameter of function type, may
bind a function admitting more or naming other parameters: its call is the
call's to admit, at most its return.

A quantified callee's group is solved as a keyworded use's is, each field
contributing as an argument does, so `pair {x = a, y = b}` under
`FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> :(LIST OF Elt)` is exactly
`LIST OF (Number | Str)` over the parameters above. The cell records the
contributions by the argument's site for the frame, one per parameter in symbol
order, whatever the static solve finds: a field whose static type the load
cannot admit still contributes, and the frame solves from it as a keyworded
call would. The judge refuses the load where a contribution the load knows —
closed, or holding a lexical variable — does not fit its parameter, or where
the solve over them has no solution, each where a run reproduces the solve:
under `FN FOR ALL #{Elt: Number} :{x :Elt} -> Elt`, `only {x = a}` over
`a :(Number | Str)` refuses the load, and so does `first {y = m}` under
`FN FOR ALL #[Elt] :{y :(LIST OF Elt)} -> Type` over
`m :((LIST OF Outer) | Null)`, whose member `Null` fails at every binding. A
record literal's field that is an instance site is wanted at its slot, read
through the group the other fields solve, as an [instance site](#static-types)
is. The call is exact at its return where its callee is unquantified, or where
the solve is reproducible over exact contributions, as a keyworded one is. A
call by name and
its keyworded twin over one class therefore load, fault or return alike, and a
call by name is blind to the order its record's fields are written in.

What the pass fixes rests in each shape's write-once
[value-channel cell](../scope/README.md#load-time-types), which the call reads.
Inside a quote's code a refusal is kept on the code shape, and the `EVAL` running
it reports it, as it reports the type channel's. Debug builds check the load's
verdicts on every run: a finished value's carried type lies within its node's
static type — under its upper end read above its variables, and over its lower
end read below them, by the judge's own `lower_end_outside` — and each argument
a call binds carries a type under its slot or parameter at the solution. Where
the load and the run make one judgment they call one function, so no net holds
two copies equal; the [narrowing law](#testing) holds a narrowed call to
selection over the full list.

### Paths

A **path** reads a type member through a module-valued expression —
`m.Carrier`, `p.inner.Carrier`, `(MAKESET ints).Set` — as the
[module design](../../design/modules.md#carriers-and-paths) has it. The type
channel types each path it spells as a
[path variable](../elaborate/README.md#the-type-channel-at-load), and the pass
resolves each one ([`paths`](statics/paths.rs)), just before the unit spelling
it, off the **content tree** of the module its root names, walked through its
member chain:

- **closed**, where the load knows that module's signature exactly — the
  root's exact static type, or a closed opaque view's — and reads the path off
  it as the run does (`elaborate::path_binding`), where the root's declared
  application pins the label, or where the member is a type a body binds
  closed. The path is that type — under a pin, the pin, which *fits* solves the
  label to at every run;
- otherwise a **class**: the variable of the first path resolved over a tree
  [`same_as`](../scope/README.md#load-time-types) its own, at the same label,
  through the same declared application. Paths of one class are one type, so a
  call solving one variable from two of them is *always*; paths of two classes
  are *maybe*; carriers over two different closed trees are *never*.

Every cell the type channel fixed is read through those resolutions. A path in
a type expression through a module built in place that does not resolve closed
refuses the load as `ShapeError::OpenPathInType`:

```text
this type reads a member of a module built by a call written in place, and a call in a type is never made; bind the call with `LET` to name its type
```

**Content trees.** The pass reads a module-valued expression's tree
([`trees`](statics/trees.rs)) as the
[module design](../../design/modules.md#the-load-and-the-run) tables it,
memoized per shape and site, a binder read through itself reading as a leaf.
A call **unfolds** where the load knows its callee: a keyworded use selected at
one registration whose `FOR ALL` variables solve to closed points, or a call by
name of an unquantified closure over a record literal. Its tree is the callee's
**summary** — the tree of its body's last statement, its parameters leaves —
with each argument's tree and each variable's point substituted, and every
other leaf of the callee's body keyed on the call. A leaf on a `MODULE` or
`GROUP` body's binder is keyed on the body's builder captures instead, wherever
it is read, since they say which run of the body holds it. The callee is typed
on demand before its first unfolded call; one met while it is being typed, and
every callee typed since, is recursive and never unfolds, so whether a call
unfolds does not turn on declaration order. Wherever the run retypes a knot
member — a `:!`, an annotated binder, an unfolded call's parameter or return, a
member read — the tree is a **retype** of the member's tree at the type the
run stamps, digested as the run digests a header; where the load cannot say
what the run stamps, the retype keys the source at the declared type. Each `:|`
site at a closed signature records its view's tree in the cell, keyed on its
source's bare member and what the fit solves, which `BodyShape::view_tree`
reads back.

**Where the run reads a path.** No value carries a path variable. The run binds
one where it substitutes a cell (`elaborate::bound_path`): a closed path at its
type, and any other by reading the module at the root's coordinate, walking its
member chain and reading the label — through *fits* against the root's declared
application where that application names the label a head parameter, as the
view door fits, and as the module's own type member otherwise. A declared
application naming lexical variables is located per reading shape and solved
there. The pass adds no capture for a path: a use
records a path variable at a path of its class whose root the reading shape
reaches through its blocks and builder captures, and a static type read across a
callable or module boundary reads each path variable no reachable path stands
for through its bounds, its upper end from above and its lower end from below.

**Pins are read, not stored.** A member read through a name chain pins the
operand's unpinned head parameters to their paths only while it reads the
member (`elaborate::applied_member`). No static type holds an application
pinning a parameter to a path: the lattice reads a pin through its bounds
covariantly where the order compares pins invariantly, so such a type read
through its bounds would be no upper end
([unplanned work](../../roadmap/rewrite/README.md#unplanned-work)).

**Closed views at load.** At a `:|` site over a closed tree the pass calls the
view door's type half,
[`opaque_view_types`](../knot/module/README.md#the-view-door), into the root's
registry: each carrier and the view's signature and, where the load knows what
the operand carries, what *fits* solves, each member's declared type under
either side and the types of each nested module's re-view, at any depth
([`views`](statics/views.rs)). What a module carries is read off its tree: a
`MODULE` body's exact signature, which the pass keeps by body; a closed
retype's, the type the load computed for it, each member read through it at the
type it declares; and a member an opaque view rebuilt, as the coercion
walk rebuilds it, a list's or dict's cells of several signatures one by one.
The pass keeps what each read yields — a view over a closed tree by its print, a
value a view rebuilt by what it was rebuilt from — so a chain of views reads
each link once. Over an opaque view whose source the load does not type
exactly, a member is read at the type the view declares for it. Where no tree
shows a member — one written in a container literal, bound to a parameter or
returned by a call the load does not unfold — its exact type stands for it, and
a view of that type is read as its signature declares its members
([the content tree's horizon](../../design/modules.md#the-load-and-the-run)).
A member declared at an application fixing each parameter of its signature is
read as if it carried that application, its own members at their declared
types under the pins, so its re-view is laid at any depth. Two re-views stay
the run's: one from an application that leaves a parameter open, and one from
an application pinning a parameter above what the member's values realize,
through a slot that leaves that parameter open, where *fits* solves the run's
re-view off the values. An exact `:!` of a module lays each declared type a
member read through it reads (`lay_carried`). The run's view door then interns
none of them, and asserts in debug builds that the closed tree's digest is the
view's; the pass asserts that what the tree shows the operand carries is its
exact static type, where it has both.

### Body classes

The pass classes every frame body — the program, a callable, a quote's code,
counted with the blocks and module bodies it runs — **sealed** or **porous**
([type tiers](../../design/type-tiers.md#two-classes-of-body)): porous where a
porous value leaves it at a porous slot, at its return or as an argument of a
call into a frame. A value that holds nothing the body minted is no exit there:
one at a site that is not porous; one the body received — a parameter or a
capture named bare, or a member of one read through the builtin `ATTR`; a record
literal written in place whose every field holds nothing minted; or a module
whose content tree ([paths](#paths)) reaches no view, no closure, no module body
holding a minting site and no leaf but a value the body received
([`holds_nothing_minted`](statics.rs)). The caching record lists the
site of each argument passing on a received value, for the run's exits to adopt
by its memo alone. A sealed
callable or quote's code
that mints at run **caches**: its types live in a scratch tier
([where each body interns](../../design/type-tiers.md#where-each-body-interns)).
A `:|` over a closed content tree mints nothing at run, since the load laid its
types in the root.

Each shape the pass types gets a write-once caching record
([`caching`](statics/caching.rs)) that classes every minting site **closed**,
**invariant** or **varying** by the leaves of its content tree — a nominal
declared per call by the type binders its declaration reads — and holds every
composite site warm whose parts read invariant views alone. A leaf is
invariant where it is a capture, or a parameter every tail edge into its body
passes on bare from an invariant parameter of the caller: a greatest fixpoint
over the tail edges of every callable's and quote's code's last statement,
fixed once the whole pass has run, a call by name's arguments paired with the
callee's parameters by field and a keyworded use's by slot. A callable whose
name is read as a value, or which is born in a `MODULE` or `GROUP` body or
written in place, may be called from anywhere, so its parameters are varying.

## Tails under a contract

An evaluation a frame [tails into](../program/README.md#frames-contracts-and-tails)
owes the frame's [`Contract`](../program/record.rs). When it selects a
registration whose declared return, substituted by the call's solution,
satisfies the contract — or calls an unquantified function by name whose return
does, the one answer [`Contract::kept_by`](../program/record.rs) gives both — it hands its cell to the callee's frame by a tail rather than spawning
it, handing the frame the contract it owes, so a keyworded self-call in tail
position holds a constant number of cells however deep it recurses, and a chain
of tail hops returns at the outermost declared return. Any other value it
finishes with is held to the contract: checked against the callee's own return,
a miss naming it, and retyped to the contract's outermost return. A call through
a view's barrier never tails: its value crosses the barrier where its frame
ends.

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
| no admitting candidate | ``no overload of `_ + _` admits (Str, Number)`` |
| an ambiguity | ``ambiguous call of `PICK _`: 2 overloads admit (Number) and none ranks first`` |
| a missing field | `:{x :Number y :Str} has no field z` |
| a missing union member | `:(Some \| None) has no member Many` |
| an incomparable `==` | `<T> and <U> cannot be compared` |
| `ATTR` over a module lacking the member | `SIG (pi: Number) has no member tau` |
| a quantified member read with no instance | `member pick is quantified, and no type was known to instantiate it at` |
| a quantified member behind a barrier read off a call's head | `member pick is quantified behind its view's barrier, and is read only at a call's head` |
| `:!` over a value that misses its type | `:(LIST OF :(Number \| Str)) does not satisfy its ascription :(LIST OF Number)` |
| a module that does not fit its signature | `SIG (other: Number) does not satisfy its ascription SIG (label: Str)` |
| `:\|` over a value that is no module | `Number is no module to ascribe` |
| a module ascribed what is no signature | `Number is no signature` |
| a member that cannot take its view's type | `member zero cannot take its view's type: its value does not satisfy Number` |
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

A no-overload miss, an ambiguity, a return miss, an ascription miss, a view of
what is never a module or at what is no signature, an `EVAL` of what is never
code or an `EVAL` whose code can never meet its return the load can already see
is no fault: it refuses the load, located at `path:line:col`
([static types](#static-types)). A refused selection and a refused view are
worded by one formatter each, `selection_refused` and `view_refused` in
[`scope`](../scope/shape.rs), for the load's refusal and the run's fault alike.
The run meets only those whose static types are too wide to tell — an argument
read out of a record field, say, whose static type is `Any`, or a type a run
binds.

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
registration whose shape is rigid or unknown is never checked, nor is a
`USING … SCOPE` block's surfaced head, which births no callable: the definition
answering it is the module's own, checked where the module is built. A
registration a builtin beats at some operand type is simply never selected there — nothing reports it, since
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
an argument that never arrives, each call of a quantified function typed by its
own arguments, a `USING … SCOPE` block's surfaced heads typed through the
ascription's pins, and a wider signature *always* at a narrower one's slot and
outranking it;
[generic](tests/generic.rs) — generic code at load: a nested callable
substituted by level and agreeing with its load-time type where it is born, a
quote's code typed twice interning nothing, a nested group's parameters and
return check, exact static types, crossings into and out of code, a candidate
over a variable outside code unknown in it, an `EVAL`'s declared type, a
quantified candidate selected where its static solve is the call's, a
*maybe* an *always* outranks dropped, a use ranked at load and one ranked at the
call, returns read through a group's intervals, by keyword and by name, and a
union in a group's parameter tried most specific member first, a tie refused;
[rankings](tests/rankings.rs) — declarations, their idempotence, each
disagreement site, a ranking as part of the shape type, and a written-order
module failing a ranked signature member; [programs](tests/programs.rs) — the
builtin library, combined definitions, lambdas, type parameters, a module's
quantified function run through a call, a call by name refused at load,
modules fitting a signature's quantified head
and each application their overloads answer, contracts,
record access, construction, error values, and a program nested to the
[syntax depth limit](../parse/README.md#the-syntax-depth-limit) in each nesting
shape run on a [`STACK_BYTES`](../program/README.md#the-stack) thread, one
level more refused at load; [quotes](tests/quotes.rs) —
an `EVAL` run to its declared return, a bare return spelling and an `EVAL`
declaring none, unmarked uses, `USING` fills, both marks and `NEEDING` keys, a
`\(…)` use selecting among its code's registrations and the offered ones, and a
refused code handing nothing to the next body drafted;
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
[annotation](tests/annotation.rs) — an annotated binder retyped to its type,
refused, settled at load and missed at run, a function recursing through its
annotated name, and a `USING` over an annotated binder;
[instances](tests/instances.rs) — each instance site and its refusals: a
quantified binding outside a module, an annotation, an ascription, a declared
return, a container's element type, a keyworded slot solved from the other
arguments first, candidates disagreeing, a call by name's record, an instance
argument or field sharing its class with a contribution made at it, a call by
name whose other fields never fit refused, a closed
solution, a solution over a type each run binds — at each kind of site, through
nested callables and a block, and its equality under one binding and two — a
keyworded slot's variables an earlier class solves, ranked, in written order
and solved class by class over two earlier classes, a later one checking what
an earlier one solved, fixed at each binding over a lexical
earlier argument, a rigid candidate and a shared variable a reproducible solve
fixes, and refused over an unknown earlier argument or one read through a
lexical bound, an instance's body reading
its solution and calling its siblings, its equality, and one made in a frame and called where it lands, which is on the
[Miri slate](../../observe/miri_slate.md);
[contributions](tests/contributions.rs) — a call solved from its arguments'
contributions: a declared argument's static type, an `EVAL`'s carried one at a
slot that solves and at one that does not, a static type that cannot fit its
slot refused at load, a contribution over a lexical variable read in its body,
through nested callables and through a block, a closure capturing a type only
where a call in it contributes it, which is on the
[Miri slate](../../observe/miri_slate.md), and a call by name solved from its
record's static type, exact at its return, refused at load, reading a rigid
contribution, solving from a rigid field the load cannot admit, refused over
its argument's lower end as its keyworded twin is, its argument read at both
ends, `Never` where a field never arrives, and judged by a
reproducible solve over a lexical variable — refused, exact at its return — as
a keyworded call over one is: refused where it fails, selected where it admits,
and a later slot reading an earlier lexical solution at its ends;
[rules](tests/rules.rs) — the law every native's type rule obeys over drawn
argument intervals and names, and what `FROM`'s and `ATTR`'s rules make
the load type and refuse and the run carry;
[narrowing](tests/narrowing.rs) — the law that static narrowing is
transparent: a drawn program the load accepts runs as it does loaded under the
test-only `unnarrowed` switch, which leaves every use whole and refuses none,
and a use the load refuses faults there; [spellings](tests/spellings.rs) — the
laws that a call by name of a one-class registration runs as its keyworded call
and is blind to its record's field order; [lexical](tests/lexical.rs) — the law that a
lexical variable reads the same through any context, a site naming one by a
type run inside drawn functions, closures, hoisting blocks, modules and a
quote's code and without them; [retypes](tests/retypes.rs) — the law that a
value retyped to a drawn type, a dict or a nominal type among them, and a
quote's code or a type value retyped to a drawn code kind or kind, lies within
the static type the load gives it at each retype site: an ascription, an
annotation, a parameter, an `EVAL` and a registration's return, the
evaluator's debug net its oracle, beside a quote and a type value dispatching
at the kind their slot declares and a retyped data node rendering its cycle;
[functions](tests/functions.rs) — a function retyped at its slot: a hidden
field of its return refused at load, a call through it exactly its declared
return, an overload over the hidden field never selected, the function equal
to itself through the slot and one header through two, a function declaring
fewer parameters than its slot shows, a function of a cycle born at its
annotation, and a tail chain through one;
[signatures](tests/signatures.rs) — a module retyped at a signature-typed
parameter, a declared return, an annotation, a record field and `:!`: a hidden
field of a member refused at load, a member read exactly its declared type,
one at an unpinned parameter read at what the module fits, an overload over a
hidden field never selected, `USING` surfacing what the signature names, a
nested module's pinned parameter read through a header, a brand the signature
hides kept, a view through a header the view of its member, a barrier's value
retyped to its return, quantified members read at a plain function type and at
another scheme, a keyworded member's covering overload picked through a
header, ambiguous where two rank alike, and a quantified keyworded member
returning what its declared shape returns through a header and a view; [generate](tests/generate.rs) holds the types,
values and programs these laws and `rules` draw;
[surface](tests/surface.rs) — printing, `==`, `ATTR`, `FROM` and a field read
through a newtype or a family seeing only what the carried type names, one node
seen at two types, a widened literal's element, and `FROM` sharing its record's
runs; [modules](tests/modules.rs) — each ascription operator, a retype at a
signature and at a meet of two, a member read
by name, through a chain and by a quoted label, a type member read through a
module name and through a view's nested member, a module binder exactly the
signature the run ties, the load refusals — a view every run's door refuses
among them — and the run faults, a quantified member instantiated where it is
wanted and one over an unpinned parameter instantiated at its path, a path
through a parameter read as the module fits its declared application, the pin
where it pins the label, `USING`
blocks opening names and spreading every overload at a key, calls through a
barrier both ways — two of them on the [Miri slate](../../observe/miri_slate.md) —
the union rule, the capture contract over each kind of outer name, and carriers
keyed on content; [digests](tests/digests.rs) — content digests over programs
that run: equal values alike, a closure and a module as their code over what
they capture, a top-level read named by its binding, a ring as one knot, one
text resolving apart digesting apart, and a view as its operator and
application over its source, apart from a retype of it, and headers over
modules branded apart digesting apart; [trees](tests/trees.rs) — the content tree
each `:|` site reads back over a parameter, a capture, a local, an expression
no binder names, a `MODULE` body over literals and a call that unfolds, a
closed tree's digest against the run's, a functor ignoring its argument, a leaf
keyed on a call's arguments or a body's captures, recursive functors never
unfolding, and a closure retyped at a parameter closed at the run's digest; [paths](tests/paths.rs) — two member reads through one path sharing
a type, two paths compared by their trees, a path through a call the carrier
the run computes, a type through an inline call over a leaf refused, each open
path's tree read back, a quantified member read through a call instanced or
refused, a functor body written on one line unfolding, and the law that a load
verdict on two paths through drawn modules, views and functor calls holds at
every run;
[views](tests/views.rs) — a view of a body over literals laying no type at run,
and views laying every nested module's re-view at load: in a slot, a record
field, a list (empty, or of modules of two signatures) or a dict, two deep,
under a header or an opaque view or a chain of them, at a slot leaving a
nested parameter unset, over a list or dict of modules of two signatures viewed
twice, and inside a record literal past the tree's horizon; a lone `:!` laying
its types; a view inside a record literal over a slot leaving a parameter open,
running; and a chain of views over a module the load types at most, re-viewing
at run no more than its first view does;
[classes](tests/classes.rs) — each body's class, a value holding nothing minted
— received, a record literal written in place, a module built over received or
literal content — passed on or returned as no exit, and each site's caching
class over parameters, captures, locals, views of views, nominals and
composites, a keyworded tail loop pairing its
arguments by slot, and the tutorial's classes (an ignored measurement);
[caching](tests/caching.rs) — caching bodies at run: `Shares`, `Fresh`, helper
and mixed-placement loops holding their tiers flat across hops, warm verdicts,
a parking loop's scratch bytes flat, the scratch and the lent branch reset
after a delivery, and each exit — a retyped value, a closure, a quote, a knot
member, a held frame's hand-off, a received list adopted by its memo alone,
a loop passing on a member of a received chain adopting a hop at a time,
and a loop handing a callee a module built over its link each hop, caching and
flat;
and [tail](tests/tail.rs), a keyworded tail
recursion holding its cells constant, which is on the
[Miri slate](../../observe/miri_slate.md). Every runnable
tutorial snippet is checked against its shown output by
`tools/verify_snippets.py` through the binary.

## Open work

- [Matching](../../roadmap/conditionals/matching.md) — `MATCH`, its arm selection,
  and matching on values.
- [Faults and call traces](../../roadmap/conditionals/faults.md) — faults apart from
  values, and the trace an uncaught one prints.
- [Catching errors](../../roadmap/conditionals/catching.md) — `TRY`, `CATCH`, `Error`
  and `Result`.
- [Invariance around a tail cycle](../../roadmap/type-tiers/cycle-invariance.md)
  — an argument built over invariant leaves classed invariant.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — the
  overlap check skipping a quantified registration, a warning for an overload
  never selected, a pin read through its bounds, a member read exact through a
  signature leaving a head parameter unpinned, a keyworded member through a
  view at most its return at load, a quantified callable's
  parameter at an application over its own group, and a quote reading a view's
  quantified member.
