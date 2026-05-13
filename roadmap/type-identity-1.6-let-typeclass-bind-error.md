# Type identity stage 1.6 — `TypeClassBindingExpectsType` bind-time error

Adds the bind-time diagnostic for `LET <Type-class> = <non-type>`. Pure
validation surface — no storage change, no consumer migration. Independent
of the other stage 1 sub-items; can land before or after them.

**Problem.** `LET Foo = 1` (Type-class LHS, non-type RHS) binds silently
today. Downstream uses of `Foo` as a type then fail at elaboration time
with `UnboundName` or `ShapeError` — worse messages than a bind-time
rejection would give.

**Impact.**

- *Better error at the actual fault site.* The binder rejects the
  declaration, naming both the binder and the resolved RHS type.
- *Storage routing for the good case stays with stage 1.7.* This sub-item
  covers only the validation surface; `LET Ty = Number` continues to write
  `data` until [stage 1.7](type-identity-1.7-let-type-value-writes-types.md)
  flips the routing.

**Directions.**

- *Error variant — decided.* New
  `KErrorKind::TypeClassBindingExpectsType { name: String, got: KType }`
  in [`kerror.rs`](../src/runtime/machine/core/kerror.rs). Display:
  `` type-class binding `Ty` expects a type value, got `<KType.name>` ``.

- *LET binder rule — decided.* In the LET `TypeExprRef`-LHS overload at
  [`let_binding.rs`](../src/runtime/builtins/let_binding.rs): if the
  resolved RHS value is not type-valued (i.e. `value.ktype() !=
  KType::TypeExprRef`), return `Err(TypeClassBindingExpectsType { name,
  got: value.ktype() })`. Otherwise proceed with the existing
  `bind_value` path. The check fires *after* the existing parameterized-
  name-shape rejection (`LET List<Number> = ...` still errors as
  `ShapeError`).

- *Identifier-LHS path unchanged — decided.* `LET t = Number`
  (lowercase, Identifier-class) continues to bind `KObject::KTypeValue(
  KType::Number)` into `data`. The check only fires on the Type-class
  overload.

## Dependencies

**Requires:** none — independent, storage-neutral.

**Unblocks:**

- [Stage 1.7 — `LET Ty = Number` routes through `register_type`](type-identity-1.7-let-type-value-writes-types.md)
  — natural pairing: 1.6 rejects the bad case, 1.7 routes the good case.
