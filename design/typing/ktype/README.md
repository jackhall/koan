# `KType` — the runtime type system

[`KType`](../../../src/machine/model/types/ktype.rs) has a variant for every concrete `KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List(Box<KType>)`, `Dict(Box<KType>, Box<KType>)`,
  `KFunction { params: Record<KType>, ret: Box<KType> }`. Always parameterized; see
  [Container type parameterization](parameterization-and-variance.md#container-type-parameterization) below.
  `params` is a name-keyed [parameter `Record<KType>`](records-and-limits.md#record-fields-and-ktype-hashing),
  so a function-typed slot's identity is its parameters by name and type
  (order-blind). The sibling
  `KFunctor { params: Record<KType>, ret, body: Option<&KFunction> }` shares the
  storage and identity rules; the variant tag keeps the two families admissibly
  disjoint (see [functors.md](../functors.md)). The `body` is an **identity-inert**
  carrier — `None` for the `:(FUNCTOR …)` type annotation (just a shape), `Some(f)`
  for a bound functor value whose callable rides the type-table identity so a later
  `:(F {…})` / `F {…}` application can invoke it. `body` is excluded from equality,
  hashing, admissibility, join, and rendering, all of which compare `params` + `ret`
  only, so two structurally-identical functor types compare and hash equal
  regardless of body. When a function's source return is
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
  subsumption lattice, `Any > {Module, Signature, Proper > {Newtype, TypeConstructor}}`:
  a parsed type-name slot is `OfKind(Proper)`, the `:Type` surface is `OfKind(Any)`, the
  `:Signature` wildcard is `OfKind(Signature)` (`:Module` instead lowers to the empty
  signature — see the module / signature carriers below), and the two
  nominal families sit strictly below `Proper`. `KKind::admits` is reflexive subsumption (a
  `Proper` / `Any` slot admits any proper-subtree type value, while the module/signature wall
  keeps each of those families to itself); `KKind::strictly_below` orders specificity, so an
  `OfKind(Newtype)` slot out-specifies an `OfKind(Proper)` sibling. See
  [Type-position slot kinds](slots-and-signatures.md#type-position-slot-kinds).
- First-class type values: a type flows raw as a `&KType` in the value channel's `Type`
  arm — there is no `KObject` box. As a parameter-slot annotation, `OfKind(Proper)` (`:Type`'s
  `OfKind(Any)` likewise) admits any *proper* type value: bare builtin type tokens (`Number`,
  `Str`, `Bool`, `Null`), newtype and union nominal tokens, an anonymous `Union` type value, and
  any other non-module / non-signature type. A signature value routes through the dedicated
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
    per-declaration identity synthesized by `KObject::ktype()` for `Wrapped` and
    `Tagged` carriers and held by `bindings.types`. Identity is
    `(Rc::as_ptr(set), index)` — never the schema, which may be cyclic. Two
    distinct nominals sit at distinct `(set ptr, index)` pairs, giving the
    per-declaration-distinctness dispatch keys on. The member's `kind` (read via
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
    bound by a `RECURSIVE TYPES` group name. Identity is the set pointer
    (`Rc::ptr_eq`); inert in value dispatch.
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
  (`:(A | A)` is `:A`). Identity is order-blind: `PartialEq` / `Hash` are set-based, so
  `:(A | B)` equals `:(B | A)`. A union admits any value one of its members admits, and
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
  these): `Module { module: &'a Module<'a>, frame: Option<Rc<FrameStorage>> }`
  is a module's **type-position** identity — the region-pinned `&Module`
  pointer plus the per-call frame anchor for functor-built modules — held by
  `bindings.types` and read during type-path elaboration; a module *value* rides
  the value channel's Object arm as `KObject::Module`, typed by its self-sig.
  `Signature { sig: SigSource<'a>, pinned_slots: Vec<(String, KType)> }`
  serves both signature roles in one variant — its `sig` names one of three
  module-lattice points ([`SigSource`](../../../src/machine/model/types/ktype.rs):
  `Declared` SIG, `SelfOf` module self-sig, `Empty`) — the introspectable value
  (a `Declared`, carrying `decl_scope` via `sig`) *and* the dispatch constraint ("any
  module satisfying this signature"); `AbstractType { source:
  AbstractSource<'a>, name: String }` is the per-abstract-type-member tag.
  `AbstractSource` is `Sig(ScopeId) | Module(&'a Module<'a>)`: a
  `Sig`-rooted member is named at SIG-declaration time (a SIG-local
  `LET Type = ...` that would otherwise collapse to its underlying type binds
  this name-bearing tag instead), while a `Module`-rooted member is the per-call
  mint `:|` opaque ascription produces (`Foo.Type`, with a module to project
  further members off). Manual `PartialEq` keys identity on
  `module.scope_id()` for `KType::Module`, `sig.sig_id()` + `pinned_slots`
  for `KType::Signature` (the `SigSource`'s `path()` is diagnostic-only), and
  `(source.scope_id(), name)` for `KType::AbstractType` — so two
  opaque ascriptions of the same source module produce distinct
  `KType::Module` identities (the abstraction barrier) but their
  `AbstractType` minting for the same slot name compares equal, and a
  per-call `Module`-rooted mint stays distinct from the `Sig`-rooted member it
  was threaded from.
  The companion wildcard `OfKind(Signature)` admits any signature value; the
  surface keyword `Signature` lowers to it in
  [`KType::from_name`](../../../src/machine/model/types/ktype_resolution.rs),
  while `Module` lowers to the empty signature (`Signature { SigSource::Empty }`),
  the module-lattice top every module value satisfies.
  The single `Signature` variant is **disambiguated by position**: a
  `Signature { .. }` *slot* matches a *module value* (on the value channel's Object
  arm) whose self-sig structurally
  satisfies `sig` (the constraint role — what `Er :OrderedSig`
  lowers to in a FUNCTOR parameter slot, so `:OrderedSig` means "module
  satisfying OrderedSig," never "the signature value itself"), while a
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
