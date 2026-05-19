# Error-handling surface follow-ups

Surface-level error-handling work deferred while the dispatcher's structured
`KError` plumbing landed. Several related items share this file because the
type-system and dispatcher decisions for one constrain the others. See
[design/error-handling.md](../../design/error-handling.md) for the shipped
substrate and the privilege-boundary principle.

**Problem.** User code has no in-language error-handling surface. The
primary user-side story — a function returns `Result<Ty, Er>` for a
user-defined error type `Er`, callers destructure with a match-form —
needs a stdlib `Result` module that doesn't exist yet (it depends on
functors, which have shipped but have no library built on them). With no
`Result`, user code can't name what it may fail with in its signature,
can't bind an error via `LET`, and can't pass one as an arg. The shipped
`TRY-WITH` recovers from interpreter faults but is not that surface; its
`user` arm has no constructor on the user side, so even the bridge from
user code into the catch machinery is open. Frames are textual summaries
— no `file:line`. A top-level failure ends the session.

**Impact.**

- *Typed user-error returns.* A function's signature carries which
  user-error values it may raise via `Result<Ty, Er>`, so callers reason
  locally and the type system enforces the discipline.
- *Errors as first-class values.* User code holds and inspects error
  values via `LET` and passes them as arguments.
- *Privilege boundary.* User code cannot impersonate runtime errors; the
  bridge from builtin to user is explicit catch-and-reraise inside a TRY
  arm.
- *Locatable error frames.* Frames carry `file:line` rather than textual
  summaries.
- *REPL ergonomics.* A top-level failure no longer ends the session; the
  next expression still runs.

**Directions.**

- *Two tiers with a privilege boundary — decided.* Builtin errors (every
  `KErrorKind` except `User`) are constructed only by the runtime; user
  code cannot raise them. They propagate ambiently through the existing
  `Forward` chain. `KErrorKind` is a closed set; `User` is the only
  variant whose payload is user-extensible.
- *Typed user errors via `Result<Ty, Er>` — decided.* A function that may
  raise user errors returns `Result<Ty, Er>` for a user-defined error type
  `Er`. `RAISE` produces a value of `Er`; the runtime carries it as
  `KErrorKind::User(KObject)` through the propagation channel above.
- *Catch as non-exhaustive match — decided per
  [design/error-handling.md](../../design/error-handling.md).* The shipped
  `TRY-WITH` form covers the builtin kinds and a `user` arm; the open
  stdlib work below extends the `user` arm to user-defined variants. A
  catch arm may construct a user-error value and reraise — the only
  mechanism by which a builtin error is lifted into the type system.
- *"Type-language binder expected" diagnostic vocabulary — open.* The
  bare-leaf arm of `elaborate_type_expr` rejects identifier-class names
  in type-position slots (shipped substrate; see
  [design/typing/elaboration.md](../../design/typing/elaboration.md)). The diagnostic
  needs vocabulary that names the surface "modules-as-types" layering
  (per
  [design/typing/functors.md](../../design/typing/functors.md))
  without leaking scheduler internals. Pick wording — candidates: "type-
  language binder expected", "name is value-language only", or similar —
  with a hint pointing at the Type-class spelling convention.

The remaining items are scoped sub-tasks for implementation rather than
design choices:

- *Errors as first-class values.* `KObject::Err` lets errors bind via
  `LET` and pass as args. Substrate for the typed surface; needs the
  dispatcher to either short-circuit through error-typed slots or splice
  errors into them.
- *stdlib `Result<Ty, Er>` module (depends on functors).* A
  functor-produced module over the shipped module-system substrate
  ([design/typing/functors.md](../../design/typing/functors.md));
  the typed user-error surface consumes it and feeds TRY's `user` arm.
- *`RAISE expr` builtin* to construct a `KErrorKind::User(KObject)` from
  a user-error value. Requires errors-as-values and `Result<Ty, Er>` so the
  value has a typed home.
- *Source spans on `KExpression`* so frames carry `file:line`. Independent
  of the type-system work.
- *Continue-on-error after the first top-level failure*, useful for a
  future REPL. Independent of the type-system work.

## Dependencies

**Requires:** none — `Result<Ty, Er>` and `RAISE` extend the shipped
TRY-WITH surface (see [design/error-handling.md](../../design/error-handling.md))
through its `user` arm and need stdlib functors;
errors-as-values, source spans, and continue-on-error are
independent of both.

**Unblocks:** none.
