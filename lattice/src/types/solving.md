# Solving

Substitution, the unifier, and the priority classes a keyworded call is
admitted, ranked and judged by. Part of [the type lattice](README.md)'s design.

## Substitute, then ask

[substitute.rs](substitute.rs) holds substitution — of a binder's own
variables (instantiation and erasure), of lexical levels, and of a signature's
head parameters by name ([`substitute_parameters`](substitute.rs)) — and the
reads that replace a variable by one of its ends. No second structural descent
exists for any of them. Interning is insert-if-absent on a content-addressed
table and a substitution that binds nothing returns its input handle, so a
substitution costs one intern per changed composite. A signature is closed
content, so no substitution enters one; a head parameter a pin names is reached
through the application's pins.

A load-time type is read where it runs by replacing each lexical variable with
the binding at its level ([`substitute_levels`](substitute.rs)). No binder
captures a lexical variable, so the substitution asks no binder depth.

A substitution keeps its operand's kind: a `Parametric` comes back a
`Parametric`, and a `Scheme` a `Scheme`, its own group still bound. A caller
whose bindings answer every variable narrows the result through `concrete`,
naming the invariant that makes it concrete. `bound_above` and `erase_rigid`
answer every free variable of a `Parametric` by its ends, so they yield a
`KType`.

A type is **read through intervals** ([`read_through`](substitute.rs)). Each
variable stands for some type between a lower and an upper end, which
[`Variable`](node.rs) — the view of the three variable nodes — reads off it: a
lexical variable's own, `[Never, bound]` for the others. Read from
above, a variable takes its upper end at a covariant position and its lower end
at a contravariant one, which gives the type above every instance; read from
below, the reverse, which gives the type below every instance. A signature is
left opaque. `bound_above` is the read from above with every free variable — a
`Quantified` under none of the type's own binders, every `Parameter` and
every lexical variable — at `[Never, bound]`, a lexical variable at its own
two ends, which is what a load-time type is
compared through where a run may bind its variables to anything under their
bounds. `erase_rigid`, which reads a variable as its bound everywhere, can put a
contravariant position below an instance.

## The unifier collects, it does not bind

[`admits_with`](unify.rs) walks a declared type against a carried one and
collects what would solve the quantified positions. Instead of binding a variable
to the first argument it meets, it records **every** argument type that reaches
the variable as a lower contribution (covariant position) or an upper one
(contravariant). `Collector::solve` then bounds each variable by a **pair**:
below by the join of its lower contributions, above by the meet of its upper
contributions and its declared bound. The solve fails only where the set the
pair denotes is empty — where the lower end lies above an upper contribution or
above the bound. A contribution set with a maximum joins to that maximum, and
one with a minimum meets to that minimum; admission cannot depend on the order
the slots are read.

While it is solved, a variable can slide between its ends, so the solve picks
no point, and *fits*' instantiation clause asks only that each pair denote
some type. A point is taken where something needs one: a call binds each
variable to its pair's **least instance**, the lower end where a lower
contribution reached the variable and the upper end otherwise. So
`(f _ :Elt _ :Elt)` binds `Elt` to `Number` over `(1, 2)` and to
`Number | Str` over `(1, "x")`. A list holding a `FN :{x :(Number | Str)} -> Null`
and a `FN :{x :(Number | Bool)} -> Null` binds `LIST OF (FN :{x :Elt} -> Null)`'s
`Elt` to `Number`, and one holding functions over `Number` and over `Str` binds
it to `Never`: a function no argument may be passed to. A same-type check across
slots is what [priority classes](#priority-classes) are for.

A **declared union** at a covariant position admits a carried member through
one of its own members ([`most_determined_first`](unify.rs)): one equal to it
first, then the members that bind nothing, then those that bind, the more
specific before the less. A member is at least as specific as another where the
other admits it with its own variables rigid ([`member_at_least`](unify.rs)), as
[`class_at_least`](#priority-classes) compares two shapes' slots, so
`(Elt | (LIST OF Key))` admitting `[1]` binds `Key` to `Number`, and under
`#{Elt: Number, Key: Any}`, `(Elt | Key)` admitting `1` binds `Elt`. Where two or
more members bind, one whose read from above the carried member does not fit is
never tried: no binding of it admits. Two binding members **tie**
([`tied_members`](unify.rs)) where each is at least as specific as the other, or
neither is and their reads from above meet above `Never`: an argument both admit
would then bind the group by whichever member the union stores first. The
elaborator refuses a binder holding a tie at a covariant position of a
parameter, slot or representation
([`TiedUnion`](../../../src/elaborate/README.md#refusals)), so the member admitted never
turns on storage order. A contravariant position takes its union whole.

A **carried** rigid variable fills, at a covariant position, whatever its bound
fills: a `Held` bounded by `LIST OF Number` fills `LIST OF Elt`, solving `Elt` to
`Number`, just as the monomorphic instance between the two would. Below a rigid
variable is only itself, so at a contravariant position it fills nothing more.

Whatever its variables solve to, a declared type lies above `Never` and under
`Any`, so the walk admits a carried `Never` at a covariant position and a
carried `Any` at a contravariant one outright, as the order does, rather than
pairing either structurally against `LIST OF Elt` or a record. Each variable
the declared type names takes what a part-by-part pairing would give it, since
an extreme is `Never` in every covariant part and `Any` in every contravariant
one: a variable at a covariant position binds `Never`, as a bare one does, so
`LIST OF (LIST OF Elt)` over `[]` binds `Elt` to `Never`, and one only at
contravariant positions keeps its bound. Every member of a declared union is
read alike, so no member order enters.

A **construction** collects through `Collector::least`, under which a variable
no contribution reaches is bounded by `Never` rather than its bound: a family is
covariant in its parameters, so its least instance is the one the payload asks
for.

A variable no contribution reaches has the pair `[Never, bound]`, so a call
binds it to its declared bound: a collector holds the group's bounds from the
start (`Collector::new`).

A collector is typed by what it takes: every contribution and pin is a `T`,
every bound a `KType`, so its solution is a `T`. A call's collector takes the
concrete types its arguments carry, so a run-time solution is concrete by its
type; one over static types, which may hold lexical variables, is parametric.

A parametric solve is **reproducible** where a run, binding each lexical
variable its contributions name, solves to the static solution so bound, and
fails exactly where the static solve fails ([`Collector::reproducible`](unify.rs)).
A contribution handed whole to a variable's cell binds member by member, a join
keeps a variable beside the rest, and a verdict that holds over a variable holds
at every binding, so the collector disqualifies only three things: an admission
that read a variable through its ends (a carried rigid variable filling a
position through its bound, or a bound split across a declared union's members,
or a verdict over one that fails), a variable answered by a meet over a
contribution holding one, and a solve failing at a verdict over one. So
`LIST OF Elt` over `LIST OF Outer` solves `Elt` to `LIST OF Outer`
reproducibly, while `LIST OF Elt` over an `Outer` bounded by `LIST OF Number`
solves it to `Number`, which a run binding `Outer` below its bound does not
reproduce. The report is one flag for the whole collector, exact where it says
reproducible and conservative elsewhere; a closed solve always is.

**A binding does not grow with its arguments; an interval does.** Beside the
pair, the unifier reports an **interval** per variable
([`intervals`](unify.rs)), holding every least instance a call can bind over
arguments lying within the static types it collected — each an interval of its
own, as [dispatch](../../../src/dispatch/README.md#static-types) types an argument:

- the **upper end** is the join of the lower contributions, where some covariant
  position under no union names the variable — one every admitted argument
  reaches — and the bound elsewhere, since a variable no contribution reaches
  takes its bound;
- the **lower end** is the meet of the upper contributions and the bound, where
  the declared types name the variable at no covariant position, and `Never`
  elsewhere;
- where every argument whose position names a variable is **exact** — its lower
  end is its upper — and the solve over them is reproducible, the arguments a
  solve can meet are those very types, so each variable's interval is its least
  instance: solved to a point, which the caller asserts by passing `exact`. The
  point may name a lexical variable, as an exact `LIST OF Outer` argument does;
  the run reads it at its binding. A solve that reads a carried rigid variable
  through its bound is no such solve, since a run binds it to one type under
  that bound.

An end bounds what a call can bind; it is not itself a binding.

## Priority classes

A shape's slots sit in **priority classes**: dense ranks, one per slot in
element order, normalized by [`dense_classes`](shape.rs) from what a bucket
declaration writes — `2 1` and `20 10` are one ranking, `_` slots come after the
numbered ones in written order, and slots sharing an integer share a class.
Written order puts each slot in a class of its own. The ranking is part of the
shape's identity, fed to its digest and rendered in `_`'s place, so
`:(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)` and the written-order
`:(EXPR #(MOVE _ :Any TO _ :Any) -> Any)` are two types, and the binary walks
treat two shapes of unequal rankings as unrelated.

The classes order **admission**. [`admit_by_class`](ranking.rs) solves a
quantified group one class at a time: the first class whose slots mention a
variable solves it, jointly over that class's slots and every position inside
them, and each later class admits its arguments against the least instance of
that pair. So
`FOR ALL #[Elt] #(PAIR x :(LIST OF Elt) WITH y :(LIST OF Elt))` fixes `Elt`
from `x` and refuses a `y` that does not lie under it, while one class over both
slots — or a call by name, whose record has no order — solves them jointly.
A later class holds the solution as a **pin** in its collector, and a pin
refuses a contribution on its wrong side where it arrives — above it at a
covariant position, below it at a contravariant one — rather than when the
class's solve is read. So a union whose walk reaches a pinned variable takes a
member the pin allows, and a later contribution cannot contradict an earlier
class's solution through a member it happened to choose.
[`solving_slots`](ranking.rs) names the slots a solve reads: each one naming a
variable whose first class is its own. In written order `PAIR`'s `x` solves and
its `y` only admits, so dispatch hands the solve an argument's static type at
`x` alone.

`admits_shape` runs the same loop with a candidate shape's slot types as the
arguments. A slot type stands for every type a call carries under it, so an
earlier class fixes no point but an **interval** of them — `[Never, L]` where a
covariant position names the variable, `[U, bound]` where only a contravariant
one does — and each later class admits against a lexical variable between
those ends. An interval whose ends meet, or a solution holding a rigid
variable of the candidate's, is read as that point. So a signature's view never promises a call its overload refuses:
in written order, `FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt)` does not fit
`#(PAIR x :(Number | Str) WITH y :(Number | Str))`, which admits
`PAIR 1 WITH "s"`, while `FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Null) TO y :Elt)`
fits `#(APPLY f :(FN :{x :Number} -> Null) TO y :Number)`, since a call
binds `Elt` at or above `Number`.

The classes order **ranking**. [`class_at_least`](ranking.rs) is the verdict
"`a` is at least as specific as `b` at class `c`": `b`'s slots in class `c`
admit `a`'s, with each variable an earlier class admitted read, as
`admits_shape` reads it, as a lexical variable between the ends of its interval,
and each one an earlier class did not admit read as its bound. It reads two
shape handles and a class, so the registry records it in the verdict table (`Relation::ClassAtLeast`) the first time a pair meets.
[`select_by_class`](ranking.rs) eliminates over a candidate list class by
class: every candidate another strictly beats at a class drops out, and a
class that orders neither of two leaves both to the next. The strict win is
`outranks` — at least as specific, and not the other way — which a caller
eliminating ahead of the list reads too. `shape_specificity`
is the same comparison between two shapes.

The classes order **judging**, too. [`judge_by_class`](ranking.rs) gives a
candidate shape a verdict against arguments of known static types, each an
interval, class by class, beside each variable's interval:

- *never* — some slot, each variable an earlier class solved read at its
  greatest instance and every lexical variable left read at its ends from
  above, meets its argument's upper end at `Never`, or does not lie above its
  argument's lower end read below its rigid variables (`lower_end_outside`, the
  one test a caller holding a need or a carried type reads too): every type a
  call carries lies above that end, so the slot admits none; or an **exact class** —
  one every slot of which that names a variable of its own has an exact
  argument, and names only earlier variables solved to a point, and whose
  static solve over those slots is reproducible — where that solve fails,
  since it is the call's own;
- *always* — every class admits whatever a call carries within the arguments'
  intervals: a slot naming no variable of its own class admits its argument's
  upper end with each earlier variable at its least instance; a slot that is a
  variable of its class alone, named by no other slot of the class, admits the
  argument's upper end under the variable's bound; and an exact class admits
  when its static solve does;
- *maybe* — any other.

Every rule reads a relation that holds of a lexical variable for every type a
run binds it to, so a verdict holds at every run.
