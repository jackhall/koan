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

- *Carrier site — open.* Where per-instance type arguments get stored:
  candidates include extending `KObject::List` / `KObject::Dict` with an
  element-`KType` field, and giving `ConstructorApply` a runtime carrier
  (a `KObject` variant or an identity field on the produced `Tagged`).
  The two may share a mechanism or be solved separately.
- *Eager vs. deferred element checking — open.* Containers are shape-only
  at dispatch today because lazy elements aren't evaluated yet; a runtime
  carrier could record the *declared* element type at construction and
  check membership post-evaluation, or force eager element typing. The
  decision interacts with how list/dict literals are built.
- *Variance — open.* Whether `List<Number>` is admissible where
  `List<Any>` is declared (and the analogous question for each
  constructor parameter) needs a variance rule per parameter position.

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
