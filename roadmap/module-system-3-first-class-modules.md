# Module system stage 3 — First-class modules

**Problem.** Stages 1-2 give an explicit module language: modules are named
declarations manipulated via the explicit module syntax. They are not values.
Stage 3 makes them first-class — packed into ordinary values, passed to
functions, returned, and unpacked back into the module language at use sites.
This is the substrate stage 5 (modular implicits) builds on.

**Impact.**

- *No dynamic module dispatch.* A function cannot take "any module satisfying
  signature S" as an argument; it can only take a value. Plugin systems,
  runtime polymorphism, and ad-hoc strategy patterns all want
  modules-as-values.
- *Stage 5 has no substrate.* Modular implicits work by the compiler
  implicitly threading a module value to a function that declared an implicit
  module parameter. Without first-class modules there is nothing to thread.
- *No module-as-data idioms.* Configuration objects, capability objects, and
  other "bundle of typed operations passed at runtime" patterns require
  values that carry a signature.

**Directions.** None decided.

- *Pack syntax.* A construct that turns a structure into a value of module
  type, recording the signature alongside. Following stage 1's keyword-style
  adaptation.
- *Unpack syntax.* A construct that brings the module value back into the
  module language at a fresh binding. The unpacked types are abstract on each
  unpack — same generativity story as stage 2.
- *Module-value type representation.* A new `KType` variant carrying the
  signature, alongside the existing host types and stage 1's module-defined
  types.
- *Static signature requirement at unpack.* Type-checking the body of an
  unpack needs the signature available statically. Either require an explicit
  ascription at unpack, or infer from the value's known type when context
  determines it.
- *Interaction with the scheduler's phase boundary.* Pack and unpack are
  value-language operations whose typing depends on the module language. The
  inference-as-scheduler-node design from stage 1 must accommodate them.

## Dependencies

**Requires:**
- [Stage 2 — Functors](module-system-2-functors.md)

**Unblocks:**
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md)
