# Error handling

Errors in Koan are values that propagate implicitly through the scheduler.
The runtime substrate handles structured propagation along the dependency
edges — when a slot writes an `Err`, the notify-walk wakes its dependents,
which short-circuit and propagate (appending a `Frame` per step) — and
surfaces errors at the top level. The in-language surface for *handling*
errors has two parts: [`Result`](#result) values that user code returns and
destructures, and [`TRY-WITH`](#try-with) / [`CATCH`](#catch) for recovering
from interpreter faults. Remaining surface work — stdlib `Result` helpers and
REPL continue-on-error — is tracked under [Open work](#open-work).

## `BodyResult::Err` and `KError`

A builtin body returns one of `Value`, `Tail`, or `Err(KError)` (see
[execution-model.md](execution-model.md)).

[`KError`](../src/machine/core/kerror.rs) is a struct:

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
- `TypeClassBindingExpectsType` — `LET <Type-class> = <non-type>` rejected at
  bind time rather than at downstream elaboration.
- `User` — landing pad for user-side error construction; see open work.

## Propagation

The scheduler walks errors along the dependency edges: a slot's terminal
`Err` write triggers the notify-walk, which wakes each waiting `Bind` /
`Combine` / `Lift` consumer; those short-circuit, append a `Frame`, and
write the error into their own slot. Errors flow to the top level; the CLI
formats them to stderr with the frame chain via `KError`'s `Display` impl.

Dispatch failures (no match, ambiguous overload, arity mismatch in bind) flow
through the same channel as builtin errors:
[`Scope::resolve_dispatch`](../src/machine/core/scope.rs) returns a
`ResolveOutcome` whose `Ambiguous` and `Unmatched` arms the scheduler driver
converts to `Err(KError)` with `KErrorKind::AmbiguousDispatch` /
`DispatchFailed`, and `KFunction::bind` returns `Result<KFuture, KError>` on
arity mismatch.
[`Scheduler::execute`](../src/machine/execute/scheduler.rs) and
[`interpret`](../src/machine/execute/interpret.rs) return `Result<(), KError>` to
complete the surfacing.

## `try_args!` macro

The default form
[`try_args!(bundle; arg: Variant, ...)`](../src/builtins.rs)
auto-constructs a structured `TypeMismatch` on failure — the common case in
builtin bodies. The override form `try_args!(bundle, return $err; ...)` is
preserved for the rare site that wants something custom (e.g., a `ShapeError`
for an out-of-bounds index, or a `MissingArg` with a hand-crafted message).

## Subtlety: TCO collapses frames

A user-fn whose body tail-calls another user-fn ends up with only the inner
function in the trace, because the slot's `function` field is replaced at TCO
time (see [execution-model.md](execution-model.md)). Non-tail-call positions —
e.g., a sub-`Dispatch` inside a parens-wrapped sub-expression — preserve the
outer frame: the slot rewrites to a `Lift` shim that retains the call frame
and `function` label until the spawned `Bind` notifies, so an error landing
on the Lift carries the outer function's frame. This matches how other
languages with TCO behave.

## User-side surface

A user-written function that can fail returns `Result<Ty, Er>` for a
user-defined error type `Er` — `Result` is a builtin parameterized type
(like `List` / `Dict`) with `ok :T` and `error :E` variants (see
[`Result`](#result)).
Callers destructure the `Result` with a match-form and handle the `ok`
and `error` arms locally. This is the primary error-handling idiom in
user code: errors flow through the type system, signatures name what
can go wrong, and there is no implicit catch.

Alongside `Result`, **interpreter faults** (every `KErrorKind` except
`User`) are raised only by the runtime — `UnboundName`, `TypeMismatch`,
`DispatchFailed`, and the rest. User code cannot construct them. They
propagate ambiently along the dependency edges through the notify-walk
and surface at the top level. `TRY-WITH` (below) lets code that needs to
recover from them — a REPL, a sandbox, a defensive wrapper — intercept
the propagation and dispatch on the `KErrorKind`; `CATCH expr` lifts a
single fault into a `Result<T, KError>` so the caller can bind it via
`LET`, MATCH on `ok` / `error`, and (inside the `error` arm) dispatch
on the per-kind tag carried in the payload (see [`CATCH`](#catch)).
The shared `Result` shape means a function that wraps a `CATCH` and a
function with a typed user-error return present the same destructuring
surface to callers.

The two tiers don't cross: a user-typed error flows as a `Result<_, _>`
value and never enters the interpreter-fault channel; an interpreter
fault propagates ambiently and only becomes a value when explicitly
caught. The asymmetry is forced by koan's dispatch model: with multiple
dispatch plus open extension, no signature can statically guarantee the
absence of `DispatchFailed`, so builtin errors stay ambient while user
errors carry the type discipline. `KErrorKind` itself is a closed set.

## `Result`

`Result` is a builtin parameterized type — a two-variant tagged union over two
type parameters, `ok :T` and `error :E`. It is the shared return-type shape for
[`CATCH`](#catch) (`Result<T, KError>`) and for user functions with typed error
returns (`Result<T, MyErr>`). It is *not* a module-system functor: functors
produce modules, whereas `Result` is a type constructor producing a tagged-union
value.

It is registered once in the root scope by
[`result::register`](../src/builtins/result.rs), dual-written the way a `UNION`
declaration is:

- the **type side** (`bindings.types`) holds a `TypeConstructor` identity with
  parameters `T` and `E`, so a slot annotated `:(Result T E)` resolves through
  the resolver's constructor-application arm;
- the **value side** (`bindings.data`) holds a `TaggedUnionType` carrier with
  schema `{ok, error}` (both `Any`), so `Result (ok v)` / `Result (error e)`
  construct values through the same path `UNION`-declared constructors use.

The carrier's `(name, scope_id)` identity uses the root scope's `ScopeId` — the
scope that owns the registration, not `ScopeId::SENTINEL` — so every `Result`
value shares one nominal identity and MATCHes uniformly. Because the name is
registered at prelude, a user `UNION Result = (...)` is rejected with `Rebind`:
the binder-placeholder install refuses a name already bound to a non-function
value.

Type parameters are erased at runtime, as for `List` / `Dict`: `Result<T, KError>`
and `Result<T, MyErr>` share one value identity, and `:(Result T E)` is not a
runtime-checkable slot type — `:Tagged` is. Carrying type arguments at runtime is
tracked under
[runtime carriers for type parameters](../roadmap/predicate_typing/runtime-type-parameter-carriers.md).

## `TRY-WITH`

`TRY-WITH` recovers from *interpreter-raised faults* — the runtime errors
listed under [Exposed variants](#exposed-variants). It is not user code's
normal error path; that is `Result` destructuring. Reach for `TRY-WITH`
when defensive recovery is the point of the code: a REPL that wants to
keep running past a typo, a sandbox evaluating untrusted input, a
top-level wrapper that converts a `DispatchFailed` into a typed
user-error and reraises.

The catch surface is the [`TRY`](../src/builtins/try_with.rs) builtin:

```
TRY (<expr>) WITH (
  ok            -> <body>
  type_mismatch -> <body>
  ...
  _             -> <body>   ; optional wildcard
)
```

Both slots are lazy `KExpression`s. `<expr>` is evaluated in a catching
sub-context: on success the `ok` arm runs with `it` bound to the bare
success value; on failure the arm matching the `KErrorKind` runs with `it`
bound to a per-variant payload struct. No matching arm and no `_` →
re-raise the original `KError`. Success with no `ok` arm and no `_` →
synthetic `ShapeError("TRY missing ok arm")`.

The branch walker is shared with `MATCH`
([`branch_walk::find_branch_body`](../src/builtins/branch_walk.rs));
TRY opts into `_` wildcard support, MATCH does not. The catching wiring
is a new scheduler primitive `NodeWork::Catch` (see
[execution-model.md](execution-model.md)): `add_catch` waits on a watched
slot and hands its `Result<&KObject, KError>` to a host closure that
decides whether to recover or re-raise. Unlike `Combine`, an errored dep
does not short-circuit — TRY's finish always runs.

### Exposed variants

User-meaningful subset. Each error arm's `it` is a Struct under one
shared `KError` tagged-union identity ([`KError::to_tagged`](../src/machine/core/kerror.rs))
with heterogeneous payload shape per arm; `ok` binds `it` to the bare
success value (no wrapper):

| Tag | `it` shape |
|---|---|
| `ok` | the success value (bare, not a struct) |
| `type_mismatch` | `{arg :Str, expected :Str, got :Str, frames :List<Str>}` |
| `missing_arg` | `{name :Str, frames :List<Str>}` |
| `unbound_name` | `{name :Str, frames :List<Str>}` |
| `arity_mismatch` | `{expected :Number, got :Number, frames :List<Str>}` |
| `ambiguous_dispatch` | `{expr :Str, candidates :Number, frames :List<Str>}` |
| `dispatch_failed` | `{expr :Str, reason :Str, frames :List<Str>}` |
| `shape_error` | `{message :Str, frames :List<Str>}` |
| `parse_error` | `{message :Str, frames :List<Str>}` |

`frames` is a `List<Str>`, each entry rendered `"in <expression> (<function>)"`.

The four dispatcher-internal kinds (`rebind`, `duplicate_overload`,
`type_class_binding_expects_type`, `type_identity_pending_at_dispatch`)
are only catchable via `_`; `it` is then bound to a minimal
`{kind :Str, message :Str, frames :List<Str>}` struct.

## `CATCH`

`CATCH <expr>` lifts a single interpreter fault into a [`Result`](#result) value
rather than letting it propagate. It is the opt-in, expression-position
counterpart to [`TRY-WITH`](#try-with): where `TRY-WITH` forces the caller to
spell out catch arms at the catch site, `CATCH` hands back a `Result<T, KError>`
the caller binds with `LET`, passes as an argument, or returns:

- `ok(v)` on success, where `v` is the bare success value;
- `error(e)` on failure, where `e` is
  [`KError::to_tagged`](../src/machine/core/kerror.rs)'s value — still carrying
  the per-`KErrorKind` tag and payload struct, so per-kind dispatch is reached by
  MATCH-ing `e` after destructuring the `Result`.

The [`CATCH`](../src/builtins/catch.rs) builtin reuses the same scheduler
primitive as `TRY-WITH` (`add_catch` / `CatchFinish`): it schedules `<expr>` as a
catching sub-dispatch and registers a finish closure that wraps the outcome in
the `Result` carrier. The carrier's `scope_id` is captured at registration time,
not read from the call-site scope, so a `CATCH`-produced `Result` and a
`Result (...)`-constructed one share nominal identity regardless of where the
`CATCH` runs. `LET` and other eager slots still short-circuit on errors, so the
lift stays opt-in.

## Open work

[Error-handling surface follow-ups](../roadmap/libraries/error-handling.md)
tracks the related items:

- **stdlib `Result` helpers** — `map`, `bind`, `unwrap_or`, etc.;
  gated on files-and-imports and the standard library. The `Result`
  constructor itself is builtin (above), so user code can use it
  before these helpers ship.
- **Continue-on-error** — top-level continuation past a single failed
  expression, useful for the CLI's batch mode.
