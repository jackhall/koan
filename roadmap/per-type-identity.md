# Per-type identity for structs and methods

**Problem.** `STRUCT Point = (x: Number, y: Number)` declares a record but cannot bundle
behavior with it. Functions that operate on a `Point` live alongside it as free
functions; there is no way to write `p.distance()` or attach an `area` method to
`Rectangle`. The deeper limitation is that `KObject::Struct { type_name, fields }`
collapses every user struct into one runtime variant under `KType::Struct`. The
dispatcher sees "this is a struct" but not "this is specifically a `Point`," so per-type
method tables have nowhere to attach.

These two pieces ship together. Per-type identity is a substrate shift with no
user-visible payoff on its own — it has to ship with its first consumer (methods) or it
sits dark. Methods can't be implemented without per-type identity. Bundling forces the
substrate to be designed against a real consumer.

**Impact.**

- *No type-local namespace.* A `Point.translate` and a `Vector.translate` cannot coexist
  as bare names — every method has to be a globally-unique free function.
- *Field access works, behavior does not.* [ATTR](../src/dispatch/builtins/attr.rs) gives
  `p.x` (field read); the symmetric `p.method()` requires per-type dispatch that the
  closed-enum carrier blocks.
- *Trait dispatch can't specialize.* Once traits land, they want to dispatch on "is this
  specifically a `Point`?" — which the umbrella `KType::Struct` prevents.

**Directions.** None decided.

- *Per-type identity carrier.* Either add a `KType::User(TypeId)` variant alongside the
  existing host types with a `Scope`-level registry of definitions, or replace `KType`
  entirely with a trait-object that host and user types both implement uniformly. The
  first is incremental; the second is cleaner but a bigger refactor.
- *Method declaration surface.* An `IMPL Point` block that registers methods, vs. inline
  `METHOD Point distance = ...` declarations, vs. attaching at struct definition time.
  The block form clusters related methods; inline keeps the surface flat and matches the
  existing one-definition-per-keyword style.
- *Method dispatch path.* Either reuse the existing dispatcher with the receiver as the
  first argument (`p.distance()` desugars to `distance(p)`), or maintain a separate
  per-type method table consulted only on `.`-syntax. The first reuses everything; the
  second cleanly separates "method" from "free function" if that distinction matters.
- *Self reference inside method bodies.* `self`, `this`, or implicit (each field name is
  in scope). Implicit is most concise but conflicts with same-named locals.

## Dependencies

**Requires:** none. Container type parameterization is shipped — method signatures get
parameterized containers in the same slots free functions do.

**Unblocks:**
- [Open issues from the leak-fix audit](leak-fix-audit.md)
- [`TRAIT` builtin for structural typing](traits.md)
- [Module system and directory layout](module-system.md)
- [Static type checking and JIT compilation](static-typing-and-jit.md)

Per-type identity is the prerequisite for the trait entries — methods are the natural
first consumer because they exercise the new dispatch hook without the additional
complication of structural matching.
