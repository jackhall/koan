# Module system stage 7 — Syntax tuning and witness types

**Problem.** Stage 5 ships with explicit module application as the only
disambiguation form, deliberately ugly so it does not accidentally become
the final answer. Stage 7 collects the patterns that emerge from real
stage-5 code and designs proper sugar against them. It also lands the
**witness types** escape hatch: an opt-in mechanism for users who want
stronger-than-probabilistic coherence at the cost of more verbose types.

This is the polish stage; everything below is additive over stages 1-6 and
breaks no existing programs.

**Acceptance criteria.**

- Routine implicit disambiguation is written with block-scoped binding,
  module-level priority, or selective imports rather than stage 5's
  deliberately ugly placeholder form.
- A type constructor carrying a witness as a module-kind slot (see
  [design/typing/functors.md](../../design/typing/functors.md#type-expressions-and-constraints))
  yields distinct types for distinct module values bound to that slot, and
  mixing two such types is a type error.
- Type inference elides the module-kind witness slot in source, so a program
  that does not opt into witness types writes no slot and keeps stage 6's
  behavior.

**Directions.**

- *Surveyed sugar candidates — deferred.* Block-scoped binding (apply a
  chosen implicit to many calls in a region), module-level priority (one
  module declared canonical, others warn), selective imports (`use
  mylib::sort except int_ord_reverse`). Selection deliberately deferred
  until stage 5's placeholder syntax has seen real use — pick from
  observed patterns, not from imagination.
- *Explicit-application syntax — deferred.* Stage 5 ships a placeholder;
  stage 7 fixes the form against the patterns of use. May coincide with
  the block-scoped binding form (block applies to many calls; explicit
  applies to one). Same deferral discipline as the sugar candidates above.
- *Witness type encoding — decided per [design/typing/functors.md § Type expressions and constraints](../../design/typing/functors.md#type-expressions-and-constraints).*
  The type constructor declares a module-kind slot whose value carries
  through type identity — a `Set` with `Elt` pinned to `Number` becomes
  `(Set WITH {Elt = Number, Ord = :(TYPE OF int_ord)})` when `int_ord` is the
  implicit used. Distinct module values means distinct types means
  cannot mix. Type inference must elide the module-kind slot in source
  so users only write it when they want.
- *Opt-in mechanism for witness types — decided in concept, syntax open.*
  Witness types are not the default. A signature marks itself as
  participating and consumers of that signature carry the phantom. The
  decision is per-signature, not per-use; the concrete marking syntax is
  unsettled.

## Dependencies

The deferred-syntax discipline relies on stage 5 having shipped with a
deliberately ugly placeholder; designing this stage before stage 5 has
real-world use defeats the purpose.

**Requires:**

- [Stage 5 — Modular implicits](modular-implicits.md)

**Unblocks:** none — stage 7 is a leaf.
