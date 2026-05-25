# Module and signature carriers move from KObject to KType

Collapse `KObject::KModule` into a new `KType::Module { module, frame }`
variant and `KObject::KSignature` into a new identity-bearing
`KType::Signature { scope_id, name, slots, ... }` variant. The
KObject/KType boundary contracts: KObject holds strictly value-side
carriers (`Number`, `Str`, `List`, `Dict`, `KFunction`, `KTypeValue`, struct
and tagged values); KType absorbs every type-language entity, including
the ones that mint nominal identity.

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
  frame }`, full stop. A signature is `KType::Signature { ... }`, full
  stop. The `KObject::K{Module,Signature}` variants and the
  `MetaSignature` meta-type all go away; surface "Signature" in
  `from_name` lowers to the identity-bearing `KType::Signature` (the name
  freed up by the prior MetaSignature rename).
- *Dual-write disappears.* FUNCTOR param binding writes to
  `bindings.types` only. `Scope::resolve_value("Er")` for a module-typed
  parameter no longer finds the name; ATTR-on-type carries the projection
  for `Er.pure(x)` instead. The `is_type_denoting` gate becomes the
  classification rule for what writes anywhere at all.
- *Scheduler still carries modules and signatures as results.* `MODULE
  Mo = (...)` and `SIG Foo = (...)` evaluate to type values that ride the
  existing `KObject::KTypeValue(KType)` carrier through the scheduler.
  No new KObject variant; the carrier already handles `Number` /`Str`/
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

**Directions.**

- *KType variant shapes — decided.* `KType::Module { module: &'a
  Module<'a>, frame: Option<Rc<CallArena>> }` carries the existing
  `KModule` payload verbatim. `KType::Signature { scope_id, name, slots,
  ... }` is the identity-bearing successor — the bare name `Signature`
  was freed by the `MetaSignature` rename (see roadmap/libraries/[functor-binder.md](functor-binder.md)
  for the rename rationale).
- *MetaSignature retirement — decided.* `KType::MetaSignature` goes
  away. A `:Signature` slot annotation lowers to `KType::AnyUserType
  { kind: Signature }`, mirroring how `:Module` lowers to `AnyUserType
  { kind: Module }`. `UserTypeKind` gains a `Signature` arm.
- *ATTR-on-type — decided prerequisite, decision-point open.*
  `Er.pure(x)` after the move requires ATTR to project members from a
  type-language entity. New ATTR arms: `KType::Module` and `KType::Signature`
  (reverse-lookup or direct via the carried `&Module` / `&Signature`
  pointer); `KType::SatisfiesSignature` (reverse-lookup through the
  module identity carried alongside, see open bullet below);
  `KType::Number`/`Str`/etc. (clean rejection — no members).
  Whether ATTR-on-type ships as a separate roadmap item *before* this
  collapse or as the first commit of this item is open — splitting lets
  the FUNCTOR binder shed its dual-write earlier, but ships ATTR-on-type
  without the full coherence benefit.
- *`SatisfiesSignature` carrier — open.* For ATTR-on-type to project
  members through a `:OrderedSig` parameter binding, the
  `SatisfiesSignature` carrier needs to identify the *underlying source
  module*, not just the SIG. Today it carries `sig_id` (the SIG
  identity) and `pinned_slots` but no module identity. Options: add
  `source_module_id` to the carrier, or replace `SatisfiesSignature`
  with `KType::Module { ... }` at the binding site once the module's
  identity is known (the variant collapse pulls the latter into reach).
- *Scheduler result plumbing — decided.* MODULE/SIG declarators wrap
  their result KType in `KObject::KTypeValue(KType)`. No new KObject
  variant; this carrier already exists for builtin type values.
- *Migration ordering — open.* The move touches dispatch (admission
  arms), scope lookup, ATTR (new arms), LET binding (write target
  switches from `data` to `types`), FUNCTOR invoke (drop the
  `bind_value` half), and every `match obj { KObject::KModule(...) }`
  consumer. Big-bang vs. staged: a staged migration probably introduces
  the new KType variants first behind a feature gate and migrates
  consumers one at a time; a big-bang flips everything at once and lives
  with a broken-build period.

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
  ([node_store.rs:169](../../src/machine/execute/scheduler/node_store.rs),
  folded into functor-binder.md as a known blocker) is likely dissolved
  or relocated as a side effect of this work.
