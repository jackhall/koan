# Code names

A class of names that holds only code, read where a shape is built, so code
named once can be built where it is used.

**Problem.** Code has no name class of its own. A quote bound by `LET` sits in
a value name, and the [shape builder](../../src/scope/README.md#three-tiers)
reads no value where it builds a shape, so a callable's body can only be a
quote written in place ([code as values](code-values.md)); code named once
cannot be the body of two callables. A module exports code only as a value,
and a `SIG` has no member for code.

**Acceptance criteria.**

- A sigiled name holds only code and resolves to a coordinate where the shape
  is built; a sigiled parameter binds per call.
- A shape is built from code only where it traces that code back to written
  quotes, as `USING … SCOPE` traces its operand; code it cannot trace is shaped
  when it is evaluated.
- A `SIG` declares a code member with `VAL`, as a slot, or with `LET`, as
  manifest code.

**Directions.**

- *A code naming channel — decided.* A name class of its own, marked by a sigil
  since case is spent, holding only code. A sigiled parameter binds per call as
  a type parameter does: a quote's shape travels with its value, so running
  code needs nothing from the shape that runs it, and only building a new shape
  from code needs the code where the shape is built. Elaboration may evaluate
  code, as a subdispatch does, provided that code is deterministic, since what
  elaboration builds is interned. A `VAL` code member is sealed as a value is
  behind its type: a holder can run the code but not see which code it is.
- *The sigil's character — open.* `#` reads as code, but `#y` sits one pair of
  parens from `#(y)`, the quote of the symbol `y`.
- *What a sigiled name resolves to — open.* Narrowing the family alone is
  admission's job, so the sigil needs a resolution rule of its own. One
  candidate: inside a quote, a sigiled name splices its code, while a lowercase
  name stays a captured reference.
- *A code binder in a module's self-signature — open.* A slot at its code kind,
  as a value binder is, or a manifest member, as a type binder is.
- *Manifest code in a `SIG` — open.* Two textually identical `SIG`s are one
  type, so manifest code that captures from its surroundings would make
  identical text differ. Code equality follows bindings
  ([quotes resolve where they are written](eval-scope.md)), so either manifest
  code captures nothing, or a `SIG`'s identity compares its code by something
  other than equality.

## Dependencies

**Requires:**

- [Code as values](code-values.md) — the code kinds a sigiled name holds.
- [Quotes resolve where they are written](eval-scope.md) — a sigiled name inside a quote resolves beside the quote's bindings.
- [Code splicing](code-splicing.md) — a shape built at run time for code the builder cannot trace.

**Unblocks:** none — a leaf.
