# Per-parameter type annotations

**Problem.** Argument types in user-fn signatures are uniformly `Any` today. The parser
already accepts a `Type` token in declarations and the dispatcher already matches host
builtins on per-param types — the gap is purely that user-fn signatures don't thread the
annotation through. A user can write `FN add (x, y) = x + y` but not `FN add (x: Number,
y: Number) -> Number`, so the runtime can't reject `add("hi", 3)` at the call boundary.

**Impact.**

- *Type errors surface at use, not at the call.* `add("hi", 3)` only fails when `+` looks
  for a `String + Number` overload — the failure points at `+`, not at `add`'s contract.
- *Overloading on user-fn signatures is impossible.* Without per-param types, two
  user-defined `add`s can't differ on argument shape — the dispatcher sees both as
  `(Any, Any) -> Any`.
- *Foundation for everything downstream.* Container parameterization, methods, and traits
  all assume signatures can carry user-supplied types. Validating the parser→dispatcher
  path with the simplest possible payload (one `Type` per slot, all leaf types) derisks
  the rest of the sequence.

**Directions.** None decided.

- *Surface form.* `FN add (x: Number, y: Number) -> Number` is the obvious match for the
  existing `Type` token class. The arrow-return syntax extends naturally to the existing
  return-type-enforcement substrate.
- *Defaulting.* An omitted annotation continues to mean `Any` (today's behavior), or
  becomes a parse error. The first preserves existing programs; the second forces
  explicit signatures, which is more pedagogically honest but a one-time migration.
- *Where the check happens.* Either at call dispatch (existing path, just stop hard-coding
  `Any`) or as a prelude inside the function body before the body runs. Dispatch-time is
  cheaper and matches host builtins; prelude-time gives better error messages because the
  failure can name the param.

## Dependencies

**Unblocks:**
- [Container type parameterization](container-type-parameterization.md)
- [Per-type identity for structs and methods](per-type-identity.md)
- [`TRAIT` builtin for structural typing](traits.md)
- [Static type checking and JIT compilation](static-typing-and-jit.md)

First slice of the type/trait sequence. Self-contained — ships independently of every
downstream item. Lands as one PR.
