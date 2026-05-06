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

- *Type-local namespace.* `Point.translate` and `Vector.translate` coexist as bare names;
  methods land per-type rather than as globally-unique free functions.
- *Behavior alongside fields.* [ATTR](../src/dispatch/builtins/attr.rs) gives `p.x` (field
  read); `p.method()` becomes the symmetric story once per-type dispatch has a carrier
  to attach method tables to.
- *Substrate for trait dispatch.* The trait work that follows wants to dispatch on "is
  this specifically a `Point`?" — the per-type identity introduced here is what makes
  that specialization expressible.

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
- [Static type checking and JIT compilation](static-typing-and-jit.md)

Per-type identity is the prerequisite for the trait entries — methods are the natural
first consumer because they exercise the new dispatch hook without the additional
complication of structural matching.
