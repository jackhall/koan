# `TRAIT` builtin for structural typing

**Problem.** Writing a function over "anything that can be iterated" or "anything that
can be compared" requires a trait — a named bag of operation signatures that a type can
claim to implement. Koan has no surface for this. The host-side
[`ktraits.rs`](../src/dispatch/ktraits.rs) (`Parseable`, `Iterable`, `Monadic`) gives the
runtime its own vocabulary; user code is denied the analog and has to write
per-concrete-type variants of every function.

A second issue surfaces simultaneously: dispatch priority within a keyword bucket. With
seven host types, signature specificity is a finite-set comparison. With traits,
specificity becomes a partial order over a lattice that grows as user code grows. Two
candidates may both match a call with neither strictly more specific than the other, and
the comparator has no rule to break the tie.

**Impact.**

- *No abstraction over types.* Every function that wants to be polymorphic either falls
  back to `Any` (losing all type information) or duplicates per concrete type.
- *Operators can't generalize.* Group-based operators (next roadmap section) need a trait
  for `Group<T>` to declare paired ops over arbitrary types. Without traits, the operator
  registry stays flat.
- *Dispatch ambiguity has no rule.* If `Point` implements both `Iterable` and
  `Comparable` and a function has two overloads — one taking `Iterable`, one taking
  `Comparable` — the dispatcher has no principled answer today. Spelling out the rule
  before the first ambiguous case ships is cheaper than retrofitting it later.

**Directions.** None decided.

- *Surface form.* `TRAIT Iterable = (next: Function<Self, Option<T>>)` (or similar)
  declares a named set of required operations. Mechanically a `KFunction` with a fixed
  signature, registered alongside `STRUCT` and `UNION`.
- *Structural satisfaction.* Any type whose method set covers the trait's required
  signatures automatically satisfies it — no separate `IMPL` declaration needed. Cheaper
  for users; risks accidental satisfaction when a method name happens to collide.
  Explicit-impl is the safer alternative if accidental satisfaction proves a real
  problem in practice.
- *Dispatch priority within a bucket.* The ordering is `concrete > trait > Any`. Ties
  between two traits with no subtype relationship need a declared rule — candidates:
  declaration order, alphabetical on trait name, or an explicit priority attribute on the
  function definition. Pick one and document it; surprise behavior here is much worse
  than a verbose rule.
- *Trait objects vs. monomorphization.* When a function takes a trait-typed parameter,
  does the runtime carry a trait-object pointer (one dispatch path, vtable lookup at
  call) or specialize per concrete type at call time (multiple dispatch paths, faster but
  larger)? Koan's tree-walking dispatcher leans toward the first.

## Dependencies

**Requires:**
- [Per-type identity for structs and methods](per-type-identity.md) — without it a trait
  can be implemented "for `Struct`" but not "for `Point` specifically."

Container type parameterization is shipped — `Iterable<T>` and `Group<T>` are now
expressible at the signature layer; this work needs to add the trait surface that uses
them.

**Unblocks:**
- [Trait inheritance](trait-inheritance.md)
- [Group-based operators](group-based-operators.md)
- [Static type checking and JIT compilation](static-typing-and-jit.md)
