# Module system stage 0 — Pre-module cleanup

**Problem.** The shipped struct and tagged-union substrate is shaped for the
value-language world stage 1 will subsume.

- [`KType::Struct`](../src/dispatch/types/ktype.rs) and `KType::Tagged` are
  flat singleton tags — every user struct (or tagged union) reports the same
  `KType` regardless of declaration.
- [`KType::from_type_expr`](../src/dispatch/types/ktype.rs) and
  [`parse_typed_field_list`](../src/dispatch/types/typed_field_list.rs)
  resolve type names through a hard-coded `from_name` table with no scope
  hook, so user-defined type names can't appear in struct schemas or in
  parameter annotations on `FN` signatures.
- Constructor dispatch on `TaggedUnionType` / `StructType` is duplicated
  between [`type_call`](../src/dispatch/builtins/type_call.rs) and
  [`call_by_name`](../src/dispatch/builtins/call_by_name.rs).
- [`KType::TypeRef`](../src/dispatch/types/ktype.rs) is unused — its own
  doc comment admits *"currently no remaining uses, but kept as a vestigial
  slot kind."*
- [`KObject::Struct.fields`](../src/dispatch/values/kobject.rs) is
  `Rc<HashMap<String, KObject>>` while its schema is an ordered
  `Rc<Vec<(String, KType)>>`; struct values lose the declaration order
  schemas preserve, and `PRINT` of a struct emits fields in
  HashMap-iteration order.

**Impact.**

- *Stage 1 substrate work stays local.* Adding `KType::ModuleType
  { module_path, name }` and a per-scope module registry becomes an
  additive change at the resolution path and the dispatch table, instead of
  a wave that also reshapes the parser, the constructor-dispatch sites,
  and every consumer of `KType::from_name`.
- *`PRINT` of struct values is deterministic.* The runtime preserves
  declaration order on the value side the same way it already does on the
  schema side.
- *Constructor dispatch has one growth point.* Stage 3 (first-class
  modules) extends a single helper to add the module-value-as-constructor
  case rather than two parallel match arms.

**Directions.** None decided.

- *Delete `KType::TypeRef`.* Audit references; remove the variant, the
  `from_name("TypeRef")` line, the `TypeExpr` resolution path, and the
  matching `accepts_part` arm. If a remaining caller surfaces, either
  reroute it to `TypeExprRef` or document the kept-for-reason in
  [`ktype.rs`](../src/dispatch/types/ktype.rs).
- *Order-preserving struct values.* Switch
  [`KObject::Struct.fields`](../src/dispatch/values/kobject.rs) to
  `Rc<Vec<(String, KObject<'a>)>>` (or a small ordered-map wrapper). Update
  [`struct_value::construct`](../src/dispatch/values/struct_value.rs),
  [`attr::access_field`](../src/dispatch/builtins/attr.rs), and
  `Parseable::summarize` to walk the vector. The schema side is already
  ordered, so the asymmetry collapses.
- *Centralize constructor dispatch.* Extract a single
  `dispatch_constructor(scope, verb_obj, args_parts) -> BodyResult` helper
  (likely in [`dispatch::values`](../src/dispatch/values.rs)) that handles
  the `TaggedUnionType` / `StructType` cases. Both
  [`type_call::body`](../src/dispatch/builtins/type_call.rs) and
  [`call_by_name::body`](../src/dispatch/builtins/call_by_name.rs) call it.
  Stage 3 adds the module-value case at the same site.
- *Thread `Scope` (or a focused `TypeResolver`) through type resolution.*
  Add a context parameter to
  [`KType::from_type_expr`](../src/dispatch/types/ktype.rs) and
  [`parse_typed_field_list`](../src/dispatch/types/typed_field_list.rs).
  The body stays hard-coded until stage 1 lands the registry; the call
  signatures are ready in advance.
- *Mark transitional `KType` variants.* `KType::Struct` and `KType::Tagged`
  get a comment near each declaration noting they will be replaced by
  module-typed variants in stage 1, so future readers see the trail.

## Dependencies

**Requires:** none. This is the foundation cleanup.

**Unblocks:**
- [Stage 1 — Module language](module-system-1-module-language.md)
