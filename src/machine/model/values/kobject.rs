use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::KFunction;
use crate::machine::core::{FrameSet, KoanRegion, Residence};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{KType, Parseable, Record, TypeNode, TypeRegistry};

use super::{Held, KKey, Module};

mod equality;
pub use equality::ValueEqualityError;

#[cfg(test)]
mod tests;

/// An [`Rc`]-shared [`KObject::Wrapped`] payload. Two constructors record the wrapper's intent:
/// [`Self::peel`] collapses one `Wrapped` layer (a re-tag replaces the value's identity, so
/// identities never stack), while [`Self::hold`] preserves the value as-is (genuine
/// construction — a union variant nesting another variant carries a `Wrapped` payload, so
/// `Succ(Zero(null))` keeps both layers). The payload rides an `Rc` (not a region `&'a`
/// reference) so a `Wrapped` carrier lifts across a dying frame by `Rc::clone` — the
/// lift-stability a carrier needs to outlive the frame it was born in.
#[derive(Clone)]
pub struct WrappedPayload<'a>(Rc<KObject<'a>>);

impl<'a> WrappedPayload<'a> {
    /// Wrap `value` for a **re-tag**, collapsing one `Wrapped` layer: a `Wrapped` shares its
    /// inner `Rc` (the new identity replaces the old), anything else is `Rc`-boxed via an
    /// independent `deep_clone`.
    pub fn peel(value: &KObject<'a>) -> Self {
        match value {
            KObject::Wrapped { inner, .. } => inner.clone(),
            _ => Self(Rc::new(value.deep_clone())),
        }
    }

    /// Wrap `value` for a **construction**, preserving it verbatim — including a nested
    /// `Wrapped` payload, so a union variant over another variant keeps every layer.
    pub fn hold(value: &KObject<'a>) -> Self {
        Self(Rc::new(value.deep_clone()))
    }

    pub fn get(&self) -> &KObject<'a> {
        &self.0
    }
}

/// Runtime value: the universal type that `KFunction`s consume and produce.
///
/// Composite payloads are `Rc`-shared under an immutable-value contract; a future
/// mutable-list builtin would need `Rc::make_mut` at the mutation site. `Struct.fields`
/// uses `IndexMap` so iteration matches declaration order.
///
/// A `KFunction` is a bare borrow into its defining region; the regions an escaping
/// closure reaches are pinned by its carrier's witness set ([`FrameSet`](crate::machine::FrameSet)),
/// not a per-value anchor. See [per-call-region/lifecycle.md § Carriers](../../../../design/per-call-region/lifecycle.md#carriers).
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    /// List value. The second field is the value's **own type handle** — the interned
    /// `List<element>` — memoized at construction from the join (LUB) of the contents under
    /// the immutable-`Rc` contract, or re-stamped at an annotated boundary to the declared
    /// list type (coarsening included). Construct via [`KObject::list`] /
    /// [`KObject::list_with_type`]; never the tuple directly outside this module.
    List(Rc<Vec<Held<'a>>>, KType),
    /// Dict value. Each value cell is a [`Held`] (an object or a first-class type); keys
    /// are the concrete scalar [`KKey`]. The second field is the value's own type handle —
    /// the interned `Dict<key, value>` over the join of the keys / values, or the declared
    /// dict type after a stamp.
    Dict(Rc<HashMap<KKey, Held<'a>>>, KType),
    KExpression(KExpression<'a>),
    KFunction(&'a KFunction<'a>),
    /// Tagged-union value. `identity` is the value's own type handle: the union member's
    /// `SetMember` handle when the carrier's type arguments are erased, or the
    /// `ConstructorApply` over that member when an ascription stamped a parameterized union's
    /// arguments in. One handle carries what the member reference and the runtime type
    /// arguments used to carry separately, so `ktype()` is a copy and identity comparison is
    /// one `u128`.
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
        identity: KType,
    },
    /// Anonymous structural record value (`{x = 1, y = "a"}`). The first field is the
    /// `Rc`-shared field record (identifier-keyed, declaration-ordered, order-blind
    /// equality); the second is the value's own type handle — the interned `Record` over
    /// each field's `ktype()` at fresh construction, re-stamped to a declared record type at
    /// an annotated boundary (mirrors `List` / `Dict`). Construct via [`KObject::record`] /
    /// [`KObject::record_with_type`]. Distinct from the nominal `Struct`: a record carries no
    /// `(name, scope_id)` identity, only its structure. Each field value is a [`Held`] (an
    /// object or a first-class type).
    Record(Rc<Record<Held<'a>>>, KType),
    /// NEWTYPE / union-variant carrier (and the ATTR abstract-type re-tag carrier): tags a
    /// representation value with a type identity. A re-tag collapses one wrapper layer
    /// ([`WrappedPayload::peel`]); a genuine construction preserves the payload verbatim
    /// ([`WrappedPayload::hold`]), so a union variant nesting another variant keeps every
    /// layer. `type_id` is the declaration-stable identity handle — for a NEWTYPE / union
    /// variant the sealed member's `SetMember` handle, for an identity-wrapper construction a
    /// `ConstructorApply` over it, and for an opaque-ascription abstract-type re-tag the
    /// per-call `AbstractType` identity.
    ///
    /// `ktype()` copies `type_id` — the per-declaration identity. ATTR over a `Wrapped` falls
    /// through to `inner`, so wrapping a struct in a NEWTYPE doesn't force every field accessor
    /// to redo.
    Wrapped {
        inner: WrappedPayload<'a>,
        type_id: KType,
    },
    /// First-class module value. A bare borrow into the region the module was minted in,
    /// pinned by the value carrier's witness set — the same contract as [`Self::KFunction`].
    /// `ktype()` reports the module's principal signature (the handle its self-sig seal
    /// interned), so a module in expression position dispatches and satisfies signature slots
    /// on the value channel.
    Module(&'a Module<'a>),
    Null,
}

impl<'a> KObject<'a> {
    /// Fresh `List` carrier: memoizes the element type as the join (LUB) of contents.
    /// Empty list memoizes `Any` (the join's identity); the empty-container *error*
    /// rule lives at the untyped-resolution boundary, not here.
    pub fn list(items: Vec<KObject<'a>>, types: &TypeRegistry) -> KObject<'a> {
        KObject::list_of_held(items.into_iter().map(Held::Object).collect(), types)
    }

    /// Fresh `List` carrier over [`Held`] cells — the type-aware path (a list element may be
    /// a first-class type). Memoizes the element type as the join (LUB) of the cells.
    pub fn list_of_held(items: Vec<Held<'a>>, types: &TypeRegistry) -> KObject<'a> {
        let element = types.join_iter(items.iter().map(|i| i.ktype(types)));
        KObject::List(Rc::new(items), types.list(element))
    }

    /// `List` carrier with an explicitly supplied **list type** — for lift (preserve the
    /// memoized type across a region-anchor rebuild) and ascription stamping (re-tag to
    /// the declared list type, coarsening included). `list_type` is the whole `List<element>`
    /// handle, already interned, not the element type.
    pub fn list_with_type(items: Rc<Vec<Held<'a>>>, list_type: KType) -> KObject<'a> {
        KObject::List(items, list_type)
    }

    /// Fresh `Dict` carrier: memoizes key + value types as the join of the keys / values.
    pub fn dict(map: HashMap<KKey, KObject<'a>>, types: &TypeRegistry) -> KObject<'a> {
        KObject::dict_of_held(
            map.into_iter().map(|(k, v)| (k, Held::Object(v))).collect(),
            types,
        )
    }

    /// Fresh `Dict` carrier over [`Held`] value cells — the type-aware path (a dict value
    /// may be a first-class type; keys stay scalar).
    pub fn dict_of_held(map: HashMap<KKey, Held<'a>>, types: &TypeRegistry) -> KObject<'a> {
        let key = types.join_iter(map.keys().map(|k| k.ktype()));
        let value = types.join_iter(map.values().map(|v| v.ktype(types)));
        KObject::Dict(Rc::new(map), types.dict(key, value))
    }

    /// `Dict` carrier with an explicitly supplied **dict type** — the whole `Dict<key, value>`
    /// handle. See [`Self::list_with_type`].
    pub fn dict_with_type(map: Rc<HashMap<KKey, Held<'a>>>, dict_type: KType) -> KObject<'a> {
        KObject::Dict(map, dict_type)
    }

    /// Fresh `Record` carrier: memoizes the per-field type record as each field's
    /// `ktype()`. Field order follows declaration; equality is order-blind per the
    /// `Record` substrate.
    pub fn record(fields: Record<KObject<'a>>, types: &TypeRegistry) -> KObject<'a> {
        KObject::record_of_held(
            Record::from_pairs(fields.into_pairs().map(|(k, v)| (k, Held::Object(v)))),
            types,
        )
    }

    /// Fresh `Record` carrier over [`Held`] field cells — the type-aware path (a field
    /// value may be a first-class type).
    pub fn record_of_held(fields: Record<Held<'a>>, types: &TypeRegistry) -> KObject<'a> {
        let field_types = fields.map(|v| v.ktype(types));
        KObject::Record(Rc::new(fields), types.record(field_types))
    }

    /// `Record` carrier with an explicitly supplied **record type** — the whole interned
    /// record-type handle — for ascription stamping (re-tag to the declared field types,
    /// coarsening included). See [`Self::list_with_type`].
    pub fn record_with_type(fields: Rc<Record<Held<'a>>>, record_type: KType) -> KObject<'a> {
        KObject::Record(fields, record_type)
    }

    /// Ascription stamping at an annotated boundary (FN return type, argument slot,
    /// LET ascription). Callers have already checked the value satisfies `declared`;
    /// this re-tags the carrier to *exactly* the declared parameter types — a
    /// `List<Number>` returned through `:(LIST OF Any)` re-tags to `List<Any>`, so
    /// downstream dispatch sees the contract rather than the implementation's
    /// incidental precision.
    ///
    /// Only the four parameterized carriers re-tag, and each re-tags to `declared` itself —
    /// the declared type IS the carrier's new identity handle. Every other shape passes through
    /// (its `ktype()` is already its nominal identity). For a `Tagged` stamped against a
    /// `ConstructorApply`, the constructor identity must already match, so adopting `declared`
    /// wholesale supplies exactly the declared arguments.
    pub fn stamp_type(self, declared: KType, types: &TypeRegistry) -> KObject<'a> {
        match (self, types.node(declared)) {
            (KObject::List(items, _), TypeNode::List { .. }) => KObject::List(items, declared),
            (KObject::Dict(map, _), TypeNode::Dict { .. }) => KObject::Dict(map, declared),
            (KObject::Record(fields, _), TypeNode::Record { .. }) => {
                KObject::Record(fields, declared)
            }
            (KObject::Tagged { tag, value, .. }, TypeNode::ConstructorApply { .. }) => {
                KObject::Tagged {
                    tag,
                    value,
                    identity: declared,
                }
            }
            (other, _) => other,
        }
    }

    /// True iff this is an empty container carrying no usable element-type information —
    /// an empty `List` whose memoized element type is `Any`, or an empty `Dict` whose
    /// key and value types are both `Any`. Reaching an *untyped* resolution boundary
    /// (untyped `LET` binding, bare top-level expression result) with this shape is an
    /// error (see [ktype/parameterization-and-variance.md § Runtime type-parameter carriers](../../../../design/typing/ktype/parameterization-and-variance.md#runtime-type-parameter-carriers)).
    ///
    /// A stamped empty container is not flagged (its carrier carries a non-`Any`
    /// element type), nor is a non-empty heterogeneous literal `List<Any>` (it carries
    /// information and is legal where `:(LIST OF Any)` is declared).
    pub fn is_unstamped_empty_container(&self) -> bool {
        match self {
            KObject::List(items, list_type) => items.is_empty() && *list_type == KType::LIST_OF_ANY,
            KObject::Dict(map, dict_type) => map.is_empty() && *dict_type == KType::DICT_ANY_ANY,
            _ => false,
        }
    }

    /// Whether this is a **shallow scalar** — a fully-owned leaf (`Number`, `KString`, `Bool`,
    /// `Null`) whose representation embeds no `&'a` region borrow and no [`Held`] cell. Such a value
    /// cannot reference any dep the construction fold was handed, so the dep-witness union is pure
    /// over-retention: the combinator gate ([`alloc_object_scalar`](crate::machine::core::StepAllocator::alloc_object_scalar))
    /// routes it to the no-fold path so it seals with an empty reach. Every other variant borrows
    /// (`KFunction`, `Module`) or holds cells that transitively might (`List`/`Dict`/`Record`/
    /// `Tagged`/`Wrapped`/`KExpression`), so it keeps the fold.
    pub fn is_shallow_scalar(&self) -> bool {
        matches!(
            self,
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null
        )
    }

    /// True when every region borrow in `self` points into `dest`. Only value-channel borrows
    /// are walked: `KFunction`, `Module`, `KExpression` splices, and the [`Held`] cells of the
    /// composite carriers. The `KType` tags (`List`/`Dict`/`Record` memos, `Tagged { identity }`,
    /// `Wrapped { type_id }`) are not walked — a handle is one `u128` naming registry-owned
    /// content, so it borrows no region at all.
    pub(crate) fn resident_in(&self, dest: &KoanRegion) -> bool {
        self.resident_in_visiting(&Residence::dest_only(dest))
    }

    /// The evidence-widened twin of [`Self::resident_in`], for a value built from (or embedding a
    /// projection of) one or more delivered carriers: every walked borrow must point into
    /// `dest` or be covered by one of `sets` — the object delivered tier's coverage predicate,
    /// over the same borrows as [`Self::resident_in`]. The `StoredReach` tokens holding the
    /// reach are opaque to this layer; core extracts the sets before calling.
    pub(crate) fn resident_in_delivered(&self, dest: &KoanRegion, sets: &[&FrameSet]) -> bool {
        self.resident_in_visiting(&Residence::with_reach(dest, sets))
    }

    pub(crate) fn resident_in_visiting(&self, residence: &Residence<'_>) -> bool {
        match self {
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => true,
            KObject::KFunction(f) => residence.owns_function(f),
            KObject::KExpression(e) => e.is_splice_free(),
            KObject::List(items, _) => items.iter().all(|h| held_resident_in(h, residence)),
            KObject::Dict(map, _) => map.values().all(|h| held_resident_in(h, residence)),
            KObject::Record(fields, _) => {
                fields.iter().all(|(_, h)| held_resident_in(h, residence))
            }
            KObject::Tagged { value, .. } => value.resident_in_visiting(residence),
            KObject::Wrapped { inner, .. } => inner.get().resident_in_visiting(residence),
            KObject::Module(m) => residence.owns_module(m),
        }
    }

    /// Runtime type tag — context-free by construction (ruling 4). Every value memoizes its
    /// full interned type handle where it is built, at a site that holds the registry, so this
    /// only ever copies a stored handle or names a pre-seeded constant. It builds nothing and
    /// needs no registry.
    pub fn ktype(&self) -> KType {
        match self {
            KObject::Number(_) => KType::NUMBER,
            KObject::KString(_) => KType::STR,
            KObject::Bool(_) => KType::BOOL,
            KObject::Null => KType::NULL,
            KObject::KExpression(_) => KType::KEXPRESSION,
            KObject::List(_, list_type) => *list_type,
            KObject::Dict(_, dict_type) => *dict_type,
            KObject::Record(_, record_type) => *record_type,
            KObject::KFunction(f) => f.value_ktype(),
            KObject::Tagged { identity, .. } => *identity,
            KObject::Wrapped { type_id, .. } => *type_id,
            KObject::Module(m) => m.ktype(),
        }
    }

    /// Independent-but-cheap clone: composite payloads `Rc::clone` under the
    /// immutable-value contract; a `KFunction` copies its bare defining-region borrow.
    pub fn deep_clone(&self) -> KObject<'a> {
        match self {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            KObject::List(items, list_type) => KObject::List(Rc::clone(items), *list_type),
            KObject::Dict(entries, dict_type) => KObject::Dict(Rc::clone(entries), *dict_type),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFunction(f) => KObject::KFunction(f),
            KObject::Tagged {
                tag,
                value,
                identity,
            } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
                identity: *identity,
            },
            KObject::Record(fields, record_type) => {
                KObject::Record(Rc::clone(fields), *record_type)
            }
            KObject::Wrapped { inner, type_id } => KObject::Wrapped {
                inner: inner.clone(),
                type_id: *type_id,
            },
            KObject::Module(m) => KObject::Module(m),
        }
    }

    pub fn as_kexpression(&self) -> Option<&KExpression<'a>> {
        match self {
            KObject::KExpression(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<&'a KFunction<'a>> {
        match self {
            KObject::KFunction(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_module(&self) -> Option<&'a Module<'a>> {
        match self {
            KObject::Module(m) => Some(*m),
            _ => None,
        }
    }
}

/// [`KObject::resident_in`]/[`KObject::resident_in_delivered`]'s cell-wise dispatch for a
/// [`Held`] value — an object recurses structurally, a type is owned data and borrows nothing.
fn held_resident_in(h: &Held<'_>, residence: &Residence<'_>) -> bool {
    match h {
        Held::Object(o) => o.resident_in_visiting(residence),
        Held::Type(_) | Held::UnresolvedType(_) => true,
    }
}

impl<'a> Parseable for KObject<'a> {
    fn ktype(&self) -> KType {
        KObject::ktype(self)
    }
}

impl<'a> KObject<'a> {
    /// Canonical surface rendering of a value. Carried types render through the registry.
    pub fn summarize(&self, types: &TypeRegistry) -> String {
        match self {
            KObject::Number(n) => n.to_string(),
            KObject::KString(s) => s.clone(),
            KObject::Bool(b) => b.to_string(),
            KObject::List(items, _) => {
                let parts: Vec<String> = items.iter().map(|i| i.summarize(types)).collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(entries, _) => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize(types)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::KExpression(e) => e.summarize(),
            KObject::KFunction(f) => f.summarize(),
            KObject::Tagged { tag, value, .. } => {
                format!("{}({})", tag, value.summarize(types))
            }
            KObject::Record(fields, _) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{} = {}", field, value.summarize(types)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            KObject::Wrapped { inner, type_id } => {
                format!("{}({})", type_id.name(types), inner.get().summarize(types))
            }
            KObject::Module(m) => m.path.clone(),
        }
    }
}
