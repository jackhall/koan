# Function-typed return annotations

Make the bare `-> (FN …)` return annotation behave like the sigiled `-> :(FN …)`
form, which it panics on today.

**Problem.** The two ways to annotate a function-typed return diverge. The
**sigiled** form `-> :(FN (x :Number) -> Number)` elaborates and runs — a closure
factory can declare and return a typed function (see the closure example in
[tutorial/04-functions.md](../../tutorial/04-functions.md)). The **bare** form
`-> (FN (x :Number) -> Number)`, parenthesized without the `:` sigil, instead
panics at definition: the keyworded submit path hits a `ResolveOutcome::Deferred`
for the function-type return slot while `pre_subs` is non-empty, tripping the
`debug_assert!` at
[`keyworded.rs`](../../src/machine/execute/dispatch/keyworded.rs) ("Deferred
resolve_dispatch implies no binder pick at submit time; `pre_subs` must be empty
here"); a release build instead proceeds on the violated invariant. The bare form
parallels how every other return type is written (`-> Number`, not `-> :Number`),
so it is the natural thing to reach for and a likely papercut.

**Acceptance criteria.**

- A function declaring a bare `-> (FN (…) -> …)` return elaborates and runs
  without panicking, in both debug and release builds — parity with the sigiled
  `-> :(FN …)` form.
- The body's returned function is checked against the declared signature; a
  return whose function type doesn't match surfaces a `TypeMismatch` for
  `<return>`, like every other return-type violation.
- A closure factory written with the bare form — `FN (ADDER n :Number) -> (FN (x :Number) -> Number) = (…)`
  — type-checks its returned closure and is callable.

**Directions.**

- Where the deferred return slot is resolved — open. The panic is a bare
  function-type return annotation reaching the keyworded submit path as
  `ResolveOutcome::Deferred` with non-empty `pre_subs`, where the sigiled form
  does not. Either route the bare `(FN …)` part through the same elaboration the
  sigiled form takes (so it resolves before submit), or widen the submit-time
  invariant to admit a deferred return slot. Decide after tracing why the bare
  part defers where the sigiled part doesn't.

## Dependencies

**Requires:** none — foundation (a self-contained dispatch/elaboration fix).

**Unblocks:** none.
