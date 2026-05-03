# Container type parameterization

**Problem.** [`KType`](../src/dispatch/kfunction.rs) is a flat tag for `Dict`, `List`,
`Function`, and `Future` — none of them carry inner-type information. A signature can say
"list" but not "list of numbers"; "function" but not "function from number to string."
Per-param annotations get user types into signatures at all; this entry extends them to
nested types so the four parameterizable host types stop being opaque.

**Impact.**

- *Signatures lose information at the boundary.* A function over `List<Point>` and one
  over `List<String>` have the same dispatch shape today.
- *No generic functions over inner types.* A `map`-like builtin has no way to say
  "`List<T>` + `Function<T, U>` -> `List<U>`" because there is no `T`.
- *`Future` is paper-typed.* The async substrate (when it lands) wants `Future<T>` so
  awaiting yields a typed result. Without parameterized types, every future is
  `Future<Any>`.

**Directions.** None decided.

- *Carrier shape.* Either store inner types on each `KType` variant directly
  (`KType::List(Box<KType>)`, `KType::Function { args, ret }`, etc.) or wrap with a
  `KType::Parameterized { base, params }` indirection. The first is cheaper and matches
  how host types are checked today; the second is more uniform but adds an indirection on
  every type comparison.
- *Surface syntax.* Angle-bracket (`Dict<String, Number>`) is familiar but collides with
  the comparison operators. Bracket form (`Dict[String, Number]`) or whitespace form
  (`Dict of String Number`) avoid the collision. Pick whichever the parser can
  disambiguate without lookahead surgery.
- *Variance.* Whether `List<Cat>` should be acceptable where `List<Animal>` is expected
  is the load-bearing semantic question. Invariant by default is safest; declaration-site
  variance markers can ship later if a real use case demands them.
- *Inference for literals.* A `[1, 2, 3]` literal needs an inferred element type. The
  obvious rule (the join of the element types) works for homogeneous lists; mixed lists
  fall back to `List<Any>` or error.

## Dependencies

**Requires:**
- [Per-parameter type annotations](per-param-type-annotations.md) — uses the same parser
  hook, extended to read nested types.

**Unblocks:**
- [Per-type identity for structs and methods](per-type-identity.md)
- [`TRAIT` builtin for structural typing](traits.md)
- [Static type checking and JIT compilation](static-typing-and-jit.md)

Independent of per-type identity for user structs: `List<Point>` distinguishes from
`List<Number>` even while `Point` still lives under the `KType::Struct` umbrella.
