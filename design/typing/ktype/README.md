# `KType` — the runtime type system

[`KType`](../../../src/machine/model/types/ktype.rs) has a variant for every concrete `KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List(Box<KType>)`, `Dict(Box<KType>, Box<KType>)`,
  `KFunction { params: Record<KType>, ret: Box<KType> }`. Always parameterized; see
  [Container type parameterization](parameterization-and-variance.md#container-type-parameterization) below.
  `params` is a name-keyed [parameter `Record<KType>`](records-and-limits.md#record-fields-and-ktype-hashing),
  so a function-typed slot's identity is its parameters by name and type
  (order-blind). `KFunction` is the *only* function-type variant: a functor — a
  module-returning function — reports it too, so it is admissible wherever a
  same-shape `:(FN …)` slot matches (see [functors.md](../functors.md)). When a
  function's source return is
  per-call-elaborated, its `ret` box holds a `DeferredReturn(DeferredReturnSurface)`
  carrier — see [Record fields and `KType` hashing](records-and-limits.md#record-fields-and-ktype-hashing).
- Structural record: `Record(Box<Record<KType>>)` — an identifier-keyed field schema
  (`:{x :Number, y :Str}`), distinct from a nominal `NewType`-kind `SetRef`
  (a structural record is anonymous; a record-repr newtype is nominal). A record *value* (`KObject::Record`,
  surface `{x = 1, y = "a"}`) memoizes its per-field type record as its carried type.
  Width/depth subtyping orders record *values* in the dispatch lattice — see
  [Variance](parameterization-and-variance.md#variance).
- Other function-like: `KExpression` (a captured-but-unevaluated expression).
- Meta-type for type-position slots: `OfKind(KKind)` — a type-accepting slot carries
  a shallow [`KKind`](../../../src/machine/model/types/kkind.rs) expectation, and a type value
  flowing in the value channel's `Type` arm is classified by `KType::kind_of` and matched
  against it by subsumption. `OfKind` is **type-channel-only**: it admits a type value, never
  a runtime instance — a value is matched by a type, never by a kind. The kinds form one
  subsumption lattice, `Any > {Signature, Proper > {Newtype, TypeConstructor}}`:
  a parsed type-name slot is `OfKind(Proper)`, the `:Type` surface is `OfKind(Any)`, the
  `:Signature` wildcard is `OfKind(Signature)` (`:Module` instead lowers to the empty
  signature — see the module / signature carriers below), and the two
  nominal families sit strictly below `Proper`. There is no `Module` kind: a module is a
  *value*, matched by a signature type, so it never reaches an `OfKind` slot at all.
  `KKind::admits` is reflexive subsumption (a
  `Proper` / `Any` slot admits any proper-subtree type value, while the signature wall
  keeps that family to itself); `KKind::strictly_below` orders specificity, so an
  `OfKind(Newtype)` slot out-specifies an `OfKind(Proper)` sibling. See
  [Type-position slot kinds](slots-and-signatures.md#type-position-slot-kinds).
- First-class type values: a type flows raw as a `&KType` in the value channel's `Type`
  arm — there is no `KObject` box. As a parameter-slot annotation, `OfKind(Proper)` (`:Type`'s
  `OfKind(Any)` likewise) admits any *proper* type value: bare builtin type tokens (`Number`,
  `Str`, `Bool`, `Null`), newtype and union nominal tokens, an anonymous `Union` type value, and
  any other non-signature type. A signature value routes through the dedicated
  `OfKind(Signature)` slot, and a module value (riding the value channel's Object arm as
  `KObject::Module`) through a `Signature { .. }` slot, so the `:Type` vs `:Module` overload
  distinction stays intact — see
  [`KType::accepts_part`](../../../src/machine/model/types/ktype_predicates.rs)
  and the pin test
  [`type_slot_admits_bare_builtin_tokens_and_user_type_carriers`](../../../src/machine/model/types/ktype_predicates/tests.rs).
- User-declared nominal types — three variants reference members of an
  `Rc`-owned [`RecursiveSet`](../../../src/machine/model/types/recursive_set.rs),
  the atomic unit of nominal allocation, identity, and lift (one strongly-connected
  component of mutually-recursive types; a non-recursive type is a singleton set).
  See [user-types.md](../user-types.md) for the full model.
  - `SetRef { set: Rc<RecursiveSet>, index }` — the **external** handle, the
    nominal identity synthesized by `KObject::ktype()` for `Wrapped` and
    `Tagged` carriers and held by `bindings.types`. Identity is
    `(set digest, index)` — the set's sealed content digest plus the member index,
    via [`same_nominal`](../../../src/machine/model/types/recursive_set.rs), never
    the schema (which may be cyclic); the `Rc` is content transport only, with a
    set-pointer fast path for the shared and pre-seal cases. Structurally identical
    declarations therefore unify — the same `NEWTYPE` elaborated twice denotes one
    type — rather than staying per-declaration distinct. The member's `kind` (read via
    `set.member(index).kind`) is one of the nominal families `KKind::{Newtype,
    TypeConstructor}` — `kind_of` reads it off the `SetRef` to classify the nominal type
    value. A user `UNION` seals one `NewType` member per variant, so each variant is a
    `SetRef`; the union name binds the anonymous `Union` of those `SetRef`s. Lift
    `Rc::clone`s the whole set, so the recursive group travels as one cycle-aware unit.
  - `SetLocal(index)` — the **intra-set sibling** reference inside a member's
    schema, a bare index resolved against the ambient set during deep traversal.
    It carries no `Rc` (so the set holds no internal refcount cycle) and never
    reaches the predicates — matching is shallow `SetRef` identity that does not
    descend a member's schema.
  - `RecursiveGroup(Rc<RecursiveSet>)` — the first-class handle to a whole set,
    bound by a `RECURSIVE TYPES` group name. Identity is the set's content digest
    (via `same_nominal`, index-free); inert in value dispatch.
  A slot that wants "any user-declared type of family X" is an `OfKind(KKind)`
  carrying the nominal family (`OfKind(Newtype)` / `OfKind(TypeConstructor)`).
  Because `OfKind` is type-channel-only, such a slot
  admits the *type value* of that family, not a runtime instance — a builtin that
  dispatches on a runtime representation (ATTR's newtype field access) takes the
  least-specific `Any` slot and validates the `KObject::Wrapped` shape in its body
  (`access_field`), never matching the value by a kind. The nominal-family surface
  keywords (`Newtype` / `TypeConstructor`) are pinned for diagnostic
  rendering only — none is registered as a writable surface name (no entry in
  [`KType::from_name`](../../../src/machine/model/types/ktype_resolution.rs)).
- `Union(Vec<KType>)` — an **untagged structural disjunction**, the type `:(A | B)`.
  Not a set-member reference: it composes any member types, canonicalized by
  [`KType::union_of`](../../../src/machine/model/types/ktype_resolution.rs) —
  flattened, deduplicated, and collapsed to the lone member when only one survives
  (`:(A | A)` is `:A`). Identity is order-blind: the stored digest sorts its member
  digests, so `:(A | B)` equals `:(B | A)` under `PartialEq` / `Hash`. A union admits any value one of its members admits, and
  each member is strictly more specific than the union
  ([`is_more_specific_than`](../../../src/machine/model/types/ktype_predicates.rs)), so a
  union-typed slot dispatches by the value's own runtime type. `kind_of` reports
  `Proper`. A user `UNION` binds the anonymous union of its per-variant `NewType`
  `SetRef`s. See [user-types.md § Unions dissolve into per-variant newtypes](../user-types.md#unions-dissolve-into-per-variant-newtypes).
- `RecursiveRef(String)` — a **definition-time transient only**: a self or
  forward-sibling reference lowers to it during elaboration and the member's
  finalize seals it to `SetLocal(index)`. It never appears in a sealed type and
  never reaches the predicates. Equality is by name only.
- Module / signature carriers (the [module system](../modules.md) rests on
  these): **there is no module variant.** A module is a value — it rides the value
  channel's Object arm as `KObject::Module`, and its `ktype()` is its principal
  signature, so the type channel names a module only through
  `Signature { sig: SelfOf(m), .. }`. A module name is a value token and types
  nothing on its own; `TYPE OF` is the door that surfaces that self-sig as a type
  value (`m :(TYPE OF int_ord)`, `-> :(TYPE OF er)`) — see
  [modules.md § Modules in type position](../modules.md#modules-in-type-position-type-of).
  `Signature { sig: SigSource<'a>, pinned_slots: Vec<(String, KType)> }`
  serves both signature roles in one variant — its `sig` names one of three
  module-lattice points ([`SigSource`](../../../src/machine/model/types/ktype.rs):
  `Declared` SIG, `SelfOf` module self-sig, `Empty`) — the introspectable value
  (a `Declared`, carrying `decl_scope` via `sig`) *and* the dispatch constraint ("any
  module satisfying this signature"). It is the one variant holding a live region
  pointer (`&'a Module` / `&'a Signature`), so it is also the one that cannot rebuild
  at `'static`.
  `AbstractType { source: ScopeId, name: String }` is the per-abstract-type-member
  tag — **owned data**, id-keyed, with no `&Module` inside it. The single variant has
  two **minting sites**, and the distinction between them is load-bearing for
  generativity even though the representation is one shape: `source` is the SIG decl
  scope's id for a member named at SIG-declaration time (a SIG-local `TYPE Carrier`
  binds this name-bearing tag rather than collapsing to an underlying type),
  or the freshly-allocated ascription module's scope id for the per-call
  mint `:|` opaque ascription produces (`view.Carrier`). Because each `:|` application
  allocates a fresh child scope, the two never collide.
  Manual `PartialEq` keys `KType::AbstractType` on `(source, name)`, while
  `KType::Signature` compares by its stored content
  digest (which folds `sig.sig_id()` and `pinned_slots`; the `SigSource`'s `path()`
  is diagnostic-only) — so two
  opaque ascriptions of the same source module mint distinct abstract identities
  (the abstraction barrier) while two `AbstractType` carriers minted from the *same*
  ascription for the same slot name compare equal, and a per-call mint stays distinct
  from the SIG-declared member it was threaded from.
  Projecting a member off a bare type-channel `AbstractType` is an error: the
  identity names no receiver, and further members project off the module value
  ([`attr.rs`](../../../src/builtins/attr.rs)).
  The companion wildcard `OfKind(Signature)` admits any signature value; the
  surface keyword `Signature` lowers to it in
  [`KType::from_name`](../../../src/machine/model/types/ktype_resolution.rs),
  while `Module` lowers to the empty signature (`Signature { SigSource::Empty }`),
  the module-lattice top every module value satisfies.
  The single `Signature` variant is **disambiguated by position**: a
  `Signature { .. }` *slot* matches a *module value* (on the value channel's Object
  arm) whose self-sig structurally
  satisfies `sig` (the constraint role — what `er :Ordered`
  lowers to in an FN parameter slot, so `:Ordered` means "module
  satisfying Ordered," never "the signature value itself"), while a
  signature *value* (a `KType::Signature { .. }` flowing in the `Type` arm) is matched only
  by the `OfKind(Signature)` wildcard. `pinned_slots` (empty for a bare
  signature) carries `WITH` abstract-type specializations; because the
  same variant rides a live `&Signature`, a `WITH` result is
  introspectable too.
- Higher-kinded application: `ConstructorApply { ctor: Box<KType>, args:
  Vec<KType> }` — structural identity by `(ctor, args)`, mirror of `List(_)`
  / `Dict(_, _)`. `ctor` is a `SetRef` to a `TypeConstructor`-kind member.
  Emitted by `elaborate_type_expr` when the outer name of a parameterized type
  expression resolves to such a member; renders as `ctor<arg1, arg2>` in
  diagnostics. See
  [functors.md § Higher-kinded type slots](../functors.md#higher-kinded-type-slots)
  for the surface form and per-call generativity.
- `Any` — the no-op fast-path.

[`KType::matches_value`](../../../src/machine/model/types/ktype_predicates.rs) plus
[`KObject::ktype`](../../../src/machine/model/values/kobject.rs) close the loop on runtime
checking: every value has a queryable type, and any declared type can be checked
against it.


## In depth

The variant catalog above is the foundation; these pages cover the rest:

- [Parameterization, variance, and runtime carriers](parameterization-and-variance.md)
- [Slot kinds and function signatures](slots-and-signatures.md)
- [Dispatch and slot specificity](dispatch.md)
- [Record fields, hashing, and limits](records-and-limits.md)
