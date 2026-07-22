# Support for functional programming

Functions are first-class values in Koan. `KFunction` is a `KObject` variant
([kobject.rs](../src/machine/model/values/kobject.rs)), so a function can be returned from a
body, bound via `LET`, looked up by name, and invoked through the
`FunctionValueCall` handler
([dispatch/fn_value.rs](../src/machine/execute/dispatch/fn_value.rs) —
`initial`) or by appearing in a position the dispatcher
resolves.

## User-defined functions

The surface form is:

```
FN (<signature>) -> ReturnType = (<body>)
```

The signature is itself a [`KExpression`](../src/machine/model/ast.rs) mixing
fixed `Keyword` tokens and `name: Type` parameter triples. The triple form is
required — a bare identifier without `: Type` is a parse error; use `: Any` to
opt out of type-checking for a slot. Keyword tokens are part of the dispatch
key. The body is a `KExpression` evaluated at call time.

Example:

```
FN (ECHO x :Number) -> Number = (x)
LET y = (ECHO 21)
```

## Anonymous functions

A keyword-less function literal uses a record-schema binder in place of the
parenthesized signature:

```
FN :{<field schema>} -> ReturnType = (<body>)
```

The `:{…}` is the record-type-schema sigil; each `name :Type` field becomes a
parameter. With no `Keyword` in the signature there is nothing to dispatch on, so
the function registers no form — its only handle is the value it evaluates to,
bound by `LET` or passed straight into a function-typed slot. It is invoked
through the function-value call path with a record argument, never positionally:

```
LET inc = (FN :{x :Number} -> Number = (x))
LET n = (inc {x = 41})
```

A **named** `FN` — the keyworded form above — is a binder, so it may stand only
at a statement position or in another binder's declaration slot, never inline in
an eagerly evaluated value position such as a call argument or a list / dict
element (that is a `NestedBinder` error). The two value routes are the anonymous
`FN :{…}` form, which installs nothing, or binding the named form through a `LET`
chain and passing the name:

```
LET g = (FN (SHOW x :Number) -> Str = ("hi"))
LET greeting = (USE g)
```

The [position rule](execution/name-placeholders.md#submission-time-binder-install-and-the-position-rule)
gives the full set of legal binder positions.

Dispatch tells the two forms apart by the signature operand's part kind: a
parenthesized `(…)` signature is a `KExpression`, while a `:{…}` schema is a
first-class `RecordType` part that sub-dispatches to a resolved
[`KType::Record`](../src/machine/model/types/ktype.rs) before the binder runs.
Three `FN` overloads share one bucket ([fn_def.rs](../src/builtins/fn_def.rs)) —
two keyworded ones split on the return-type carrier, and one whose signature slot
admits the resolved record schema. The record's
fields become keyword-less `Argument`s, and everything downstream —
`reconstruct_positional`, lexical closure capture, and contravariant function
subtyping — is shared with the keyworded form, so an anonymous function projects
the same `KType::KFunction` and fills the same function-typed parameter slots.

## Body representation

```rust
Body { Builtin(BuiltinFn) | UserDefined(KExpression) }
```

(in [kfunction.rs](../src/machine/core/kfunction.rs)). The `UserDefined(KExpression)`
shape was chosen over `Box<dyn Fn>` so that the TCO and error-frame paths can
introspect the body — TCO needs to recognize the tail position; error frames
need to know which function the trace step belongs to. A boxed closure would
have hidden both.

## Calling convention: per-call scope

The user-fn body executor (`run_user_fn`, lowered onto the scheduler by
`dispatch::exec::invoke`) allocates a per-call [`CallFrame`](../src/machine/core/arena.rs),
binds each parameter into a fresh child `Scope` whose `outer` is the function's
captured definition scope, and returns the body unmodified as
`Action::Tail` (lowered to `Outcome::Continue`) for the scheduler to dispatch in the same slot.
Two consequences:

- User-fns inherit TCO automatically — every call rewrites the slot in place.
  No special TCO handling for user-fn vs builtin tail returns.
- Free names (anything not a parameter) resolve through the function's
  `captured` scope — lexical, not dynamic. See
  [memory-model.md](memory-model.md) for the scoping mechanics.

## Closures

Per-call regions back the child scope, parameter clones, and any in-body
`LET`/`value_pass` allocations. When a closure escapes (e.g., a fn defined
inside a body and returned as the body's value), `Rc<CallFrame>` keeps the
captured region alive for as long as the closure is reachable. The mechanics —
`lift_kobject`, the `Option<Rc<FrameStorage>>` carried by `KObject::KFunction`,
the fast path when no functions were allocated — live in
[memory-model.md](memory-model.md).

End-to-end verification:

- [`fast_lane_closure_escapes_outer_call_and_remains_invocable`](../src/machine/execute/run_loop/tests/dispatch_shapes.rs)
  — return a closure from a body, call it after the outer frame has finalized.
- [`fast_lane_escaped_closure_with_param_returns_body_value`](../src/machine/execute/run_loop/tests/dispatch_shapes.rs)
  — escaped closure with a parameter resolves the captured binding correctly.

## Composition with the language extension story

Because signatures are themselves `KExpression`s, a user-defined `FN` introduces
a new dispatchable shape that participates in the same scoring as builtins. A
function isn't just a callable value; the dispatch table is the language's
extension mechanism. See [expressions-and-parsing.md](expressions-and-parsing.md)
for how this lets users add what look like new keyword forms.

## Non-goals

- **Variadic signatures.** Functions take a fixed argument set determined by
  their signature. Variadic argument support won't ship — the comparator's
  tiebreak rule for variadic-vs-fixed overloads has no clean answer, and the
  surface use cases are covered by passing a list as one argument.

## Open work

The generic-function story extends through the [module
system](typing/modules.md). Modular implicits
([stage 5](../roadmap/predicate_typing/modular-implicits.md)) add a second
kind of dispatch alongside slot-specificity: a function declares an implicit
module parameter, and the compiler infers and inserts a satisfying module at
each call site. `sort {mo : ORDERED} (xs :(LIST OF mo.t))` is an ordinary `FN`
in the value language whose `mo` is resolved by lexical implicit search rather
than by a runtime argument. Functors
([typing/functors.md](typing/functors.md)) give the *module*
language the analog of the higher-order story this doc covers — a module
parameterized by another module, applied generatively to produce fresh
abstract types. See [typing/](typing/README.md) for the full
plan; container type parameterization (`:(LIST OF Number)`,
`:(FN (args) -> R)`, etc.) is shipped today and is documented in
[typing/ktype/README.md](typing/ktype/README.md).
