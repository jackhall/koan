# Module and signature carriers move from KObject to KType

Collapse `KObject::KModule` into a new `KType::Module { module, frame }`
variant and `KObject::KSignature` into a new identity-bearing
`KType::Signature(&'a Signature<'a>)` variant. Split abstract-type
members of modules into a dedicated `KType::AbstractType { source_module,
name }` carrier, and give the type-position wildcards
`KType::AnyModule` / `KType::AnySignature` their own variants. The
KObject/KType boundary contracts: KObject holds strictly value-side
carriers (`Number`, `Str`, `List`, `Dict`, `KFunction`, `KTypeValue`,
struct and tagged values); KType absorbs every type-language entity,
including the ones that mint nominal identity.

**Problem.** Modules and signatures live as both KObject *and* KType today,
and the duality forces every consumer to pick a side:

- `KObject::KModule(&Module, Option<Rc<CallArena>>)` is the value-side
  handle ([kobject.rs:138](../../src/machine/model/values/kobject.rs)); a
  module's *identity* is its `ktype() → KType::UserType { kind: Module,
  scope_id, name }`. A module flowing through dispatch, scope lookup, or
  ATTR projection rides the KObject side; the same module appearing in a
  type-position slot rides the KType side. Same entity, two
  representations.
- `KObject::KSignature(&Signature)` mirrors this for signatures, with
  `ktype()` returning the meta-type `KType::MetaSignature`. The
  identity-bearing role is played by `KType::SatisfiesSignature { sig_id,
  ... }` on the slot-annotation side — a third representation for the
  same source entity.
- Per-call FUNCTOR parameters need the **dual-write** in
  [`KFunction::invoke`](../../src/machine/core/kfunction/invoke.rs) — the
  per-call argument lands in both `bindings.types` (so `:Er.Type` resolves
  in type position) and `bindings.data` (so `Er.pure(x)` resolves in
  value position). The dual-write exists because the two stores are
  authoritative for disjoint lookup directions, and a module-typed
  parameter has to be findable from both.

The conceptual cost is that "modules are first-class values" is a
half-truth — they're first-class for dispatch and ATTR, but second-class
for type identity (which lives in a parallel map). The mechanical cost
is the dual-write plus every callsite that has to know which side of the
duality it's reading.

**Impact.**

- *One representation per entity.* A module is `KType::Module { module,
  frame }`, full stop. A signature is `KType::Signature(&'a
  Signature<'a>)`, full stop. The `KObject::K{Module,Signature}`
  variants, the `KType::MetaSignature` meta-type, and the
  `UserTypeKind::Module` arm all go away; surface "Signature" in
  `from_name` lowers to `KType::AnySignature` (the name freed by the
  MetaSignature retirement).
- *Dual-write disappears.* FUNCTOR param binding writes to
  `bindings.types` only. `Scope::resolve_value("Er")` for a module-typed
  parameter no longer finds the name; ATTR-on-type carries the projection
  for `Er.pure(x)` instead. The `is_type_denoting` gate becomes the
  classification rule for what writes anywhere at all.
- *Scheduler still carries modules and signatures as results.* `MODULE
  Mo = (...)` and `SIG Foo = (...)` evaluate to type values that ride the
  existing `KObject::KTypeValue(KType)` carrier through the scheduler.
  No new KObject variant; the carrier already handles `Number`/`Str`/
  builtin types and now carries the identity-bearing Module/Signature
  variants too.
- *The conceptual claim sharpens.* "Modules are first-class values"
  becomes "the type language is first-class; modules and signatures live
  there." The engine stops pretending modules are values; instead, types
  themselves flow through the scheduler when needed (via `KTypeValue`),
  which they already do.
- *KObject/KType boundary contracts.* KObject becomes the strictly
  value-side carriers; KType absorbs all type-language entities including
  the identity-bearing ones. The duality maintenance burden — every
  consumer choosing which side to read from — disappears.
- *Abstract type members get their own variant.* `KType::AbstractType
  { source_module, name }` carries the arena-pinned source pointer
  directly, so ATTR projection and diagnostics on an opaque-ascription
  abstract type read from the same pointer as `KType::Module` rather
  than walking back through a `UserType { kind: Module, .. }` lookup.

**Directions.**

- *KType variant shapes — decided.* `KType::Module { module: &'a
  Module<'a>, frame: Option<Rc<CallArena>> }` carries the existing
  `KModule` payload verbatim. `KType::Signature(&'a Signature<'a>)` is
  the identity-bearing successor, paralleling `KType::Module`'s
  arena-pointer shape. `KType::AbstractType { source_module: &'a
  Module<'a>, name: String }` replaces `UserType { kind: Module, .. }`
  for opaque-ascription abstract type members — named for what it
  actually is in OCaml terms (an abstract type *member* of a module,
  like `M.t`, not "an abstract module" per se). Manual `PartialEq` on
  `AbstractType` compares `(source_module.scope_id(), name)` rather than
  pointer equality, so two opaque-ascriptions of the same source module
  with the same abstract name compare equal.
- *KType lifetime parameterization — decided.* `KType` becomes
  `KType<'a>`, aligning with `KObject<'a>`, `KFunction<'a>`,
  `Module<'a>`. The exception that "KType happens to be all owned data"
  ends as soon as a type-language entity closes over an arena-pinned
  scope. Blast radius: ~60 struct fields and ~16 fn signatures need the
  annotation; the ~900 `KType::Number`/`Any`/`List(...)` constructor sites
  are untouched (Rust infers the parameter).
- *Type-position wildcards — decided.* `:Module` lowers to
  `KType::AnyModule` and `:Signature` lowers to `KType::AnySignature`.
  Dedicated wildcard variants rather than reusing `AnyUserType { kind:
  Module | Signature }`; with `UserTypeKind::Module` dropped entirely,
  the `:Module` slot's semantics sharpen to "admits first-class
  modules," not "admits abstract-types-from-some-module."
- *ATTR-on-type — decided.* Ships as the **first commit** of this
  item — without it, dropping the dual-write breaks `Er.pure(x)` after
  per-call binding moves to `bindings.types`-only. New ATTR arms on
  `KObject::KTypeValue(...)`: `KType::Module { module, .. }` →
  `access_module_member(module, field)`; `KType::Signature(s)` →
  reverse-lookup against `s`; `KType::AbstractType { source_module, .. }`
  → project against `source_module`; `KType::Number | Str | Bool | Null
  | ...` → clean rejection ("type X has no members"). The existing
  `KObject::KModule(_, _)` arm
  ([`attr.rs`](../../src/builtins/attr.rs)) becomes the
  `KTypeValue(KType::Module { .. })` arm.
- *`SatisfiesSignature` at the binding site — decided.* Replaced with
  `KType::Module { module, frame }` rather than extended with a
  `source_module_id`. Once a FUNCTOR's signature-typed parameter binds,
  the module identity is already in hand — write the module variant
  directly and let ATTR-on-type project from it. `SatisfiesSignature`
  the variant stays for slot-annotation use (the row in `type_identity_for`
  changes its mint shape; the variant definition is untouched).
- *Scheduler result plumbing — decided.* MODULE/SIG declarators wrap
  their result KType in `KObject::KTypeValue(KType)`. No new KObject
  variant; this carrier already exists for builtin type values.
- *Migration ordering — decided.* Big-bang within a feature branch,
  sequenced as five coherent commits: (1) parameterize `KType<'a>` as a
  no-op prelude; (2) add the new variants with predicate/name/elaboration
  arms while old variants stay in place; (3) flip producers
  (MODULE/SIG/opaque-ascription) and lockstep consumers (ATTR, lift,
  `derive_nominal_identity`, `using_scope`, `type_identity_for`); (4)
  remove the old carriers (`KObject::KModule`, `KObject::KSignature`,
  `KType::MetaSignature`, `UserTypeKind::Module`) and drop the FUNCTOR
  `bind_value`-for-type-denoting-params branch in `invoke.rs`; (5)
  verify the
  [`node_store.rs`](../../src/machine/execute/scheduler/node_store.rs)
  `read_result` panic is dissolved by re-running a FUNCTOR with a
  signature-typed parameter, or attribute and file a follow-up if a
  residual race remains. Step 1 is mechanical and reviewable on its own;
  step 4 is the load-bearing one.

## Dependencies

**Requires:**

**Unblocks:**

- [VAL-slot ATTR re-tagging](val-slot-attr-retagging.md) — ATTR-on-type
  is the projection path the re-tagging lives inside; once the substrate
  move puts the per-call frame and `&Module` pointer both in hand at the
  ATTR call site, re-tagging VAL-slot reads with the abstract identity is
  a local edit to `access_module_member`.
- [FUNCTOR binder](functor-binder.md) — landing the
  substrate move first lets FUNCTOR's signature-typed-parameter handling
  ride the single-store machinery from day one. The functor-definition
  panic on the dual-write path
  ([node_store.rs](../../src/machine/execute/scheduler/node_store.rs),
  folded into functor-binder.md as a known blocker) is likely dissolved
  or relocated as a side effect of this work.
