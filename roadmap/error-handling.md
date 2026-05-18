# Error-handling surface follow-ups

Surface-level error-handling work deferred while the dispatcher's structured
`KError` plumbing landed. Several related items share this file because the
type-system and dispatcher decisions for one constrain the others. See
[design/error-handling.md](../design/error-handling.md) for the shipped
substrate and the privilege-boundary principle.

**Problem.** Today's `KError` channel propagates every error kind
uniformly, but user code has no way to construct, hold, or handle errors.
The `User` `KErrorKind` arm is a placeholder with no constructor and no
matcher. There is no typed surface for "which user errors may this
function raise."

**Impact.**

- *In-language error handling.* User code recovers from runtime errors and
  resumes execution.
- *Typed user-error returns.* A function's signature carries which
  user-error values it may raise via `Result<Ty, Er>`, so callers reason
  locally and the type system enforces the discipline.
- *Privilege boundary.* User code cannot impersonate runtime errors; the
  bridge from builtin to user is explicit catch-and-reraise inside a match
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
- *Catch as non-exhaustive match — decided.* Arms cover whichever builtin
  kinds and user-error variants the caller wants to handle; anything else
  continues to propagate. A catch arm may construct a user-error value and
  reraise — the only mechanism by which a builtin error is lifted into the
  type system.
- *"Type-language binder expected" diagnostic vocabulary — open.* The
  bare-leaf arm of `elaborate_type_expr` rejects identifier-class names
  in type-position slots (shipped substrate; see
  [design/typing/elaboration.md](../design/typing/elaboration.md)). The diagnostic
  needs vocabulary that names the surface "modules-as-types" layering
  (per
  [design/typing/functors.md](../design/typing/functors.md))
  without leaking scheduler internals. Pick wording — candidates: "type-
  language binder expected", "name is value-language only", or similar —
  with a hint pointing at the Type-class spelling convention.

The remaining items are scoped sub-tasks for implementation rather than
design choices:

- *Errors as first-class values.* `KObject::Err` lets errors bind via
  `LET` and pass as args. Substrate for the typed surface; needs the
  dispatcher to either short-circuit through error-typed slots or splice
  errors into them.
- *`Result<Ty, Er>` as a functor.* A functor-produced module over the
  shipped module-system substrate
  ([design/typing/functors.md](../design/typing/functors.md));
  the typed user-error surface consumes it.
- *Catch-builtins.* The match-form surface. Pattern arms over selected
  `KErrorKind` variants and over the user-error type's variants, with
  unmatched arms propagating. Requires errors-as-values and `Result<Ty, Er>`.
- *`RAISE expr` builtin* to construct a `KErrorKind::User(KObject)` from
  a user-error value. Requires errors-as-values and `Result<Ty, Er>` so the
  value has a typed home.
- *Source spans on `KExpression`* so frames carry `file:line`. Independent
  of the type-system work.
- *Continue-on-error after the first top-level failure*, useful for a
  future REPL. Independent of the type-system work.

## Dependencies

**Requires:** none — `Result<Ty, Er>` runs against the shipped
module-system substrate; errors-as-values, source spans, and
continue-on-error are independent of the type-system work.

**Unblocks:** none.
