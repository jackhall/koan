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
- `DuplicateDeclaration` — two statements of one block declare the same name.
  Ruled on where the block's claim store is built, with both declaring statements
  in hand and neither yet run, so it names both positions instead of landing on
  whichever body committed second (see
  [execution/name-placeholders.md](execution/name-placeholders.md#a-claim-lives-in-the-scopes-claim-store)).
- `DuplicateOverload` — an `FN` indistinguishable from a registered overload: same
  element shape, same type in every argument slot.
- `SchedulerDeadlock` — the scheduler reached a fixed point with work still outstanding.
- `User` — landing pad for user-side error construction; see open work.

## Propagation

The scheduler walks errors along the dependency edges: a slot's terminal
`Err` write triggers the notify-walk, which wakes each waiting consumer; a dep-finish
short-circuits, appends a `TraceFrame`, and writes the error into its own slot (a
catch instead recovers or re-raises). Errors flow to the top level; the CLI
formats them to stderr with the frame chain via `KError`'s `Display` impl.

A frame is *captured*, not rendered, at the point the work that might fail is set up.
A `DeferredTraceFrame`
([src/machine/execute/outcome.rs](../src/machine/execute/outcome.rs)) is `Copy` and has
one arm per capture shape: a park's own frame carries a fixed label plus the working
expression the slot dispatches; a frameless dep-finish carries two `&'static str`s; and
a call's declared-return contract carries the call site's span-and-file pair plus the
callable's interned `value_ktype`. Each becomes `TraceFrame` text only on an error arm —
`propagate_dep_error`
([src/machine/execute/decide.rs](../src/machine/execute/decide.rs)) for a dep error, and
the declared-return readers in
[src/machine/execute/finalize.rs](../src/machine/execute/finalize.rs) for a contract. A
step that finishes without an error therefore allocates no trace text at all, and a call
that returns cleanly renders no signature text.

The two capture shapes are safe for different reasons. A frame-carrying park already holds
its expression in its continuation, sealed against the slot's anchor, so the expression's
region is live wherever the render runs. A contract's capture names no region at all: a
span, a `FileId` into the run's source table, and a type-registry handle are all
lifetime-free, which is what lets the sealed
[`ReturnObligation`](../src/machine/execute/obligation.rs) ride a tail chain as pure
`Copy` data (see
[tail-call-optimization.md § The kept-first return contract](tail-call-optimization.md#the-kept-first-return-contract)).

A rendered call frame names the call two ways: `function` carries the call site's own
source text — `BOOM 1` for a keyword call, `f {x = 7}` for a bound-value one — and
`expression` carries the callable's by-name identity, its `value_ktype` rendered through
`KType::name` as `:(FN (x :Number) -> Str)`, with the frame's location resolved from the
same span. A contract sealed from an expression with no source extent falls back to the
by-name render alone. The by-name identity is the right render for a function *value*,
which the anonymous `FN` form makes reachable only through the name a `LET` binds — that
is call-site knowledge, and the call site is what `function` supplies.

Dispatch failures (no match, ambiguous overload, arity mismatch in bind) flow
through the same channel as builtin errors:
[`Scope::resolve_dispatch`](../src/machine/execute/decide/resolve_dispatch.rs) returns a
`DispatchOutcome` whose `Ambiguous` and `Unmatched` arms the scheduler driver
converts to `Err(KError)` with `KErrorKind::AmbiguousDispatch` /
`DispatchFailed`, and `KFunction::bind_args_into` returns `Err(KError)` on arity
mismatch rather than filling the call's argument slots.
[`KoanRuntime::execute`](../src/machine/execute/harness.rs) and
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
function in the trace, because the slot's work is reinstalled at TCO
time (see [execution/README.md](execution/README.md) and
[tail-call-optimization.md](tail-call-optimization.md)).
Non-tail-call positions — e.g., a sub-`Dispatch` inside a parens-wrapped
sub-expression — preserve the outer frame: the consuming slot parks on the
sub-`Dispatch` as a dependency, and the dep-finish short-circuit retains the call
frame and `function` label, so an error landing on the dependency carries the
outer function's frame. This matches how other languages with TCO behave.

## User-side surface

A user-written function that can fail returns
`:(Result {Ok = <Ty>, Error = <Er>})` for a
user-defined error type `Er` — `Result` is a builtin parameterized type
(like `List` / `Dict`) with `Ok` and `Error` variants, each typed by the
same-named type parameter (see [`Result`](#result)).
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
single fault into a `Result` at `Error = KError` so the caller can bind it via
`LET`, MATCH on `Ok` / `Error`, and (inside the `Error` arm) dispatch
on the per-kind member identity the payload carries (see [`CATCH`](#catch)).
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

`Result` is a builtin two-member union whose members, `Ok` and `Error`, name the
type arguments an application supplies. It is the shared return-type shape for
[`CATCH`](#catch) (`:(Result {Ok = <Ty>, Error = KError})`) and for user functions
with typed error returns (`:(Result {Ok = <Ty>, Error = MyErr})`). It is *not* a
functor — a module-returning function — whereas `Result` is a union head whose
projections produce wrapped member values.

It is registered once in the root scope by
[`result::register`](../src/builtins/result.rs), **type-only** the way a `UNION`
declaration is: a sealed two-member group — `Ok` and `Error`, each over `Any` —
with `bindings.types["Result"]` bound to the members' union, and each parameter
named for the member whose payload it types, so the two agree by construction.
`Result.Ok v` / `Result.Error e` construct by member projection — the same door a
`UNION`-declared variant uses, with no value-side carrier. Applying the union head
itself (`Result (Ok 1)`) constructs nothing: it raises the member-projection guidance
every union head gives. Type-position application binds the arguments by member name —
`:(Result {Ok = Number, Error = MyError})` — and lowers per member, the rule general to
any union head (see
[ktype/parameterization-and-variance.md § Applying a union head](typing/ktype/parameterization-and-variance.md#applying-a-union-head)).
The arity-1 `AS` form does not apply to `Result`; it errors directing to the
record form.

The members are minted once at prelude registration and their union bound in the
root scope's `types`, so every `Result` value carries one of the two member
identities and MATCHes uniformly. Because the name is
registered at prelude, a user `UNION Result = (...)` is rejected with `Rebind`:
the binder-placeholder install refuses a name already bound to a non-function
value.

A `Result` value's type arguments are erased at construction — both `CATCH`
and a `Result.Ok v` / `Result.Error e` construction leave the carrier's
`type_id` a bare `SetMember` handle. A `:(Result {Ok = …, Error = …})` slot is nonetheless
runtime-checkable: the slot is the union of per-member applications, and the
constructor-application `matches_value` arm (see
[ktype/parameterization-and-variance.md § Applying a union head](typing/ktype/parameterization-and-variance.md#applying-a-union-head))
confirms the member identity and then checks the *inhabited* member's payload
against the same-named type argument — a direct lookup, since member name and
argument name coincide. So a caught
`Result` at `Error = KError` is rejected where `Error = MyErr` is declared, because
the `Error` payload (a `KError`) does not satisfy `MyErr`. Ascription at an
annotated boundary stamps the carrier's `type_id` to the `ConstructorApply` over the
member it inhabits;
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
expression and `WITH`; every arm body must produce that type. Arm heads name
members of a fixed slate — `Result`'s `Ok` plus every member of the prelude
[`KError`](#the-kerror-union) union — or are `_`, the default arm. A head that is
neither (a boolean literal, an undeclared name) is an arm-set error at the form,
checked before the outcome is known. Both slots are lazy `KExpression`s. `<expr>`
is evaluated in a catching sub-context: on success the `Ok` arm runs with `it`
bound to the bare success value; on failure the arm naming the lowered error's own
kind member runs with `it` bound to that kind's payload record. No matching arm and
no `_` → re-raise the original `KError`. Success with no `Ok` arm and no `_` →
synthetic `ShapeError("TRY missing Ok arm")`.

A `TRY` arm set carries **no coverage requirement**: an unhandled kind re-raises
rather than failing the form, which is what makes leaving kinds out the ordinary
spelling and `_` optional.

The TRY body and each WITH arm are independent lexical blocks: any
`LET` introduced inside the body or an arm binds into that arm's own
scope and does not survive past the `TRY` (see the arm-as-block
treatment in [execution/calls-and-values.md § Lexical provenance chain](execution/calls-and-values.md#lexical-provenance-chain)).
This is the structural reason a `LET x` inside a TRY body is not a
`Rebind` of an enclosing `x`, and equally the reason a fresh `LET y`
inside the body is not visible to code following the `TRY`.

The branch walker is `MATCH`'s member walk over a fixed member set — an implicit
`OVER` covering `Ok` plus the error members
([branch_walk.rs](../src/builtins/branch_walk.rs)). `TRY` and `MATCH … OVER` share
one arm parser and one `_` default-arm rule; they differ only in coverage (above)
and in how the winner is chosen — `TRY` keys on the member the outcome already
carries, `MATCH … OVER` runs the specificity tournament. The catching wiring is the
action-harness catch (`Action::Catch`, lowered to a `Continuation::Catch`; see
[execution/README.md](execution/README.md)): it waits on a watched slot and hands
its `Result<&KObject, KError>` to a host closure that decides whether to recover
or re-raise. Unlike a dep-finish, an errored dep does not short-circuit — TRY's
finish always runs (the `catching` adapter).

### The `KError` union

Every catchable kind is a member of `KError`, a prelude union registered beside
`Result` ([error_union.rs](../src/builtins/error_union.rs)) with one `NewType`
member per `KErrorKind` surface name. Registration and lowering read the member
names off one table in [kerror.rs](../src/machine/core/kerror.rs), so a kind's
member and the name its lowering looks up cannot drift apart.

A lowered error is therefore an ordinary union value: one `KObject::Wrapped`
carrying its kind's member handle over the kind's field record. One identity spells
the kind, so a caught error renders its name exactly once —
`PRINT (CATCH (mystery))` reads
`Error(UnboundName({frames = [], name = mystery}))`. Because `KError` is a real
registered type, `MATCH e OVER KError WITH (…)` eliminates a caught error through
the same member walk any union takes, and `CATCH`'s declared
`:(Result {Ok = Any, Error = KError})` return names a type that resolves rather than
a documentary one.

An error arm — in `TRY` or in `MATCH … OVER KError` — binds `it` to the kind's
payload **record** (ruling F3, the same narrowing a union variant's arm takes).
That record is an ordinary anonymous record value, so an arm reads a single field
straight off it: `UnboundName -> (PRINT it.name)`, `ShapeError -> (PRINT
it.message)`, and `it.frames` on any kind, through the same bare-record projection
[`wrapped_field_cell`](../src/builtins/attr.rs) serves every record with. The
per-kind field names are the table below.

### Exposed variants

User-meaningful subset, with the payload record each kind's arm binds; `Ok` binds
`it` to the bare success value (no wrapper). Member names are the capitalized
`KErrorKind` names — a `Type` token, since Type tokens cannot contain underscores:

| Member | `it` shape |
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

The five dispatcher-internal kinds (`Rebind`, `DuplicateDeclaration`,
`DuplicateOverload`, `TypeClassBindingExpectsType`, `SchedulerDeadlock`) are
ordinary members like any other: an arm names one directly, and `_` reaches them as
members no named arm claims rather than through a side channel. Their payload is a
flattened `{kind :Str, message :Str, frames :List<Str>}` record.

## `CATCH`

`CATCH <expr>` lifts a single interpreter fault into a [`Result`](#result) value
rather than letting it propagate. It is the opt-in, expression-position
counterpart to [`TRY-WITH`](#try-with): where `TRY-WITH` forces the caller to
spell out catch arms at the catch site, `CATCH` hands back a `Result` at
`Error = KError` — its declared return type is
`:(Result {Ok = Any, Error = KError})` — which
the caller binds with `LET`, passes as an argument, or returns:

- `Ok(v)` on success, where `v` is the bare success value;
- `Error(e)` on failure, where `e` is the lowered error value
  ([kerror.rs](../src/machine/core/kerror.rs)) — a `KError` member value carrying
  its kind's identity over the kind's payload record, so per-kind dispatch is
  reached by `MATCH e OVER KError` after destructuring the `Result`.

The [`CATCH`](../src/builtins/catch.rs) builtin reuses the same scheduler
mechanism as `TRY-WITH` (`Action::Catch` / the `catching` adapter): it schedules `<expr>` as a
catching sub-dispatch and registers a finish closure that wraps the outcome in
a `Result` value under the projected `Ok` / `Error` member. The prelude `Result`
union is read from `bindings.types` (via `scope.resolve_type("Result")`) at body
time and its members projected there, so a `CATCH`-produced value and a
`Result.Ok` / `Result.Error`-constructed one share nominal identity regardless of
where the `CATCH` runs. The wrap **holds** rather than peels, so an error's kind
layer nests inside the `Error` layer and each name renders once. `LET` and other eager slots still short-circuit on errors, so the
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
