# Record substrate for identifier-keyed binding

A single ordered identifier-keyed map shape, with order-blind equality and
a name+type hash, shared by every construct that binds parameters by name.

**Problem.** koan has three runtime name→value maps with divergent Rust
representations and one lossy type-level encoding, so there is no single
place to define equality or hashing for "a record of identifier-keyed
fields." `Bindings.data` is
[`HashMap<String, (&KObject, BindingIndex)>`](../../src/machine/core/bindings.rs),
`Struct.fields` is
[`Rc<IndexMap<String, KObject>>`](../../src/machine/model/values/kobject.rs),
and dispatch's resolved arguments land in
[`ArgumentBundle { args: HashMap<String, Rc<KObject>> }`](../../src/machine/core/kfunction/argument_bundle.rs).
At the type level the divergence is sharper: `UserTypeKind::Struct` already
carries `Rc<Vec<(String, KType)>>`
([ktype.rs](../../src/machine/model/types/ktype.rs)) — an ordered
`(name, type)` record — while `KType::KFunction { args: Vec<KType> }` and
`KType::KFunctor { params: Vec<KType> }` discard the parameter names the
[signature](../../src/machine/model/types/signature.rs)'s `Argument.name`
slots already hold. The dict carrier (`KType::Dict(K, V)`, `KObject::Dict`)
is a different shape: arbitrary value keys, one homogeneous value type.

**Impact.**

- A single `Record` shape — an ordered identifier-keyed map — backs the
  struct schema, FN/FUNCTOR parameter types, and the runtime binding
  carriers, so equality and hashing are defined once.
- Field order is preserved for rendering and iteration while equality
  ignores it: `(x :Number, y :Str)` and `(y :Str, x :Number)` are the same
  record type.
- A name+type hash makes a `Record` type usable directly as a dispatch /
  memo key.
- The dict carrier stays a sibling: records restrict keys to identifiers
  and admit heterogeneous per-field types; dicts admit arbitrary value keys
  and one homogeneous value type. The two never share a key representation,
  so records keep cheap `String`/identifier keys off the dispatch hot path.

**Directions.**

- *Record shape — decided.* An ordered identifier-keyed map, generic over
  its value (`KType` for type-level records; the runtime value type for
  value-level records). Storage preserves insertion/declaration order
  (`IndexMap`-shaped); equality is order-blind (same set of `(name, type)`
  pairs, names unique within a record).
- *Hash — decided.* A commutative fold over per-field
  `mix(hash(name), hash(type))` using wrapping addition (not XOR, which
  cancels on a duplicate). Order-blind by construction, so it agrees with
  order-blind equality.
- *Record vs. dict — decided: siblings.* Neither is built on the other.
  Records use `String`/identifier keys with heterogeneous field types; the
  dict carrier keeps `Box<dyn Serializable>` keys with one value type. The
  fully-general map (arbitrary keys *and* heterogeneous values) is never
  materialized.
- *Concrete value-carrier merge — deferred.* Whether `Struct.fields`,
  `ArgumentBundle`, and `Bindings.data` adopt one Rust container is the
  scope of [argument-binding unification](argument-binding-unification.md);
  this item defines the shape and its equality/hash, not the carrier
  consolidation.

## Dependencies

**Requires:**

None — this is the foundation the rest of the record work builds on.

**Unblocks:**

- [FN/FUNCTOR named identity](fn-named-identity.md) — names round-trip into
  `KType::KFunction` / `KFunctor` against this shape's equality.
- [Record structural subtyping and projection](record-subtyping.md) —
  width/depth admission is defined over this shape.
- [Argument-binding unification](argument-binding-unification.md) —
  consolidates the runtime carriers onto this shape.
