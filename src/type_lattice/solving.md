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
variable stands for some type between a lower and an upper end. Read from
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

A **carried** rigid variable fills, at a covariant position, whatever its bound
fills: a `Held` bounded by `LIST OF Number` fills `LIST OF Elt`, solving `Elt` to
`Number`, just as the monomorphic instance between the two would. Below a rigid
variable is only itself, so at a contravariant position it fills nothing more.

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

**A binding does not grow with its arguments; an interval does.** Beside the
pair, the unifier reports an **interval** per variable
([`intervals`](unify.rs)), holding every least instance a call can bind over
arguments lying within the static types it collected — each an interval of its
own, as [dispatch](../dispatch/README.md#static-types) types an argument:

- the **upper end** is the join of the lower contributions, where some covariant
  position under no union names the variable — one every admitted argument
  reaches — and the bound elsewhere, since a variable no contribution reaches
  takes its bound;
- the **lower end** is the meet of the upper contributions and the bound, where
  the declared types name the variable at no covariant position, and `Never`
  elsewhere;
- where every argument whose position names a variable is **exact** — its lower
  end is its upper — and holds no rigid variable, the arguments a solve can
  meet are those very types, so each variable's interval is its least
  instance: solved to a point, which the caller asserts by passing `exact`. An argument
  over a rigid variable is no such argument: a solve reads a carried rigid
  variable through its bound, where a run binds it to one type under that
  bound.

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
class that orders neither of two leaves both to the next. `shape_specificity`
is the same comparison between two shapes.

The classes order **judging**, too. [`judge_by_class`](ranking.rs) gives a
candidate shape a verdict against arguments of known static types, each an
interval, class by class, beside each variable's interval:

- *never* — some slot, each variable an earlier class solved read at its
  greatest instance, meets its argument's upper end at `Never`, or does not lie
  above its argument's lower end read below its rigid variables: every type a
  call carries lies above that end, so the slot admits none; or an **exact
  class** — one every slot of which that names a variable of its own has an
  exact argument holding no rigid variable, and names only earlier variables
  solved to a point — whose static solve over those slots fails, since that
  solve is the call's own;
- *always* — every class admits whatever a call carries within the arguments'
  intervals: a slot naming no variable of its own class admits its argument's
  upper end with each earlier variable at its least instance; a slot that is a
  variable of its class alone, named by no other slot of the class, admits the
  argument's upper end under the variable's bound; and an exact class admits
  when its static solve does;
- *maybe* — any other.

Every rule reads a relation that holds of a lexical variable for every type a
run binds it to, so a verdict holds at every run.
