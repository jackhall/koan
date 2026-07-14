use std::collections::HashMap;
use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{FrameSet, KoanRegion, Residence};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    KType, Parseable, Record, RecursiveSet, Serializable, SigSource, SignatureElement,
};

use super::{Held, Module};

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
    /// List value. The second field is the memoized/ascribed element type: at fresh
    /// construction the join (LUB) of the contents under the immutable-`Rc` contract; at
    /// an annotated boundary re-stamped to the declared element type (coarsening
    /// included). Construct via [`KObject::list`] / [`KObject::list_with_type`]; never
    /// the tuple directly outside this module.
    List(Rc<Vec<Held<'a>>>, Box<KType<'a>>),
    /// Dict value. Each value cell is a [`Held`] (an object or a first-class type); keys
    /// stay scalar ([`Serializable`]). The second/third fields are the memoized/ascribed
    /// key + value types, computed as the join of the keys / values at fresh construction
    /// or re-stamped at an annotated boundary.
    Dict(
        Rc<HashMap<Box<dyn Serializable<'a> + 'a>, Held<'a>>>,
        Box<KType<'a>>,
        Box<KType<'a>>,
    ),
    KExpression(KExpression<'a>),
    KFunction(&'a KFunction<'a>),
    /// Tagged-union value. `(set, index)` references the union's sealed `RecursiveSet`
    /// member; `ktype()` synthesizes `KType::SetRef { set, index }` so dispatch on type
    /// identity sees the declared union, and lift shares the set by `Rc::clone`.
    ///
    /// `type_args` carries the value's runtime type arguments for a parameterized union
    /// (`Result<T, E>`): empty means erased; populated, `ktype()` synthesizes
    /// `KType::ConstructorApply` so dispatch and slot admission see the full
    /// instantiation. Populated by ascription stamping at annotated boundaries.
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
        set: Rc<RecursiveSet<'a>>,
        index: usize,
        type_args: Rc<Vec<KType<'a>>>,
    },
    /// Anonymous structural record value (`{x = 1, y = "a"}`). The first field is the
    /// `Rc`-shared field record (identifier-keyed, declaration-ordered, order-blind
    /// equality); the second is the memoized/ascribed per-field type record — the join
    /// of each field's `ktype()` at fresh construction, re-stamped to a declared
    /// `KType::Record` at an annotated boundary (mirrors `List` / `Dict`). Construct via
    /// [`KObject::record`] / [`KObject::record_with_type`]. Distinct from the nominal
    /// `Struct`: a record carries no `(name, scope_id)` identity, only its structure.
    /// Each field value is a [`Held`] (an object or a first-class type).
    Record(Rc<Record<Held<'a>>>, Box<Record<KType<'a>>>),
    /// NEWTYPE / union-variant carrier (and the ATTR abstract-type re-tag carrier): tags a
    /// representation value with a type identity. A re-tag collapses one wrapper layer
    /// ([`WrappedPayload::peel`]); a genuine construction preserves the payload verbatim
    /// ([`WrappedPayload::hold`]), so a union variant nesting another variant keeps every
    /// layer. `type_id` is a declaration-stable `&'a KType` — for a NEWTYPE / union variant it
    /// is a `KType::SetRef` into the sealed singleton set `bindings.types[name]` holds
    /// (so the shared `Rc<RecursiveSet>` travels with it); for an opaque-ascription
    /// abstract-type re-tag it is the per-call `AbstractType` identity.
    ///
    /// `ktype()` reports `(*type_id).clone()` — the per-declaration identity. ATTR over a
    /// `Wrapped` falls through to `inner`, so wrapping a struct in a NEWTYPE doesn't force
    /// every field accessor to redo.
    Wrapped {
        inner: WrappedPayload<'a>,
        type_id: &'a KType<'a>,
    },
    /// First-class module value. A bare borrow into the region the module was minted in,
    /// pinned by the value carrier's witness set — the same contract as [`Self::KFunction`].
    /// `ktype()` reports the module's principal signature (`Signature { SelfOf(m) }`), so a
    /// module in expression position dispatches and satisfies signature slots on the value
    /// channel.
    Module(&'a Module<'a>),
    Null,
}

impl<'a> KObject<'a> {
    /// Fresh `List` carrier: memoizes the element type as the join (LUB) of contents.
    /// Empty list memoizes `Any` (the join's identity); the empty-container *error*
    /// rule lives at the untyped-resolution boundary, not here.
    pub fn list(items: Vec<KObject<'a>>) -> KObject<'a> {
        KObject::list_of_held(items.into_iter().map(Held::Object).collect())
    }

    /// Fresh `List` carrier over [`Held`] cells — the type-aware path (a list element may be
    /// a first-class type). Memoizes the element type as the join (LUB) of the cells.
    pub fn list_of_held(items: Vec<Held<'a>>) -> KObject<'a> {
        let elem = KType::join_iter(items.iter().map(|i| i.ktype()));
        KObject::List(Rc::new(items), Box::new(elem))
    }

    /// `List` carrier with an explicitly supplied element type — for lift (preserve the
    /// memoized type across a region-anchor rebuild) and ascription stamping (re-tag to
    /// the declared element type, coarsening included).
    pub fn list_with_type(items: Rc<Vec<Held<'a>>>, elem: KType<'a>) -> KObject<'a> {
        KObject::List(items, Box::new(elem))
    }

    /// Fresh `Dict` carrier: memoizes key + value types as the join of the keys / values.
    pub fn dict(map: HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>>) -> KObject<'a> {
        KObject::dict_of_held(map.into_iter().map(|(k, v)| (k, Held::Object(v))).collect())
    }

    /// Fresh `Dict` carrier over [`Held`] value cells — the type-aware path (a dict value
    /// may be a first-class type; keys stay scalar).
    pub fn dict_of_held(map: HashMap<Box<dyn Serializable<'a> + 'a>, Held<'a>>) -> KObject<'a> {
        let k = KType::join_iter(map.keys().map(|k| k.ktype()));
        let v = KType::join_iter(map.values().map(|v| v.ktype()));
        KObject::Dict(Rc::new(map), Box::new(k), Box::new(v))
    }

    /// `Dict` carrier with explicitly supplied key + value types. See [`Self::list_with_type`].
    pub fn dict_with_type(
        map: Rc<HashMap<Box<dyn Serializable<'a> + 'a>, Held<'a>>>,
        key: KType<'a>,
        value: KType<'a>,
    ) -> KObject<'a> {
        KObject::Dict(map, Box::new(key), Box::new(value))
    }

    /// Fresh `Record` carrier: memoizes the per-field type record as each field's
    /// `ktype()`. Field order follows declaration; equality is order-blind per the
    /// `Record` substrate.
    pub fn record(fields: Record<KObject<'a>>) -> KObject<'a> {
        KObject::record_of_held(Record::from_pairs(
            fields.into_pairs().map(|(k, v)| (k, Held::Object(v))),
        ))
    }

    /// Fresh `Record` carrier over [`Held`] field cells — the type-aware path (a field
    /// value may be a first-class type).
    pub fn record_of_held(fields: Record<Held<'a>>) -> KObject<'a> {
        let types = fields.map(|v| v.ktype());
        KObject::Record(Rc::new(fields), Box::new(types))
    }

    /// `Record` carrier with an explicitly supplied per-field type record — for
    /// ascription stamping (re-tag to the declared field types, coarsening included).
    /// See [`Self::list_with_type`].
    pub fn record_with_type(fields: Rc<Record<Held<'a>>>, types: Record<KType<'a>>) -> KObject<'a> {
        KObject::Record(fields, Box::new(types))
    }

    /// Ascription stamping at an annotated boundary (FN return type, argument slot,
    /// LET ascription). Callers have already checked the value satisfies `declared`;
    /// this re-tags the carrier to *exactly* the declared parameter types — a
    /// `List<Number>` returned through `:(LIST OF Any)` re-tags to `List<Any>`, so
    /// downstream dispatch sees the contract rather than the implementation's
    /// incidental precision.
    ///
    /// Only the three parameterized carriers re-tag; every other shape passes through
    /// (its `ktype()` is already its nominal identity). For a `Tagged` stamped against
    /// a `ConstructorApply`, the constructor identity must already match; the
    /// `type_args` are replaced with the declared args.
    pub fn stamp_type(self, declared: &KType<'a>) -> KObject<'a> {
        match (self, declared) {
            (KObject::List(items, _), KType::List { element: elem, .. }) => {
                KObject::List(items, elem.clone())
            }
            (
                KObject::Dict(map, _, _),
                KType::Dict {
                    key: k, value: v, ..
                },
            ) => KObject::Dict(map, k.clone(), v.clone()),
            (
                KObject::Tagged {
                    tag,
                    value,
                    set,
                    index,
                    ..
                },
                KType::ConstructorApply { args, .. },
            ) => KObject::Tagged {
                tag,
                value,
                set,
                index,
                type_args: Rc::new(args.clone()),
            },
            (KObject::Record(fields, _), KType::Record { fields: types, .. }) => {
                KObject::Record(fields, types.clone())
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
            KObject::List(items, elem) => items.is_empty() && matches!(elem.as_ref(), KType::Any),
            KObject::Dict(map, k, v) => {
                map.is_empty()
                    && matches!(k.as_ref(), KType::Any)
                    && matches!(v.as_ref(), KType::Any)
            }
            _ => false,
        }
    }

    /// Whether this is a **shallow scalar** — a fully-owned leaf (`Number`, `KString`, `Bool`,
    /// `Null`) whose representation embeds no `&'a` region borrow and no [`Held`] cell. Such a value
    /// cannot reference any dep the construction fold was handed, so the dep-witness union is pure
    /// over-retention: the combinator gate ([`alloc_object_scalar`](crate::machine::core::StepAllocator::alloc_object_scalar))
    /// routes it to the no-fold path so it seals with an empty reach. Every other variant borrows
    /// (`KFunction`) or holds cells / memos that transitively might (`List`/`Dict`/`Record`/`Tagged`/
    /// `Wrapped`/`KExpression`), so it keeps the fold.
    pub fn is_shallow_scalar(&self) -> bool {
        matches!(
            self,
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null
        )
    }

    /// True when every answerable region borrow in `self` points into `dest` — the object twin
    /// of [`KType::resident_in`]. Honest-partial: `Wrapped { type_id }`'s `&KType` pointer is
    /// un-answerable (`KType` opts out of the residence side-table — see `arena.rs`'s `Stored`
    /// impls), so this walk is permissive there; every other borrow (`KFunction`, `KExpression`
    /// splices, and the region pointers nested in a memoized/carried `KType` tag) is checked.
    pub(crate) fn resident_in(&self, dest: &KoanRegion) -> bool {
        self.resident_in_visiting(&Residence::dest_only(dest))
    }

    /// The evidence-widened twin of [`Self::resident_in`], for a value built from (or embedding a
    /// projection of) one or more delivered carriers: every answerable borrow must point into
    /// `dest` or be covered by one of `sets` — the object delivered tier's coverage predicate,
    /// same partiality as [`Self::resident_in`]. The `StoredReach` tokens holding the reach are
    /// opaque to this layer; core extracts the sets before calling.
    pub(crate) fn resident_in_delivered(&self, dest: &KoanRegion, sets: &[&FrameSet]) -> bool {
        self.resident_in_visiting(&Residence::with_reach(dest, sets))
    }

    pub(crate) fn resident_in_visiting(&self, residence: &Residence<'_>) -> bool {
        match self {
            KObject::Number(_) | KObject::KString(_) | KObject::Bool(_) | KObject::Null => true,
            KObject::KFunction(f) => residence.owns_function(f),
            KObject::KExpression(e) => e.is_splice_free(),
            KObject::List(items, elem) => {
                elem.resident_in_visiting(residence, &mut Vec::new())
                    && items.iter().all(|h| held_resident_in(h, residence))
            }
            KObject::Dict(map, k, v) => {
                k.resident_in_visiting(residence, &mut Vec::new())
                    && v.resident_in_visiting(residence, &mut Vec::new())
                    && map.values().all(|h| held_resident_in(h, residence))
            }
            KObject::Record(fields, types) => {
                types
                    .iter()
                    .all(|(_, t)| t.resident_in_visiting(residence, &mut Vec::new()))
                    && fields.iter().all(|(_, h)| held_resident_in(h, residence))
            }
            KObject::Tagged {
                value, type_args, ..
            } => {
                value.resident_in_visiting(residence)
                    && type_args
                        .iter()
                        .all(|t| t.resident_in_visiting(residence, &mut Vec::new()))
            }
            KObject::Wrapped { inner, .. } => inner.get().resident_in_visiting(residence),
            KObject::Module(m) => residence.owns_module(m),
        }
    }

    /// Runtime type tag.
    pub fn ktype(&self) -> KType<'a> {
        match self {
            KObject::Number(_) => KType::Number,
            KObject::KString(_) => KType::Str,
            KObject::Bool(_) => KType::Bool,
            KObject::Null => KType::Null,
            KObject::List(_, elem) => KType::list(elem.clone()),
            KObject::Dict(_, k, v) => KType::dict(k.clone(), v.clone()),
            KObject::KFunction(f) => function_value_ktype(f),
            KObject::KExpression(_) => KType::KExpression,
            // A `TypeConstructor` value keeps the ctor identity — bare `SetRef` when
            // `type_args` is erased, else the applied form.
            KObject::Tagged {
                set,
                index,
                type_args,
                ..
            } => {
                let bare = KType::SetRef {
                    set: Rc::clone(set),
                    index: *index,
                };
                if type_args.is_empty() {
                    bare
                } else {
                    KType::constructor_apply(Box::new(bare), type_args.as_ref().clone())
                }
            }
            KObject::Record(_, field_types) => KType::record(field_types.clone()),
            KObject::Wrapped { type_id, .. } => (*type_id).clone(),
            KObject::Module(m) => KType::signature(SigSource::SelfOf(m), Vec::new()),
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
            KObject::List(items, elem) => KObject::List(Rc::clone(items), elem.clone()),
            KObject::Dict(entries, k, v) => KObject::Dict(Rc::clone(entries), k.clone(), v.clone()),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFunction(f) => KObject::KFunction(f),
            KObject::Tagged {
                tag,
                value,
                set,
                index,
                type_args,
            } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
                set: Rc::clone(set),
                index: *index,
                type_args: Rc::clone(type_args),
            },
            KObject::Record(fields, field_types) => {
                KObject::Record(Rc::clone(fields), field_types.clone())
            }
            KObject::Wrapped { inner, type_id } => KObject::Wrapped {
                inner: inner.clone(),
                type_id,
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
/// [`Held`] value — an object recurses structurally, a type routes to [`KType::resident_in_visiting`]
/// under the same [`Residence`].
fn held_resident_in(h: &Held<'_>, residence: &Residence<'_>) -> bool {
    match h {
        Held::Object(o) => o.resident_in_visiting(residence),
        Held::Type(t) => t.resident_in_visiting(residence, &mut Vec::new()),
    }
}

fn function_value_ktype<'a>(f: &'a KFunction<'a>) -> KType<'a> {
    use crate::machine::model::types::{DeferredReturnSurface, ReturnType};
    use crate::machine::model::Record;
    // The parameter record keys each `Argument` by its declared name — the names the
    // signature already holds, never the dispatch keywords. So a function value projects
    // the same `(name → type)` record a `:(FN (name :Type) -> _)` slot declares.
    let params: Record<KType<'a>> = f
        .signature
        .elements
        .iter()
        .filter_map(|el| match el {
            SignatureElement::Argument(a) => Some((a.name.clone(), a.ktype.clone())),
            _ => None,
        })
        .collect();
    // A `Deferred(_)` source return projects into the confined `KType::DeferredReturn`
    // carrier, holding the hashable surface shadow of the deferred form. Equality,
    // hashing, and specificity over the structural `KType` then read the deferred shape
    // directly instead of seeing it coarsened to `Any`. See
    // [ktype/records-and-limits.md § Record fields](../../../../design/typing/ktype/records-and-limits.md#record-fields-and-ktype-hashing).
    let ret = match &f.signature.return_type {
        ReturnType::Resolved(kt) => Box::new(kt.clone()),
        ReturnType::Deferred(d) => Box::new(KType::DeferredReturn(
            DeferredReturnSurface::from_deferred(d),
        )),
    };
    KType::function_type(params, ret)
}

impl<'a> Parseable<'a> for KObject<'a> {
    fn equal(&self, other: &dyn Parseable<'a>) -> bool {
        self.summarize() == other.summarize()
    }
    fn ktype(&self) -> KType<'a> {
        KObject::ktype(self)
    }
    fn summarize(&self) -> String {
        match self {
            KObject::Number(n) => n.to_string(),
            KObject::KString(s) => s.clone(),
            KObject::Bool(b) => b.to_string(),
            KObject::List(items, _) => {
                let parts: Vec<String> = items.iter().map(|i| i.summarize()).collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(entries, _, _) => {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::KExpression(e) => e.summarize(),
            KObject::KFunction(f) => f.summarize(),
            KObject::Tagged { tag, value, .. } => format!("{}({})", tag, value.summarize()),
            KObject::Record(fields, _) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{} = {}", field, value.summarize()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            KObject::Wrapped { inner, type_id } => {
                format!("{}({})", type_id.name(), Parseable::summarize(inner.get()),)
            }
            KObject::Module(m) => m.path.clone(),
        }
    }
}
