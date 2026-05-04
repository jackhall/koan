# Type system

## Token classes — the parser-level foundation

The lexer ([tokens.rs](../src/parse/tokens.rs)) splits non-literal atoms into
three classes by capitalization:

- **All-caps** (`LET`, `THEN`, `=`, `->`) — dispatch keywords. Contribute fixed
  tokens to a signature's bucket key.
- **Capitalized + at least one lowercase** (`Number`, `Str`, `KFunction`,
  `MyType`) — type references.
- **Lowercase / snake_case** — identifiers.

This split is what lets the language reserve a syntactic slot for type names
without quoting. `FN (x: Number) -> Str = (...)` works because `Number` and
`Str` are recognizable as types from their shape alone.

## `KType` — the runtime type system

[`KType`](../src/dispatch/kfunction.rs) has a variant for every concrete
`KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List`, `Dict`.
- Function-like: `KFunction`, `KExpression`.
- `TypeRef` — a reference to a named type.
- `Any` — the no-op fast-path.

[`KType::matches_value`](../src/dispatch/kfunction.rs) plus
[`KObject::ktype`](../src/dispatch/kobject.rs) close the loop on runtime
checking: every value has a queryable type, and any declared type can be checked
against it.

## Function signatures

`FN` syntax requires both per-parameter types and a return type:

```
FN (sig) -> ReturnType = (body)
```

Each parameter slot in `<sig>` is written as `name: Type`. A bare identifier
without `: Type` is a parse error — there is no implicit `Any` default. Use
`: Any` to opt a slot out of type-checking. Parameter types are checked at
dispatch via the same `Argument::matches` path as builtins, so a call whose
arguments don't satisfy the signature surfaces as
[`KErrorKind::DispatchFailed`](../src/dispatch/kerror.rs); the same call shape
with different parameter types routes to a different overload by
slot-specificity (see below).

The return type is non-optional and runtime-enforced. The scheduler injects a
check at user-fn slot finalization that surfaces
[`KErrorKind::TypeMismatch`](../src/dispatch/kerror.rs) (with a `<return>` arg
name and a frame naming the called function) on mismatch. `Any` is the
no-enforcement fast path for sites that genuinely don't care.

This was the "make function shapes honest" choice. Builtin signatures got
audited at the same time: `LET` was fixed from `Null` to `Any`, FN-registration
from `Null` to `KFunction`.

## Dispatch and slot-specificity

When multiple registered functions match an incoming expression, dispatch picks
by slot-specificity: typed slots outrank untyped ones; literal-typed slots
outrank `Any`. See [expressions-and-parsing.md](expressions-and-parsing.md) for
how the parser splits an expression into the `Keyword`/slot positions that
specificity scores against.

## Known limitations

- **TCO collapses frames.** When A tail-calls B, only B's return type is
  checked at runtime — the slot's `function` field is replaced at TCO time. The
  future static pass will close this gap.
- **Builtins are not runtime-checked.** They return through `BodyResult::Value`
  with no slot frame, so the runtime check has nowhere to attach. Their
  declared return types are honest but unenforced; the static pass will check
  them uniformly.
- **No container parameterization.** `List`, `Dict`, `KFunction`, `Future`
  carry no inner-type information today.

## Open work

The type/trait sequence is the longest open arc in the language. In dependency
order:

- [Container type parameterization](../roadmap/container-type-parameterization.md)
  — `List<Number>`, `Dict<Str, Any>`, etc.
- [Per-type identity for structs and methods](../roadmap/per-type-identity.md)
  — every user struct currently collapses to `KType::Struct`; methods can't
  attach to specific types.
- [`TRAIT` builtin for structural typing](../roadmap/traits.md) — "anything
  iterable", "anything orderable".
- [Trait inheritance](../roadmap/trait-inheritance.md) — `Ord` extending `Eq`
  is the standard layering.
- [Group-based operators](../roadmap/group-based-operators.md) — `+`/`-` form
  a math group but the language treats every operator as flat-independent.

The type/trait sequence sits in the middle of the roadmap because it unblocks
group-based operators.

Future-facing:
[Static type checking and JIT compilation](../roadmap/static-typing-and-jit.md)
— closes the TCO and builtin runtime-check gaps uniformly, and is the language's
performance ceiling.
