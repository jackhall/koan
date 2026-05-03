# Error handling

Errors in Koan are values that propagate implicitly through the scheduler.
There is no in-language try/catch — "catching" is for builtins to do, not
surface syntax. The runtime substrate is in place; no catch-builtin has shipped
yet.

## `BodyResult::Err` and `KError`

A builtin body returns one of `Value`, `Tail`, or `Err(KError)` (see
[execution-model.md](execution-model.md)).

[`KError`](../src/dispatch/kerror.rs) is a struct:

```rust
struct KError {
    kind: KErrorKind,
    frames: Vec<Frame>,
}
```

with these `KErrorKind` variants:

- `TypeMismatch` — arg or return type didn't match.
- `MissingArg` — required argument absent from the call.
- `UnboundName` — identifier resolves to nothing.
- `ArityMismatch` — wrong number of args at bind.
- `AmbiguousDispatch` — two or more functions matched at equal specificity.
- `DispatchFailed` — no function matched.
- `ShapeError` — list/dict shape didn't fit (e.g., index out of bounds).
- `ParseError` — produced by the parser, propagated through the same channel.
- `User` — landing pad for a future `RAISE`-style builtin.

## Propagation

The scheduler walks errors through `Forward` chains, short-circuiting any
`Bind` whose dependency errored and appending a `Frame` per propagation step.
Errors flow to the top level; the CLI formats them to stderr with the frame
chain via `KError`'s `Display` impl.

[`Scope::dispatch`](../src/dispatch/scope.rs) and `KFunction::bind` return
`Result<KFuture, KError>` — dispatch failures (no match, ambiguous overload,
arity mismatch in bind) flow through the same channel as builtin errors.
[`Scheduler::execute`](../src/execute/scheduler.rs) and
[`interpret`](../src/execute/interpret.rs) return `Result<(), KError>` to
complete the surfacing.

## `try_args!` macro

The default form
[`try_args!(bundle; arg: Variant, ...)`](../src/dispatch/builtins.rs)
auto-constructs a structured `TypeMismatch` on failure — the common case in
builtin bodies. The override form `try_args!(bundle, return $err; ...)` is
preserved for the rare site that wants something custom (e.g., a `ShapeError`
for an out-of-bounds index, or a `MissingArg` with a hand-crafted message).

## `null()` is intentional-only

Before this work, builtin bodies returned `null()` as a "something went wrong"
sentinel. After: `null()` means *intentional* null only. The two surviving call
sites are `IF false THEN x` skipping its lazy slot and `PRINT`'s no-useful-
return value. Every other former `null()` site became `err(KError::...)`.

## Design constraint: no in-language try/catch

The user-explicit constraint is that errors are values that propagate
implicitly, and "catching" is for builtins to do, not surface syntax. The
runtime substrate is established; the catch-builtin shape is intentionally
deferred until the type system has the surface to express it (see open work).

## Subtlety: TCO collapses frames

A user-fn whose body tail-calls another user-fn ends up with only the inner
function in the trace, because the slot's `function` field is replaced at TCO
time (see [execution-model.md](execution-model.md)). Non-tail-call positions —
e.g., a sub-`Dispatch` inside a parens-wrapped sub-expression — preserve the
outer frame via the `frame_holding_slots` finalize path. This matches how other
languages with TCO behave; future work could add per-step frame accumulation if
traces lose too much detail in practice.

## Open work

[deferred-surface-items.md](../roadmap/deferred-surface-items.md) tracks several
related items:

- **Errors-as-values** — promote `KError` to a `KObject` variant so user code
  can hold and inspect them.
- **Catch-builtin** — the surface form for handling errors. Depends on
  errors-as-values and on the type system having the right surface.
- **`RAISE`** — user-side error construction; populates the `User` arm.
- **Source spans on `KExpression`** — frames currently can't point to a
  line/column in source.
- **Continue-on-error** — top-level continuation past a single failed
  expression, useful for the CLI's batch mode.
