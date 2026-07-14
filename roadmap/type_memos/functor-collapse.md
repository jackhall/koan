# Functor collapse

`FN` is koan's only function binder; a module-returning function is an ordinary value-side
function with no type-language machinery behind it.

**Problem.** `FUNCTOR` is a second function binder whose only definition-time job is rejecting a
return slot that is not a module — `-> Number`, `-> :(FN …)`, and a dotted abstract-type return all
reject ([`return_validation.rs`](../../src/builtins/functor_def/tests/return_validation.rs)).
[functors.md](../../design/typing/functors.md) states the rest plainly: the dispatch path,
scheduler integration, per-call scope, and body executor are the same as a plain FN, and FUNCTOR is
"a thin definition-time façade over FN mechanics". The `is_functor` flag
([`kfunction.rs`](../../src/machine/core/kfunction.rs)) drives exactly two effects — that return
validation, and a distinct `KType::KFunctor { params, ret, body }` reported by `ktype()`.

The façade is load-bearing on a premise that no longer holds. A functor binds into
`bindings.types` under a Type-token name — binding one to a lowercase name is an error at the LET
site ([`let_binding.rs`](../../src/builtins/let_binding.rs)) — and head application resolves it
through a Type-head `TypeCall` against that type-table entry
([`apply_callable.rs`](../../src/machine/execute/dispatch/apply_callable.rs)). Both exist only
because a module was a type: the functor's *result* had to live in the type language for a `Type`
head to name it. With modules on the value channel a functor consumes values (modules, and types,
which ride `Carried::Type`) and produces a value, yet `KType::KFunctor`, the type-side home, the
`TypeCall` application arm, and the `:(FUNCTOR (params) -> R)` type sigil all persist.

Nothing else needs the variant. Generic functions are functors over `:Type` parameters returning a
**module** that holds the specialized FN ([generics.md](../../design/typing/generics.md)), not a
type. Real type constructors are [user-defined type
constructors](user-defined-type-constructors.md), and `KType::KFunctor` is not doing that duty —
the only non-sentinel `KKind::TypeConstructor` identities are the builtin `Result` and opaque
ascription's per-call mints.

**Acceptance criteria.**

- `FN` is koan's only function binder: `FUNCTOR` is not a keyword, and `src/builtins/functor_def.rs`
  does not exist.
- A module-returning function binds value-side in `bindings.data` and is applied by the ordinary
  call convention; `bindings.types` holds no callable value.
- `KType::KFunctor` and `KFunctionValue::is_functor` are deleted; a module-returning function's
  `ktype()` is `KType::KFunction`.
- `:(FN (params) -> R)` is the only function-type surface; the `:(FUNCTOR …)` sigil is unbound.
- A `:Type` parameter binds a type value through the ordinary value channel, so generic functions
  ([generics.md](../../design/typing/generics.md)) and modular implicits
  ([implicits.md](../../design/typing/implicits.md)) are expressed over `FN` with no
  functor-specific machinery.
- Generativity is unchanged: two applications of a module-returning FN whose body contains `:|`
  mint distinct abstract types.
- The functor test trees (`src/builtins/functor_def/tests/`, `src/builtins/fn_def/tests/functor/`)
  are rehomed under FN, or deleted where they exercised functor-only machinery.

**Directions.**

- *Applicative-mode seam — decided.* [functors.md](../../design/typing/functors.md) keeps FUNCTOR
  partly as the declaration-site seam for future applicative semantics, on the reasoning that
  routing applicative mode through FUNCTOR "keeps the generative/applicative choice visible at the
  declaration". No marker is needed to keep it visible: a module cannot be named in type position,
  so a module-returning function's return slot names a signature, and "the return type is a
  signature" is exactly the property `is_functor` stands in for. Applicative opt-in, when the
  predicate-typing substrate lands, keys on that derived classification.
- *Type-valued parameters — decided.* A `:Type` parameter survives the collapse: type values ride
  the `Carried::Type` arm through the ordinary value channel, so an FN admits one with no functor
  machinery.
- *Return validation versus classification — decided.* Validation drops: an FN may return anything,
  and the ordinary per-call return contract covers a module return. Classification survives as a
  *derived* property — "does this return slot name a signature?" — computed from the return slot on
  demand rather than stored as a flag. A deferred return (`-> :(TYPE OF er)`) resolves it at
  dep-finish through `classify_return_type`'s existing `Deferred` verdict arm
  ([`return_type.rs`](../../src/builtins/fn_def/return_type.rs)), the same arm FUNCTOR's return
  validation uses today.
- *Application surface — decided.* The Type-head `TypeCall` arm that resolves a
  `KType::KFunctor { body: Some(_) }` is deleted; a module-returning FN is called by the ordinary
  keyworded convention.

## Dependencies

**Requires:**

- [Module naming flip](module-naming-flip.md) — a functor's result is a module bound under a
  Type-token name today, and can become an ordinary FN call only once module results bind
  value-side.

**Unblocks:** none tracked yet.
