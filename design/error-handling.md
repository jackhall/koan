# Error handling

Errors in Koan are values that propagate implicitly through the scheduler.
The runtime substrate handles structured propagation along the dependency
edges — when a slot writes an `Err`, the notify-walk wakes its dependents,
which short-circuit and propagate (appending a `TraceFrame` per step) — and
surfaces errors at the top level. The in-language surface for *handling*
errors has two parts: [`Result`](#result) values that user code returns and
destructures, and [`TRY-WITH`](#try-with) / [`CATCH`](#catch) for recovering
from interpreter faults. Remaining surface work — stdlib `Result` helpers and
REPL continue-on-error — is tracked under [Open work](#open-work).

## `Done(Err)` and `KError`

A builtin body's result lowers to an `Outcome`: a final value or an error
both ride `Done`, a tail rides `Continue` (see
[execution/README.md](execution/README.md)). A failure is `Done(Err(KError))`.

[`KError`](../src/machine/core/kerror.rs) is a struct:

```rust
struct KError {
    kind: KErrorKind,
    frames: Vec<TraceFrame>,
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
- `Rebind` — a second `LET` of a name already bound in the same scope.
- `DuplicateOverload` — an `FN` whose signature exactly matches a registered overload.
- `SchedulerDeadlock` — the scheduler reached a fixed point with work still outstanding.
- `User` — landing pad for user-side error construction; see open work.

## Propagation

The scheduler walks errors along the dependency edges: a slot's terminal
`Err` write triggers the notify-walk, which wakes each waiting consumer; a dep-finish
short-circuits, appends a `TraceFrame`, and writes the error into its own slot (a
catch instead recovers or re-raises). Errors flow to the top level; the CLI
formats them to stderr with the frame chain via `KError`'s `Display` impl.

Dispatch failures (no match, ambiguous overload, arity mismatch in bind) flow
through the same channel as builtin errors:
[`Scope::resolve_dispatch`](../src/machine/core/scope.rs) returns a
`ResolveOutcome` whose `Ambiguous` and `Unmatched` arms the scheduler driver
converts to `Err(KError)` with `KErrorKind::AmbiguousDispatch` /
`DispatchFailed`, and `KFunction::bind_args` returns `Result<Record<Held>, KError>` on
arity mismatch.
[`Scheduler::execute`](../src/machine/execute/run_loop.rs) and
[`interpret`](../src/machine/execute/runtime/interpret.rs) return `Result<(), KError>` to
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
time (see [execution/README.md](execution/README.md) and
[per-call-region/frames.md § TCO frame reuse](per-call-region/frames.md#tco-frame-reuse)).
Non-tail-call positions — e.g., a sub-`Dispatch` inside a parens-wrapped
sub-expression — preserve the outer frame: the consuming slot parks on the
sub-`Dispatch` as a dependency, and the dep-finish short-circuit retains the call
frame and `function` label, so an error landing on the dependency carries the
outer function's frame. This matches how other languages with TCO behave.

## User-side surface

A user-written function that can fail returns `Result<Ty, Er>` for a
user-defined error type `Er` — `Result` is a builtin parameterized type
(like `List` / `Dict`) with `Ok :T` and `Error :E` variants (see
[`Result`](#result)).
Callers destructure the `Result` with a match-form and handle the `Ok`
and `Error` arms locally. This is the primary error-handling idiom in
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
`LET`, MATCH on `Ok` / `Error`, and (inside the `Error` arm) dispatch
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
type parameters, `Ok :T` and `Error :E`. It is the shared return-type shape for
[`CATCH`](#catch) (`Result<T, KError>`) and for user functions with typed error
returns (`Result<T, MyErr>`). It is *not* a module-system functor: functors
produce modules, whereas `Result` is a type constructor producing a tagged-union
value.

It is registered once in the root scope by
[`result::register`](../src/builtins/result.rs), **type-only** the way a `UNION`
declaration is: `bindings.types["Result"]` holds a `TypeConstructor` identity
whose payload carries both the parameter names `T` / `E` and the variant
`schema` `{Ok, Error}` (both `Any`). `Result (Ok v)` / `Result (Error e)`
construct by reading that schema off a fresh `types["Result"]` lookup — the
same identity-borne path `UNION`-declared constructors use, with no value-side
carrier. Type-position application of `Result`'s two parameters is not yet
wired: the `AS` constructor-application form
([functors.md § Higher-kinded type slots](typing/functors.md#higher-kinded-type-slots))
is arity-1, and multi-parameter application is tracked in
[modular implicits](../roadmap/predicate_typing/modular-implicits.md).

The identity's `(name, scope_id)` fields use the root scope's `ScopeId` — the
scope that owns the registration, not `ScopeId::SENTINEL` — so every `Result`
value shares one nominal identity and MATCHes uniformly. Because the name is
registered at prelude, a user `UNION Result = (...)` is rejected with `Rebind`:
the binder-placeholder install refuses a name already bound to a non-function
value.

A `Result` value's type arguments are erased at construction — both `CATCH`
and a `Result (Ok v)` / `Result (Error e)` constructor leave the carrier's
`type_args` empty. A `:(Result T E)` slot is nonetheless runtime-checkable: the
`matches_value(ConstructorApply, Tagged)` arm (see
[ktype/parameterization-and-variance.md § Runtime type-parameter carriers](typing/ktype/parameterization-and-variance.md#runtime-type-parameter-carriers))
confirms the constructor identity and then checks the *inhabited* tag's payload
against the type argument that field maps to (`Ok`→`T`, `Error`→`E`). So a caught
`Result<_, KError>` is rejected where a `Result<_, MyErr>` is declared, because
the `Error` payload (a `KError`) does not satisfy `MyErr`. Ascription at an
annotated boundary stamps the carrier's `type_args` to the declared instantiation;
the remaining per-call parameter-slot binding for generic value-slot functions is
tracked under
[modular implicits](../roadmap/predicate_typing/modular-implicits.md).

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
TRY (<expr>) -> :<Type> WITH (
  Ok           -> <body>
  TypeMismatch -> <body>
  ...
  _            -> <body>   ; optional wildcard
)
```

Like `MATCH`, `TRY` declares a result type with `-> :<Type>` between the
expression and `WITH`; every arm body must produce that type. Arm heads are
capitalized variant tags (`Type` tokens) — `Ok` and the
capitalized `KErrorKind` names. Both slots are lazy `KExpression`s. `<expr>`
is evaluated in a catching sub-context: on success the `Ok` arm runs with `it`
bound to the bare success value; on failure the arm matching the `KErrorKind`
runs with `it` bound to a per-variant payload struct. No matching arm and no
`_` → re-raise the original `KError`. Success with no `Ok` arm and no `_` →
synthetic `ShapeError("TRY missing Ok arm")`.

The TRY body and each WITH arm are independent lexical blocks: any
`LET` introduced inside the body or an arm binds into that arm's own
scope and does not survive past the `TRY` (see the arm-as-block
treatment in [execution/calls-and-values.md § Lexical provenance chain](execution/calls-and-values.md#lexical-provenance-chain)).
This is the structural reason a `LET x` inside a TRY body is not a
`Rebind` of an enclosing `x`, and equally the reason a fresh `LET y`
inside the body is not visible to code following the `TRY`.

The branch walker is shared with `MATCH`
([`branch_walk::find_branch_body`](../src/builtins/branch_walk.rs));
TRY opts into `_` wildcard support, MATCH does not. The catching wiring is the
action-harness catch (`Action::Catch`, lowered to a `Continuation::Catch`; see
[execution/README.md](execution/README.md)): it waits on a watched slot and hands
its `Result<&KObject, KError>` to a host closure that decides whether to recover
or re-raise. Unlike a dep-finish, an errored dep does not short-circuit — TRY's
finish always runs (`catch_cont`).

### Exposed variants

User-meaningful subset. Each error arm's `it` is a Struct under one
shared `KError` tagged-union identity ([`KError::to_tagged`](../src/machine/core/kerror.rs))
with heterogeneous payload shape per arm; `Ok` binds `it` to the bare
success value (no wrapper). Tags are the capitalized `KErrorKind` names — a
`Type` token, since Type tokens cannot contain underscores:

| Tag | `it` shape |
|---|---|
| `Ok` | the success value (bare, not a struct) |
| `TypeMismatch` | `{arg :Str, expected :Str, got :Str, frames :List<Str>}` |
| `MissingArg` | `{name :Str, frames :List<Str>}` |
| `UnboundName` | `{name :Str, frames :List<Str>}` |
| `ArityMismatch` | `{expected :Number, got :Number, frames :List<Str>}` |
| `AmbiguousDispatch` | `{expr :Str, candidates :Number, frames :List<Str>}` |
| `DispatchFailed` | `{expr :Str, reason :Str, frames :List<Str>}` |
| `ShapeError` | `{message :Str, frames :List<Str>}` |
| `ParseError` | `{message :Str, frames :List<Str>}` |

`frames` is a `List<Str>`, each entry rendered `"in <expression> (<function>)"`.

The four dispatcher-internal kinds (`Rebind`, `DuplicateOverload`,
`TypeClassBindingExpectsType`, `SchedulerDeadlock`)
are only catchable via `_`; `it` is then bound to a minimal
`{kind :Str, message :Str, frames :List<Str>}` struct.

## `CATCH`

`CATCH <expr>` lifts a single interpreter fault into a [`Result`](#result) value
rather than letting it propagate. It is the opt-in, expression-position
counterpart to [`TRY-WITH`](#try-with): where `TRY-WITH` forces the caller to
spell out catch arms at the catch site, `CATCH` hands back a `Result<T, KError>`
the caller binds with `LET`, passes as an argument, or returns:

- `Ok(v)` on success, where `v` is the bare success value;
- `Error(e)` on failure, where `e` is
  [`KError::to_tagged`](../src/machine/core/kerror.rs)'s value — still carrying
  the per-`KErrorKind` tag and payload struct, so per-kind dispatch is reached by
  MATCH-ing `e` after destructuring the `Result`.

The [`CATCH`](../src/builtins/catch.rs) builtin reuses the same scheduler
mechanism as `TRY-WITH` (`Action::Catch` / `CatchFinish`): it schedules `<expr>` as a
catching sub-dispatch and registers a finish closure that wraps the outcome in
a `Result` value. The prelude `Result` identity's `scope_id` is read from
`bindings.types` (via `scope.resolve_type("Result")`) at body time, not from the
call-site scope, so a `CATCH`-produced `Result` and a `Result (...)`-constructed
one share nominal identity regardless of where the `CATCH` runs. `LET` and other eager slots still short-circuit on errors, so the
lift stays opt-in.

## Open work

- **`Result` combinators** — `map`, `bind`, `unwrap_or`, etc.; Koan source
  over the builtin `Result` type, tracked under the
  [standard library](../roadmap/libraries/standard-library.md). The `Result`
  constructor itself is builtin (above), so user code can use it before these
  helpers ship.
- **Continue-on-error** — top-level continuation past a single failed
  expression, useful for the CLI's batch mode, tracked under
  [continue-on-error for the REPL and batch mode](../roadmap/editor_tooling/continue-on-error.md).
