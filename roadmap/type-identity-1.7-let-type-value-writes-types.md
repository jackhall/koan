# Type identity stage 1.7 — `LET Ty = Number` routes through `register_type`

Completes the Type-class LET storage split: `LET Ty = Number` (Type-class
LHS, type-valued RHS) writes the `types` map via
`Scope::register_type`, not `data` via `bind_value`. Couples with an
ascribe migration — without it, SIG abstract-type members become invisible.

**Problem.** After [stage 1.4](type-identity-1.4-scope-resolve-type-and-rewire.md),
`LET Ty = Number` still routes through `bind_value` (writes `data`). Type
names introduced by LET therefore live in a different binding home than
builtin type names (which live in `types`). The asymmetry blocks alias
transparency: `LET Ty = Number; resolve_type("Ty")` returns `None`, so a
use site `: Ty` can't pick the `Number` overload via the same `&KType`
pointer as a direct `: Number` use.

A coupling: ascription's
[`is_abstract_type_name`](../src/runtime/builtins/ascribe.rs) helper scans
`decl_scope().bindings().data()` to discover SIG abstract-type members
(`Type Ty = ...`). After this sub-item routes those declarations through
`register_type` (writes `types`), the scan misses them. The ascribe-side
migration must ship in the same PR.

**Impact.**

- *Alias transparency.* `LET Ty = Number` makes `Ty` and `Number`
  observably the same `&KType` at dispatch.
- *SIG abstract-type members continue to resolve.* `is_abstract_type_name`
  scans both maps.

**Directions.**

- *LET routing — decided.* In the LET `TypeExprRef`-LHS overload at
  [`let_binding.rs`](../src/runtime/builtins/let_binding.rs): when the
  RHS is type-valued (passes the [stage 1.6](type-identity-1.6-let-typeclass-bind-error.md)
  check), unwrap the inner `KType` and call `scope.register_type(name,
  kt)` instead of `scope.bind_value(name, allocated_object)`. The
  `BodyResult::Value` return still hands back a `KObject::KTypeValue`
  for dispatch transport — the storage move changes where the binding
  lives, not the dispatch carrier shape.

- *Ascribe both-map scan — decided.* Shared helper
  `abstract_type_names_of(scope: &Scope) -> Vec<String>` that walks both
  `bindings.data()` (filtering type-classed names not already in
  `types`) and `bindings.types()`. Used by both
  `is_abstract_type_name`'s call site and `shape_check` at
  [`ascribe.rs`](../src/runtime/builtins/ascribe.rs). Goes through
  `Bindings` (no raw RefCell access outside the façade).

- *Test migration — decided.* `run_tests.rs:235` and
  `fn_def/tests/module_stage2.rs:20` switch their `scope.lookup("ty")`
  assertions to `scope.resolve_type("ty")`.

- *Pairing ascribe migration with LET routing — decided.* The alternative
  (defer LET routing entirely until stage 3) carries the asymmetry across
  stages 1–2. Migrating ascribe to scan both maps is the local cost paid
  here; the deferred-alternative's cost was three stages of asymmetry.

## Dependencies

**Requires:**

- [Stage 1.4 — `Scope::resolve_type` and `register_type` rewire](type-identity-1.4-scope-resolve-type-and-rewire.md)
  — `register_type` must already write `types`.
- [Stage 1.6 — `TypeClassBindingExpectsType` bind-time error](type-identity-1.6-let-typeclass-bind-error.md)
  — the routing change runs only when 1.6's check passes.

**Unblocks:**

- [Type identity stage 2 — `KObject::TypeNameRef` carrier](type-identity-2-typename-ref-carrier.md)
  — operates against a uniform Type-class binding home.
