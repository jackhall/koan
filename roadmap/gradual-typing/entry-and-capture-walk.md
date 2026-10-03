# One entry per draft, one walk per capture

The builder hands a nested draft everything it needs through its `Entry`, and
the static pass follows a capture through one walk.

**Problem.** The builder passes a nested draft's inputs through three channels:
`Entry` (`tail`, `arm`, `surfaced`), the arguments of `enter_child`, and three
fields of `Builder` — `signature`, `quantified`, `heads`
([build.rs](../../src/scope/shape/build.rs)) — that `enter_body` sets for the
next `body` call to take. `body` can fail before it takes them, at the
operator-run rewrite, the `QuantifiedValue` check or `binders`, and
`enter_code` recovers from a refused quote restoring the claims, the frame, the
skip floor and the type depth but not those three. So after
`LET q = #((SIG Shown = #[(EXPR #(SHOW _ :Number) -> Str)]) (USING (m :! Shown) SCOPE (1 + 2 == 3)))`
— a `MixedGroups` refusal kept as the code's own — the next body drafted,
`LET f = (FN :{} -> Number = #(1))`, takes `Shown`'s surfaced head as a
registration of its own, with hop counts that belong to the `USING` block, and
`PRINT (f {})` panics binding it as a parameter.

A `\(…)` use in a quote's code lists only its offered key
([build.rs](../../src/scope/shape/build.rs), `candidates`), so in
`#((EXPR #(GREET y :Any) -> Any = #(y)) (PRINT \(GREET 1)))` the code's own
`GREET` is no candidate, though `\x` binds to a binder of the code that sees it
and an unmarked use lists the code's registrations beside its hole
([holes and marks](../../src/scope/README.md#holes-and-marks)).

The static pass ([statics.rs](../../src/dispatch/statics.rs)) follows a
capture to its source in four places — `read_declared`, `slot_of`, `hole` and
`coordinate_of` — with three rules at a quote's code root: `read_declared`
reads what it finds through `bound_above`, `hole` stops, and `slot_of` walks
through. `candidate` reads a registration's shape through `slot_of`, so a
`$(GREET $a)` in a quote inside a generic body, `GREET` declared there over
the body's `Outer`, is known as rigid over the chain outside the code. Its
return then names a lexical variable the code's chain has no home for, and a
`$(ONLY $(GREET $a))` whose `ONLY` solves from it fails the assertion in
`coordinate_of` at load.

`solved_earlier` solves the classes before an instance argument's slot in one
collector, where the call solves class by class, each pinned to the solutions
before it (`admit_by_class`). The two differ where an earlier class reaches a
variable only contravariantly and a later earlier class gives it a lower end:
under `EXPR #(PICK 3 AT 1 OR 2)` and
`EXPR FOR ALL #[Elt Key] #(PICK f :(FN :{x :Elt} -> Elt) AT y :(FN :{x :Elt} -> Null) OR h :{a :Elt, b :Key}) -> Elt`,
`PICK pick AT y OR {a = 1, b = true}` over
`y :(FN :{x :(Number | Str)} -> Null)` instantiates `pick` at the pooled
`Elt = Number`, where the call solves class 0 to `Number | Str`, and the use is
refused as admitting nothing — a *never* that does not hold at the call
([laws](../../src/type_lattice/laws.md#a-load-time-verdict-holds-at-every-run)).

**Acceptance criteria.**

- `Builder` has no field one draft sets for the next to take: a nested draft's
  signature, surfaced names, quantified names, surfaced heads and held groups
  arrive in its `Entry`, and one scoped helper saves and restores the walk's
  state around every nested draft, `enter_code`'s included.
- After the quote above refuses its code, `f`'s body shape declares no
  registration, and `PRINT (f {})` prints `1`.
- A `\(…)` use lists the code's registrations at its key that see it beside its
  offered key: the `\(GREET 1)` above lists the code's `GREET` and the offered
  `GREET _`, and the quote's type still needs `GREET _`.
- The static pass follows a capture through one walk, which reports a crossing
  into a quote's code; `read_declared`, `candidate`, `hole` and every caller of
  `slot_of` read through it, and a candidate reached across a code root whose
  registered shape names a variable of the chain outside is unknown to the load.
- The `$(ONLY $(GREET $a))` program above loads, and `PRINT (WRAP 1)` prints
  `1`, as it does with `$(GREET $a)` alone.
- An instance argument's earlier classes are solved as the call solves them,
  class by class, each pinned to the solutions before it; the
  `PICK pick AT y OR {a = 1, b = true}` program above loads and prints `1`.
- A property law states that a lexical variable reads the same through any
  context: a contribution or instance site naming one by a type — `$`-marked
  in a quote's code — resolves to one type whether a called `FN`, a hoisting
  block, a returned closure, a module or a quote's code lies between it and the
  variable's home, the unwrapped program its oracle.
- A property law states that static narrowing is transparent: over generated
  programs, a call over a narrowed candidate list runs what selection over the
  full list would, and a use the load refuses faults at every call within its
  arguments' static types.
- The scope plan generator writes quote values, registrations and keyworded
  uses in code, and a law over it states where each name and key in a quote
  lands: an unmarked name at a binder of the code that sees it, a builtin, or
  its hole; a `$` name where a read written at the quote lands, skipping every
  binder in the code; a `\` name at a binder of the code that sees it, or
  offered; an unmarked use at the builtins, the code's registrations that see it
  and its hole; a `$(…)` use at what a use written at the quote lists; and a
  `\(…)` use at the code's registrations that see it and its offered key.

**Directions.**

- *How the earlier classes are solved — decided.* One `Collector` per class in
  class order, each pinned through `Collector::pin` to the solutions of the
  classes before it; the lattice's own class walk is not changed.
- *What a candidate reached across a code root is known as — decided.*
  Unknown where its registered shape names a variable of the chain outside, and
  as registered where closed: a reader outside a chain reaches it unknown
  ([one chain](../../src/elaborate/README.md)). A closed shape through
  `bound_above` reads each slot at its lower end, so `GREET x :Outer` would
  admit nothing.
- *Where the narrowing law's program generator comes from — decided.* The
  `Desc` vocabulary of [tests/rules.rs](../../src/dispatch/tests/rules.rs),
  moved to a shared generator module and rendered to koan source beside a
  generator of literals under a drawn type.
- *How the narrowing law runs a use the load refuses — decided.* A
  `#[cfg(test)]` switch in the static pass leaves every keyworded use whole and
  refuses none, keeping each contribution; the law compares a program's run
  with its unnarrowed run, and checks the unnarrowed run faults at a refused
  use. A twin program hiding its arguments behind `Any` drops their
  contributions, so its call may solve otherwise.
- *What a `\(…)` use lists over a key its code registers — decided.* The
  code's registrations that see it beside its offered key, as an unmarked use
  lists them beside its hole; not those alone, as `\x` binds alone, since an
  overload set accumulates.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Calls by name judged as keyworded calls](calls-by-name-judged.md) — its laws draw from the program generator this item adds.
