# Bare parenthesized return annotations

Make the bare `-> (LIST OF Str)` / `-> (FN …)` return annotation behave like its
sigiled twin, which it panics on today.

**Problem.** The two ways to annotate a constructed return type diverge. The
**sigiled** form `-> :(FN (x :Number) -> Number)` elaborates and runs — a closure
factory can declare and return a typed function (see the closure example in
[tutorial/04-functions.md](../../tutorial/04-functions.md)). The **bare** form,
parenthesized without the `:` sigil, instead panics at definition: the keyworded
submit path hits a `ResolveOutcome::Deferred` for the return slot while
`pre_subs` is non-empty, tripping the `debug_assert!` at
[`keyworded.rs`](../../src/machine/execute/dispatch/keyworded.rs) ("Deferred
resolve_dispatch implies no binder pick at submit time; `pre_subs` must be empty
here"); a release build instead proceeds on the violated invariant.

Every parenthesized type constructor in return position trips it, not only `FN`:
`-> (FN (y :Number) -> Number)`, `-> (LIST OF Str)`, and `-> (MAP Str -> Number)`
all panic, while each sigiled counterpart runs. The bare form parallels how every
other return type is written (`-> Number`, not `-> :Number`), so it is the natural
thing to reach for and a likely papercut.

**Acceptance criteria.**

- A function declaring a bare parenthesized return type — `-> (LIST OF Str)`,
  `-> (MAP Str -> Number)`, `-> (FN (…) -> …)` — elaborates and runs without
  panicking, in both debug and release builds, at parity with the sigiled form.
- The body's returned value is checked against the declared type; a return that
  doesn't match surfaces a `TypeMismatch` for `<return>`, like every other
  return-type violation.
- A closure factory written with the bare form — `FN (ADDER n :Number) -> (FN (x :Number) -> Number) = (…)`
  — type-checks its returned closure and is callable.

**Directions.**

- *Where the deferred return slot is resolved — open.* The panic is a bare
  parenthesized return annotation reaching the keyworded submit path as
  `ResolveOutcome::Deferred` with non-empty `pre_subs`, where the sigiled form
  does not. Either route the bare parenthesized part through the same elaboration
  the sigiled form takes (so it resolves before submit), or widen the submit-time
  invariant to admit a deferred return slot. Decide after tracing why the bare
  part defers where the sigiled part doesn't.
- *Whether the fix is per-constructor or shared — open.* The three constructors
  panic through one assertion, which suggests a single elaboration seam rather
  than three fixes; confirm the shared root before scoping the change.

## Dependencies

**Requires:** none — foundation (a self-contained dispatch/elaboration fix).

**Unblocks:** none.
