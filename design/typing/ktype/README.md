# `KType` — the runtime type system

[`KType`](../../../src/machine/model/types/ktype.rs) is a `Copy` content-digest handle
— a bare `u128`, carrying no owned substructure. All type content lives in an interned
[`TypeNode`](../../../src/machine/model/types/node.rs) owned by the run frame's registry
([type-registry.md](../type-registry.md)); a handle names one node, and a node's child
positions are themselves `KType` handles (the composition edges *are* the content). Every
concrete `KObject` has a `TypeNode` variant:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List { element }`, `Dict { key, value }`,
  `KFunction { params: Record<KType>, ret }`. Each child position is a `KType` handle.
  Always parameterized; see
  [Container type parameterization](parameterization-and-variance.md#container-type-parameterization) below.
  `params` is a name-keyed [parameter `Record<KType>`](records-and-limits.md#record-fields-and-ktype-hashing),
  so a function-typed slot's identity is its parameters by name and type
  (order-blind). `KFunction` is the *only* function-type node: a functor — a
  module-returning function — reports it too, so it is admissible wherever a
  same-shape `:(FN …)` slot matches (see [functors.md](../functors.md)). When a
  function's source return is
  per-call-elaborated, its `ret` handle names a `DeferredReturn(DeferredReturnSurface)`
  node — see [Record fields and `KType` hashing](records-and-limits.md#record-fields-and-ktype-hashing).
- Structural record: `Record { fields: Record<KType> }` — a `BinderSymbol`-keyed field schema
  (`:{x :Number, y :Str}`; a field name may also be a capitalized `Type` token, `:{Ty :Signature}`),
  distinct from a nominal `NewType`-kind `SetMember`
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
  `KKind::admits` is reflexive subsumption: `Any` is the lattice top and admits every type
  value — a signature is a type value, so `:Type` takes it — while `Proper` admits the
  proper subtree only (the signature wall lives at the `Proper` tier: a proper-type slot
  names what can type an ordinary value, which a signature is not). `KKind::strictly_below`
  orders specificity, so an `OfKind(Newtype)` slot out-specifies an `OfKind(Proper)` sibling
  and an `OfKind(Signature)` slot out-specifies an `OfKind(Any)` one. See
  [Type-position slot kinds](slots-and-signatures.md#type-position-slot-kinds).
- First-class type values: a type flows raw as a `&KType` in the value channel's `Type`
  arm — there is no `KObject` box. As a parameter-slot annotation, `:Type`'s `OfKind(Any)`
  admits any type value — bare builtin type tokens (`Number`, `Str`, `Bool`, `Null`), newtype
  and union nominal tokens, an anonymous `Union` type value, and a signature value — while
  `OfKind(Proper)` admits the same set minus signatures. A module value (riding the value
  channel's Object arm as `KObject::Module`) routes through a `Signature { .. }` slot, never
  an `OfKind` slot, so the `:Type` vs `:Module` overload distinction stays intact — see
  [`KType::accepts_part`](../../../src/machine/model/types/ktype_predicates.rs)
  and the pin test
  [`type_slot_admits_bare_builtin_tokens_and_user_type_carriers`](../../../src/machine/model/types/ktype_predicates/tests.rs).
- User-declared nominal types — three node kinds carry the recursive-group model,
  in which a member's identity unit is its strongly-connected component, not its
  declaration group (a non-recursive type is a singleton component).
  See [user-types.md](../user-types.md) for the full model.
  - `SetMember { scc_digest, index, scc_size, name, kind, schema }` — one sealed
    member. Its handle is the `Copy` `(scc_digest, index)` folded into one digest —
    the nominal identity `KObject::ktype()` reports for `Wrapped` carriers and
    held by `bindings.types`. Identity is the SCC digest plus the
    member index, never the (possibly cyclic) schema; `scc_size`, `name`, `kind`,
    and `schema` are digest-excluded because they are exactly the inputs the digest
    was computed over. Structurally identical declarations therefore unify — the
    same `NEWTYPE` elaborated twice denotes one type. The member's `kind` is one of
    the nominal families `KKind::{Newtype, TypeConstructor}` — `kind_of` reads it to
    classify the nominal type value. A user `UNION` seals one `NewType` member per
    variant; the union name binds the anonymous `Union` of those member handles. A
    sibling reference inside a sealed schema is the sibling's own absolute member
    handle — a cyclic composition edge the registry holds without refcounting.
  - `Sibling(index)` — a **relative** sibling reference inside a pre-seal group
    window, a bare index meaningful only against the ambient window. Ordinary
    interned content, but it never appears in a sealed schema, never reaches the
    predicates, and never rides a value; the seal rewrites each one to an absolute
    member handle.
  A slot that wants "any user-declared type of family X" is an `OfKind(KKind)`
  carrying the nominal family (`OfKind(Newtype)` / `OfKind(TypeConstructor)`).
  Because `OfKind` is type-channel-only, such a slot
  admits the *type value* of that family, not a runtime instance — a builtin that
  dispatches on a runtime representation (ATTR's record field access) takes the
  least-specific `Any` slot and validates the value's shape in its body
  (`access_field`), never matching the value by a kind. The nominal-family surface
  keywords (`Newtype` / `TypeConstructor`) are pinned for diagnostic
  rendering only — none is registered as a writable surface name (no entry in
  [the declared builtin names](../../../src/machine/model/types/builtin_names.rs)
  `KType::from_symbol` reads).
- `Union { members: Vec<KType> }` — an **untagged structural disjunction**, the type `:(A | B)`.
  Not a member reference: it composes any member types, canonicalized by
  [`TypeRegistry::union_of`](../../../src/machine/model/types/registry.rs) —
  flattened, deduplicated, and collapsed to the lone member when only one survives
  (`:(A | A)` is `:A`). Identity is order-blind: the digest sorts its member
  handles, so `:(A | B)` equals `:(B | A)`. A union admits any value one of its members admits, and
  each member is strictly more specific than the union
  ([`is_more_specific_than`](../../../src/machine/model/types/ktype_predicates.rs)), so a
  union-typed slot dispatches by the value's own runtime type. `kind_of` reports
  `Proper`. A user `UNION` binds the anonymous union of its per-variant `NewType`
  member handles. See [user-types.md § Unions dissolve into per-variant newtypes](../user-types.md#unions-dissolve-into-per-variant-newtypes).
- Module / signature carriers (the [module system](../modules.md) rests on
  these): **there is no module variant.** A module is a value — it rides the value
  channel's Object arm as `KObject::Module`, and its `ktype()` is its principal
  signature, so the type channel names a module only through
  the self-sig `Signature` type it seals at creation. A module name is a value token and types
  nothing on its own; `TYPE OF` is the door that surfaces that self-sig as a type
  value (`m :(TYPE OF int_ord)`, `-> :(TYPE OF er)`) — see
  [modules.md § Modules in type position](../modules.md#modules-in-type-position-type-of).
  `Signature { schema: SigSchema, schema_digest }`
  serves both signature roles in one node. The node carries **no binder and no label**:
  a `SIG`-declared interface, a module's self-sig, and the empty `:Module` top are one
  shape differing only in schema, so two textually identical `SIG` declarations are one
  type. `schema_digest` is the content digest of the schema, computed once at
  construction; the node is both the introspectable value *and* the dispatch constraint
  ("any module satisfying this signature"). `name()` is content-derived — `"Module"`
  when the schema is empty, else the structural `SIG (member: Type, …)` in member-name
  order — so no per-declaration path is stored to go stale.
  `AbstractType { source: ScopeId, name: TypeSymbol, param_names: Vec<TypeSymbol>, nonce: Option<ScopeId> }`
  is the per-abstract-type-member node — owned data, id-keyed. `param_names` carries the
  member's order: empty is a first-order proper type (`TYPE Elt`), non-empty a type
  constructor over those named parameters (`TYPE (Elem AS Wrap)`), and `kind_of` reads
  the list to classify the member `ProperType` or `TypeConstructor`. `source` is the
  binder the member is named against; `nonce` is the generativity mechanism — `None` for
  a SIG-body declaration, `Some(<per-application module scope id>)` for the mint `:|`
  opaque ascription produces (`view.Carrier`), so two opaque ascriptions of one SIG never
  unify. **All four fields are identity; nothing on the node is digest-excluded** —
  `param_names` feeds kind classification and `source` feeds member substitution, so both
  are functional reads and interning must not collapse across them. A SIG-own member's
  `source` is canonicalized to `ScopeId::SENTINEL` in the stored schema, so two textually
  identical SIG declarations project to one schema; the substitution walks test against
  that constant binder.
  Projecting a member off a bare type-channel `AbstractType` is an error: the
  identity names no receiver, and further members project off the module value
  ([`attr.rs`](../../../src/builtins/attr.rs)).
  The companion wildcard `OfKind(Signature)` admits any signature value; the
  surface keyword `Signature` lowers to it in
  [`KType::from_symbol`](../../../src/machine/model/types/ktype_resolution.rs),
  while `Module` lowers to the empty signature, the module-lattice top every module value
  satisfies.
  The single `Signature` node is **disambiguated by position**: a
  `Signature { .. }` *slot* matches a *module value* (on the value channel's Object
  arm) whose self-sig structurally
  satisfies the slot's schema (the constraint role — what `er :Ordered`
  lowers to in an FN parameter slot, so `:Ordered` means "module
  satisfying Ordered," never "the signature value itself"), while a
  signature *value* (a `Signature` handle flowing in the `Type` arm) is matched only
  by the `OfKind(Signature)` wildcard. A `WITH` specialization folds its pins into the
  schema as manifest members before interning, so a `WITH` result is an ordinary
  concrete-membered signature, introspectable like any other.
- Higher-kinded application: `ConstructorApply { constructor, arguments: Record<KType> }`
  — structural identity by `(constructor, arguments)`, mirror of `List` / `Dict`,
  with `Record`'s order-blind identity. `arguments` maps each of the
  constructor's parameter names to the elaborated argument type. `constructor` is a
  `TypeConstructor`-kind handle — a `SetMember` of a declared family, or an
  `AbstractType` naming a SIG's abstract
  constructor slot. Emitted when a constructor identity is applied to a record of
  named type arguments (`:(Wrap {Elem = Number})`) or through the arity-1 `AS`
  sugar; renders as `:(ctor {Name = Type, …})` in diagnostics, which re-parses. See
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
