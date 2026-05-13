# Support for functional programming

Functions are first-class values in Koan. `KFunction` is a `KObject` variant
([kobject.rs](../src/runtime/model/values/kobject.rs)), so a function can be returned from a
body, bound via `LET`, looked up by name, and invoked via
[`call_by_name`](../src/runtime/builtins/call_by_name.rs) or by appearing in a
position the dispatcher resolves.

## User-defined functions

The surface form is:

```
FN (<signature>) -> ReturnType = (<body>)
```

The signature is itself a [`KExpression`](../src/ast.rs) mixing
fixed `Keyword` tokens and `name: Type` parameter triples. The triple form is
required — a bare identifier without `: Type` is a parse error; use `: Any` to
opt out of type-checking for a slot. Keyword tokens are part of the dispatch
key. The body is a `KExpression` evaluated at call time.

Example:

```
FN (ECHO x: Number) -> Number = (x)
LET y = (ECHO 21)
```

## Body representation

```rust
Body { Builtin(BuiltinFn) | UserDefined(KExpression) }
```

(in [kfunction.rs](../src/runtime/machine/kfunction.rs)). The `UserDefined(KExpression)`
shape was chosen over `Box<dyn Fn>` so that the TCO and error-frame paths can
introspect the body — TCO needs to recognize the tail position; error frames
need to know which function the trace step belongs to. A boxed closure would
have hidden both.

## Calling convention: parameter substitution

`KFunction::invoke` clones the body, rewrites parameter `Identifier`s to
`Future(call-site value)`, and returns it as `BodyResult::Tail` for the
scheduler to dispatch in the same slot. Two consequences:

- User-fns inherit TCO automatically — every call rewrites the slot in place.
  No special TCO handling for user-fn vs builtin tail returns.
- Free names (anything not a parameter) resolve through the function's
  `captured` scope — lexical, not dynamic. See
  [memory-model.md](memory-model.md) for the scoping mechanics.

## Closures

Per-call arenas back the substituted body, the child scope, parameter clones,
and any in-body `LET`/`value_pass` allocations. When a closure escapes (e.g., a
fn defined inside a body and returned as the body's value), `Rc<CallArena>`
keeps the captured arena alive for as long as the closure is reachable. The
mechanics — `lift_kobject`, the `Option<Rc<CallArena>>` carried by
`KObject::KFunction`, the fast path when no functions were allocated — live in
[memory-model.md](memory-model.md).

End-to-end verification:

- [`closure_escapes_outer_call_and_remains_invocable`](../src/runtime/builtins/call_by_name.rs)
  — return a closure from a body, call it after the outer frame has finalized.
- [`escaped_closure_with_param_returns_body_value`](../src/runtime/builtins/call_by_name.rs)
  — escaped closure with a parameter still substitutes correctly.

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
system](module-system.md). Modular implicits
([stage 5](../roadmap/module-system-5-modular-implicits.md)) add a second
kind of dispatch alongside slot-specificity: a function declares an implicit
module parameter, and the compiler infers and inserts a satisfying module at
each call site. `sort {Mo : ORDERED} (xs: List<Mo.t>)` is an ordinary `FN`
in the value language whose `Mo` is resolved by lexical implicit search rather
than by a runtime argument. Functors
([stage 2](../roadmap/module-system-2-scheduler.md)) give the *module*
language the analog of the higher-order story this doc covers — a module
parameterized by another module, applied generatively to produce fresh
abstract types. See [module-system.md](module-system.md) for the full
plan; container type parameterization (`List<Number>`,
`Function<(args) -> R>`, etc.) is shipped today and is documented in
[type-system.md](type-system.md).
