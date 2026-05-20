# Runtime carriers for type parameters

**Problem.** Parameterized types erase their type arguments at runtime, so
dispatch and slot admission can't see them. `List` / `Dict` values carry
no element type — [`matches_value`](../../src/machine/model/types/ktype_predicates.rs)
treats container slots as shape-only at dispatch (element-type validation
is deferred to a post-evaluation pass, because a lazy list at dispatch time
may still hold unevaluated `Expression` parts). `ConstructorApply` (the
elaborated form of a user-applied constructor like `Result<T, E>`) has no
runtime carrier at all: `matches_value` returns `false` for it outright,
since no `KObject` synthesizes a `ConstructorApply` `ktype()`. The upshot
is that `List<Number>` and `List<Str>` are indistinguishable as values, and
so are `Result<T, KError>` and `Result<T, MyErr>` — a slot typed
`:(Result Number MyErr)` does not validate that a value's error parameter
is actually `MyErr`.

**Impact.**

- *Dispatch on type parameters.* Overloads distinguished by a container's
  element type or a constructor's type argument (`List<Number>` vs
  `List<Str>`, `Result<_, KError>` vs `Result<_, MyErr>`) resolve to the
  right candidate.
- *Full parameterized-type slot admission.* A slot typed `:(Result Number
  MyErr)` or `:List<Number>` validates the complete instantiation at the
  dispatch boundary, not just the outer shape.
- *Error-type discipline for `Result`.* The builtin `Result` parameterized
  type (see [error-handling](../libraries/error-handling.md)) gains
  enforcement that a caught `Result<T, KError>` is not silently accepted
  where a `Result<T, MyErr>` is declared.

**Directions.**

- *Carrier site — decided.* Every parameterized value carries its type
  arguments on the variant, and `ktype()` reads them rather than recomputing.
  `KObject::List` / `KObject::Dict` gain a memoized element-type field (element
  type for `List`; key + value for `Dict`), computed once at construction —
  values are immutable `Rc`, so the join is computed exactly once. `KObject::Tagged`
  gains a `type_args` field: empty means erased (today's behavior), and when
  populated `ktype()` synthesizes `ConstructorApply { ctor, args: type_args }`
  instead of the bare `UserType`.
- *Type-parameter representation — decided.* No `KType::TypeParam` variant.
  Type parameters stay what they already are — ordinary names resolved through
  scope, with per-call deferred elaboration (the existing FN return-type
  `Deferred` path; see
  [elaboration.md](../../design/typing/elaboration.md) and
  [functors.md](../../design/typing/functors.md)). A `:(List T)` or
  `:(Result T E)` slot binds `T` / `E` per-call by unifying the slot's
  parameterized type against the value's carried type arguments. The `Result`
  field→parameter linkage (`ok`→`T`, `error`→`E`) is registration metadata on
  the builtin, not a `KType` variant.
- *Ascription is authoritative at annotated boundaries — decided.* The carrier
  is populated by ascription, mirroring `Wrapped.type_id` and module
  `compatible_sigs`. At an annotated boundary (FN return type, argument slot,
  `LET` ascription) the declared type is the contract: (1) check the value
  satisfies it via `matches_value` (covariant, content-recursive — already
  implemented), then (2) re-tag the value to *exactly* the declared type,
  **coarsening included** — a `List<Number>` value returned where `:(List Any)`
  is declared is re-tagged `List<Any>`, so downstream dispatch sees the
  contract, not the implementation's incidental precision. Unannotated, a value
  keeps its precise memoized type (the join, for containers); surrendering
  precision is the deliberate act of writing an annotation.
- *Empty containers require type information — decided.* An empty `[]` / `{}`
  has no join to infer from. In an annotated position the (vacuous) check passes
  and the declared element type is stamped. With no annotation anywhere
  upstream, an empty container that reaches an untyped resolution boundary (an
  untyped `LET` binding, a bare expression result) is an **error**, not a silent
  `List<Any>` / `Dict<Any, Any>`. A *heterogeneous non-empty* literal
  (`[2, "hello"]`) is unaffected: it carries information (`List<Any>`), so it is
  legal where `:(List Any)` is declared and fails — correctly — where
  `:(List Number)` is.
- *Variance — decided.* Covariant in every parameter position, consistent with
  the existing `is_more_specific_than` ranking
  ([ktype.md § Variance](../../design/typing/ktype.md#variance)).

## Dependencies

**Requires:**

None — the type-language elaboration of parameterized types
(`KType::ConstructorApply`, `KType::List`, `KType::Dict`) is shipped; this
item plugs runtime carriers into the value side so the elaborated types
have something to match against.

**Unblocks:**

None as a hard prerequisite. Beneficiaries that gain precision once
carriers land: the builtin `Result` parameterized type
([error-handling](../libraries/error-handling.md)), `List` / `Dict`
element-type dispatch, and any later implicit-search dispatch that keys on
a parameterized type — but each of those ships and functions under the
current erasure first.
