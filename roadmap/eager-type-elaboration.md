# Eager type elaboration with placeholder-based recursion

**Problem.** Type elaboration today is synchronous, name-keyed, and detached
from the scheduler that drives value evaluation. Three concrete gaps:

- *Deferred per-lookup elaboration.* First-class type values in the runtime
  are stored as
  [`KObject::TypeExprValue(TypeExpr)`](../src/dispatch/values/kobject.rs) — the
  parser's surface form, not the elaborated [`KType`](../src/dispatch/types/ktype.rs).
  Resolution is deferred until consultation:
  [`ScopeResolver::resolve`](../src/dispatch/types/resolver.rs)
  re-elaborates the stored `TypeExpr` against the *current* scope on every
  lookup, using
  [`KType::from_type_expr`](../src/dispatch/types/ktype_resolution.rs) with a
  [`NoopResolver`](../src/dispatch/types/resolver.rs) on the recursive arm to
  suppress further shadowing inside type parameters. The runtime carries two
  type representations in parallel (`TypeExpr` for stored values, `KType` for
  dispatch slots), and consumers throughout dispatch reach into `TypeExpr`
  for `.name` and `.params`
  ([`attr.rs`](../src/builtins/attr.rs),
  [`let_binding.rs`](../src/builtins/let_binding.rs),
  [`argument_bundle.rs`](../src/dispatch/kfunction/argument_bundle.rs),
  [`type_call.rs`](../src/builtins/type_call.rs),
  [`type_ops.rs`](../src/builtins/type_ops.rs),
  [`value_lookup.rs`](../src/builtins/value_lookup.rs),
  [`struct_def.rs`](../src/builtins/struct_def.rs),
  [`fn_def.rs`](../src/builtins/fn_def.rs),
  [`module.rs`](../src/dispatch/values/module.rs)).
- *Synchronous FN-signature elaboration.* Parens-wrapped type expressions in
  FN parameter positions (`xs: (LIST_OF Number)`) aren't sub-dispatched:
  `parse_fn_param_list` in
  [`builtins/fn_def.rs`](../src/builtins/fn_def.rs) only accepts
  `ExpressionPart::Type(t)` triples and routes them through the synchronous
  `KType::from_type_expr`. FN-def's `ScopeResolver` does a synchronous
  `scope.lookup(name)` and returns `None` rather than parking on a
  dispatch-time placeholder, so a type identifier bound by an earlier
  top-level `LET MyType = (LIST_OF Number)` only resolves in a sibling FN
  signature when the LET has finalized by the time the FN body runs;
  today's tests work around this by putting MODULE / SIG declarations in a
  prior batch. Value-name forward references already park on placeholders
  via the
  [`Scope::placeholders`](../src/dispatch/runtime/scope.rs) sidecar and the
  scheduler's `notify_list` / `pending_deps` machinery — the type-name path
  is the gap.
- *Recursive type definitions.* `STRUCT Tree { children: List<Tree> }` and
  mutually recursive groups (`STRUCT TreeA { b: TreeB } / STRUCT TreeB { a:
  TreeA }`) have no surface support. Adding the elaboration pieces above
  without a self-reference recognition mechanism would deadlock recursive
  definitions on their own placeholder under the uniform-park rule.

The deferred-elaboration model also forces the "shadow only at the top
level, not inside type parameters" choice that `NoopResolver` exists to
enforce; recursive shadowing through deferred re-elaboration would let a
bound type alias's RHS look up names in a scope that includes itself,
opening cycle risk (`LET T = List<T>` re-resolves `T` on every lookup and
never terminates).

**Impact.**

- *One canonical runtime type representation.* `KObject::KTypeValue(KType)`
  replaces `KObject::TypeExprValue(TypeExpr)`. Type-builtins (`LIST_OF`,
  `DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF`, `STRUCT_OF`, `UNION_OF`)
  construct `KType` directly. `TypeExpr` is confined to the parse →
  elaborate seam; nothing downstream of FN-signature elaboration handles
  surface syntax. Consumers that today pull `t.name` / `t.params` operate
  on `KType` shape directly, so the surface/elaborated bookkeeping
  disappears.
- *Type expressions assemble end-to-end inside FN signatures.* Top-level
  type bindings (`LET MyType = (LIST_OF Number)`) and parameterized type
  expressions inside FN parameter lists compose freely: a `FN (USE xs:
  MyType)` waking the binding behaves the same as `FN (USE xs: (LIST_OF
  Number))`, and either form can be tightened as inference proceeds with
  dependents waking on the refinement. Submission order stops being
  load-bearing for type-name forward references the way it already
  isn't for value-name ones. Shadowing composes uniformly through the
  lexical scope chain at bind time; `NoopResolver` is deletable;
  `ScopeResolver` becomes a flat name → `KType` lookup.
- *Recursive type definitions are first-class.* `STRUCT Tree { children:
  List<Tree> }` and mutually recursive groups elaborate cleanly via
  threaded-set self-reference recognition during body elaboration, with
  the result wrapped in `KType::Mu { binder, body }` when self-references
  fired. Trivially cyclic aliases (`LET T = T`) surface as a structured
  error rather than a stack overflow.
- *Diagnostics gain a `KType` renderer.* Type-value printing
  ([`kobject.rs`](../src/dispatch/values/kobject.rs)) routes through a
  `KType::render` instead of `TypeExpr::render`. Error messages that today
  print `TypeExpr` surface text print elaborated `KType` instead, with
  `Mu` / `RecursiveRef` rendered as the binder name (e.g. `Tree`) so
  recursion stays readable.

**Directions.**

- *Runtime representation — decided.* Replace
  `KObject::TypeExprValue(TypeExpr)` with `KObject::KTypeValue(KType)`.
  Every consumer migrates off `t.name` / `t.params` to the `KType` shape.
  Type-builtins return `KType` directly. The migration is a single coherent
  change, not a parallel-representation interim — keeping both shapes alive
  during the migration would re-introduce the surface/elaborated coupling
  this work removes.
- *Recursion encoding — decided.* Two new `KType` variants, both
  permanent, both describing finalized recursion:
  - `KType::Mu { binder: Path, body: Box<KType> }` — recursive type with
    binder name in scope inside the body.
  - `KType::RecursiveRef(Path)` — back-reference to an enclosing `Mu`
    binder.

  No transient `KType::Placeholder` variant. The "currently elaborating"
  state lives in the elaborator's call frame as a threaded set of binder
  names, not in the type language. Cycle-aware traversals (equality,
  printing, hashing) carry an "inside this `Mu` binder" set so
  back-references terminate after one unfold.
- *Bind-time elaboration with threaded-set self-reference recognition —
  decided.* Every type-binding site (`LET T = ...`, `STRUCT T = ...`,
  `UNION T = ...`) registers a scheduler placeholder in
  `Scope::placeholders` — the same sidecar value bindings use — and
  dispatches its body as scheduler work. The elaborator threads a set of
  binder names currently being elaborated. A name lookup during
  elaboration:

  - *In the threaded set.* Return `KType::RecursiveRef(name)` directly. Do
    not park. This is what keeps recursive type definitions from
    deadlocking on their own placeholder.
  - *Resolves to a finalized binding.* Return the stored `KType`.
  - *Resolves to a `Scope::placeholders` entry not in the threaded set.*
    Park on that producer NodeId via the standard `notify_list` /
    `pending_deps` machinery, exactly the way value elaboration parks on
    value-name placeholders today.
  - *Unbound.* Structured error.

  At binding finalization, a single boolean tracked through elaboration
  decides the wrap: bare body if no self-reference fired, `Mu { binder:
  name, body }` if any did. No post-walk over the body. No upfront
  recursion detection, no parser-side annotation, no separate-syntax
  distinction between recursive and non-recursive definitions.
- *Mutual recursion — decided.* At top-level, batch-register every name
  in a strongly-connected declaration group as a scheduler placeholder
  before elaborating any body, and seed the elaborator's threaded set
  with all SCC member names. Any back-reference from any SCC member's
  body to any other member's name returns `RecursiveRef(name)` directly.
  SCC discovery rides on the existing scheduler (each binding's body
  elaboration is scheduler work; mutual references inside the SCC
  short-circuit, mutual references outside the SCC park on each other's
  placeholders the same way value forward references park).
- *Parens-wrapped type expressions sub-dispatch — decided.* A parameter
  position written `xs: (LIST_OF MyType)` schedules the parens-wrapped
  part as a sub-Dispatch; its `KObject::KTypeValue` result splices in via
  the standard `Bind` path. An `elaborate_type_expr` helper in
  [`src/dispatch/types/resolver.rs`](../src/dispatch/types/resolver.rs)
  is the shared entry point.
- *Bare type identifiers park on scheduler placeholders — decided.*
  FN-def's signature elaboration consults `Scope::placeholders` extended
  with type-binding placeholders. A type name whose binder has dispatched
  but not finalized parks the elaborating slot via the same `notify_list`
  / `pending_deps` machinery value-name forward references use today. The
  placeholder is a NodeId, not a name string — recursion recognition is
  the elaborator's threaded-set responsibility, not the placeholder's.
  Names not yet even dispatched at FN-definition time (signature-typed
  parameters whose type comes from a SIG only in scope at functor
  application time) carry the original `TypeExpr` on the resulting
  `KFunction`; the first call re-runs resolution against the FN's
  captured scope and memoizes the result (one `OnceCell<KType>` per slot,
  sound because the captured scope is lexically fixed). The OnceCell
  fallback narrows to genuine functor late-binding cases; top-level and
  lexical-scope cases are handled at bind time and the OnceCell never
  fires there.
- *`NoopResolver` removal — decided.* Falls out of the migration: with
  bindings storing elaborated `KType` directly, `ScopeResolver::resolve`
  no longer re-elaborates anything, so the "suppress shadowing in inner
  type parameters" use case for `NoopResolver` disappears. The struct and
  the trait's only non-`ScopeResolver` impl can be deleted.
- *`KType` renderer — decided.* Add `KType::render` covering every variant,
  including `Mu` (printed as the binder name) and `RecursiveRef` (printed
  as the bound name). `TypeExpr::render` stays for parser-side use only;
  the `KObject::TypeExprValue → KObject::KTypeValue` migration switches
  type-value printing to `KType::render`.
- *Module-qualified type names — open.* `TypeExpr` carries a name string
  that can naturally hold a path like `MyMod.Number`; `KType` has no
  path-aware variant today. If module-qualified type references ever need
  to flow as type values, either `KType::ModuleType` covers the case
  (already path-shaped) or a new `KType::Qualified(Path)` variant is
  needed. Decision deferred until a use case forces it; the current
  module-system stages don't.
- *Recursion encoding key — decided.* `KType::RecursiveRef(String)` keys
  by binder name only. When [per-declaration type identity for structs
  and tagged unions](per-declaration-type-identity.md) ships, its
  `{ scope_id, name }` carrier inherits the same name; `RecursiveRef`
  resolution walks the enclosing `Mu` or schema-binder context to find
  the concrete identity. No rework of the recursion encoding required
  when per-declaration-identity lands.
- *TCO interaction — decided.* Phase-3 Combine-shaped STRUCT/UNION
  bodies and the phase-4 Combine-shaped FN-def constructor body are
  declaration-time registration paths, not tail sites — they return
  `BodyResult::Value` (or `DeferTo` to the Combine) rather than `Tail`.
  Call-time TCO of the user-defined functions FN-def constructs lives
  in `KFunction::invoke`'s `BodyResult::tail_with_frame`, on a path
  this work doesn't touch.
- *Forward references and partial definitions — open.* Eager elaboration
  means a type alias's RHS must resolve at bind time. Mutual recursion
  is handled by the SCC pre-registration above, but a binding whose RHS
  references a name not yet introduced (e.g. a top-level `LET T = U` where
  `U` is declared later in source) still fails. Whether to extend
  `Scope::placeholders` to typed names beyond the SCC group, or to require
  source-order declaration for non-mutually-recursive aliases, is left
  open until a real use case appears.

## Dependencies

**Unblocks:**
- [Stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md) —
  higher-kinded slot elaboration (`KType::TypeConstructor`), sharing
  constraints (`<Type: E.Type>`), and the remaining stage-2 audit slate
  ride on the scheduler-driven elaborator this work lands.
