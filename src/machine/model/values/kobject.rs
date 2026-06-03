use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{CallArena, KFuture, ScopeId};
use crate::machine::model::ast::{KExpression, TypeName};
use crate::machine::model::types::{
    KType, Parseable, Record, Serializable, SignatureElement, UserTypeKind,
};

#[cfg(test)]
mod tests;

/// Reference to a [`KObject`] statically guaranteed not to be a [`KObject::Wrapped`].
/// The sole constructor [`Self::peel`] collapses any `Wrapped` layer; by induction
/// peeling one level suffices. Encodes the newtype-over-newtype collapse invariant in
/// the type rather than caller discipline.
#[derive(Copy, Clone)]
pub struct NonWrappedRef<'a>(&'a KObject<'a>);

impl<'a> NonWrappedRef<'a> {
    pub fn peel(value: &'a KObject<'a>) -> Self {
        match value {
            KObject::Wrapped { inner, .. } => *inner,
            _ => Self(value),
        }
    }

    pub fn get(&self) -> &'a KObject<'a> {
        self.0
    }
}

/// Runtime value: the universal type that `KFunction`s consume and produce.
///
/// Composite payloads are `Rc`-shared under an immutable-value contract; a future
/// mutable-list builtin would need `Rc::make_mut` at the mutation site. `Struct.fields`
/// uses `IndexMap` so iteration matches declaration order.
///
/// `KFunction` and `KFuture` carry an `Option<Rc<CallArena>>` lifecycle anchor; see
/// [per-call-arena-protocol.md § Carriers](../../../../design/per-call-arena-protocol.md#carriers).
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    /// List value. The second field is the memoized/ascribed element type: at fresh
    /// construction the join (LUB) of the contents under the immutable-`Rc` contract; at
    /// an annotated boundary re-stamped to the declared element type (coarsening
    /// included). Construct via [`KObject::list`] / [`KObject::list_with_type`]; never
    /// the tuple directly outside this module.
    List(Rc<Vec<KObject<'a>>>, Box<KType<'a>>),
    /// Dict value. The second/third fields are the memoized/ascribed key + value types,
    /// computed as the join of the keys / values at fresh construction or re-stamped at
    /// an annotated boundary.
    Dict(
        Rc<HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>>>,
        Box<KType<'a>>,
        Box<KType<'a>>,
    ),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>, Option<Rc<CallArena>>),
    KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>),
    /// Tagged-union value. `(name, scope_id)` carries the schema's identity through to
    /// the value; `ktype()` synthesizes `KType::UserType { kind: Tagged, .. }` so
    /// dispatch on type identity sees the declared union.
    ///
    /// `type_args` carries the value's runtime type arguments for a parameterized union
    /// (`Result<T, E>`): empty means erased; populated, `ktype()` synthesizes
    /// `KType::ConstructorApply` so dispatch and slot admission see the full
    /// instantiation. Populated by ascription stamping at annotated boundaries.
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
        scope_id: ScopeId,
        name: String,
        type_args: Rc<Vec<KType<'a>>>,
    },
    /// Struct value. `(name, scope_id)` carries the schema's identity through to the
    /// value; `ktype()` synthesizes `KType::UserType { kind: Struct, .. }`.
    Struct {
        name: String,
        scope_id: ScopeId,
        fields: Rc<IndexMap<String, KObject<'a>>>,
    },
    /// Anonymous structural record value (`{x = 1, y = "a"}`). The first field is the
    /// `Rc`-shared field record (identifier-keyed, declaration-ordered, order-blind
    /// equality); the second is the memoized/ascribed per-field type record — the join
    /// of each field's `ktype()` at fresh construction, re-stamped to a declared
    /// `KType::Record` at an annotated boundary (mirrors `List` / `Dict`). Construct via
    /// [`KObject::record`] / [`KObject::record_with_type`]. Distinct from the nominal
    /// `Struct`: a record carries no `(name, scope_id)` identity, only its structure.
    Record(Rc<Record<KObject<'a>>>, Box<Record<KType<'a>>>),
    /// First-class type value carrying the elaborated `KType` directly. The parser's
    /// surface `TypeName` is lowered at the seam so downstream consumers never see
    /// surface syntax again. Slot kind is still `KType::TypeExprRef`; the slot is the
    /// dispatch-position marker, the variant is the runtime value.
    ///
    /// Also the value-side carrier for first-class modules and signatures: a module
    /// value is `KTypeValue(KType::Module { module, frame })`, a signature value is
    /// `KTypeValue(KType::Signature { sig, pinned_slots })`.
    KTypeValue(KType<'a>),
    /// Bind-time carrier for a `TypeExprRef`-slot value whose surface `TypeName` couldn't
    /// be lowered to a concrete `KType` at `ExpressionPart::resolve_for` time — a
    /// bare-leaf name not in [`KType::from_name`]'s builtin table (`Point`, `IntOrd`,
    /// `MyList`). Preserves the parser-side `TypeName` for consumers that want the
    /// surface name; scope-aware resolution + memoization lives on
    /// [`crate::machine::core::Scope::resolve_type_expr`].
    TypeNameRef(TypeName),
    /// NEWTYPE carrier: tags a representation value with a NEWTYPE type identity.
    /// `inner` is invariantly *not* a `Wrapped` — the [`NonWrappedRef`] field type
    /// enforces newtype-over-newtype collapse at the construction path. `type_id` is
    /// the `&'a KType::UserType { kind: Newtype, .. }` minted at NEWTYPE declaration
    /// time (the same arena reference `bindings.types[name]` holds).
    ///
    /// `ktype()` reports `(*type_id).clone()` — the per-declaration nominal identity.
    /// ATTR over a `Wrapped` falls through to `inner`, so wrapping a struct in a
    /// NEWTYPE doesn't force every field accessor to redo.
    Wrapped {
        inner: NonWrappedRef<'a>,
        type_id: &'a KType<'a>,
    },
    Null,
}

impl<'a> KObject<'a> {
    /// Fresh `List` carrier: memoizes the element type as the join (LUB) of contents.
    /// Empty list memoizes `Any` (the join's identity); the empty-container *error*
    /// rule lives at the untyped-resolution boundary, not here.
    pub fn list(items: Vec<KObject<'a>>) -> KObject<'a> {
        let elem = KType::join_iter(items.iter().map(|i| i.ktype()));
        KObject::List(Rc::new(items), Box::new(elem))
    }

    /// `List` carrier with an explicitly supplied element type — for lift (preserve the
    /// memoized type across an arena-anchor rebuild) and ascription stamping (re-tag to
    /// the declared element type, coarsening included).
    pub fn list_with_type(items: Rc<Vec<KObject<'a>>>, elem: KType<'a>) -> KObject<'a> {
        KObject::List(items, Box::new(elem))
    }

    /// Fresh `Dict` carrier: memoizes key + value types as the join of the keys / values.
    pub fn dict(map: HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>>) -> KObject<'a> {
        let k = KType::join_iter(map.keys().map(|k| k.ktype()));
        let v = KType::join_iter(map.values().map(|v| v.ktype()));
        KObject::Dict(Rc::new(map), Box::new(k), Box::new(v))
    }

    /// `Dict` carrier with explicitly supplied key + value types. See [`Self::list_with_type`].
    pub fn dict_with_type(
        map: Rc<HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>>>,
        key: KType<'a>,
        value: KType<'a>,
    ) -> KObject<'a> {
        KObject::Dict(map, Box::new(key), Box::new(value))
    }

    /// Fresh `Record` carrier: memoizes the per-field type record as each field's
    /// `ktype()`. Field order follows declaration; equality is order-blind per the
    /// `Record` substrate.
    pub fn record(fields: Record<KObject<'a>>) -> KObject<'a> {
        let types = fields.map(|v| v.ktype());
        KObject::Record(Rc::new(fields), Box::new(types))
    }

    /// `Record` carrier with an explicitly supplied per-field type record — for
    /// ascription stamping (re-tag to the declared field types, coarsening included).
    /// See [`Self::list_with_type`].
    pub fn record_with_type(
        fields: Rc<Record<KObject<'a>>>,
        types: Record<KType<'a>>,
    ) -> KObject<'a> {
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
            (KObject::List(items, _), KType::List(elem)) => KObject::List(items, elem.clone()),
            (KObject::Dict(map, _, _), KType::Dict(k, v)) => {
                KObject::Dict(map, k.clone(), v.clone())
            }
            (
                KObject::Tagged {
                    tag,
                    value,
                    scope_id,
                    name,
                    ..
                },
                KType::ConstructorApply { args, .. },
            ) => KObject::Tagged {
                tag,
                value,
                scope_id,
                name,
                type_args: Rc::new(args.clone()),
            },
            (KObject::Record(fields, _), KType::Record(types)) => {
                KObject::Record(fields, types.clone())
            }
            (other, _) => other,
        }
    }

    /// True iff this is an empty container carrying no usable element-type information —
    /// an empty `List` whose memoized element type is `Any`, or an empty `Dict` whose
    /// key and value types are both `Any`. Reaching an *untyped* resolution boundary
    /// (untyped `LET` binding, bare top-level expression result) with this shape is an
    /// error (see [ktype.md § Runtime type-parameter carriers](../../../../design/typing/ktype.md#runtime-type-parameter-carriers)).
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

    /// Runtime type tag. `KFuture` reports as `KFunction` since a bound-but-unrun call
    /// is functionally a thunk and KFutures don't escape as user-visible values today.
    pub fn ktype(&self) -> KType<'a> {
        match self {
            KObject::Number(_) => KType::Number,
            KObject::KString(_) => KType::Str,
            KObject::Bool(_) => KType::Bool,
            KObject::Null => KType::Null,
            KObject::List(_, elem) => KType::List(elem.clone()),
            KObject::Dict(_, k, v) => KType::Dict(k.clone(), v.clone()),
            KObject::KFunction(f, _) => function_value_ktype(f),
            KObject::KFuture(t, _) => function_value_ktype(t.function),
            KObject::KExpression(_) => KType::KExpression,
            // Erased `type_args` reports the bare `UserType` identity; a populated
            // carrier synthesizes the applied form so dispatch sees the full
            // instantiation (`Result<Number, MyErr>`).
            KObject::Tagged {
                name,
                scope_id,
                type_args,
                ..
            } => {
                let bare = KType::UserType {
                    kind: UserTypeKind::tagged_sentinel(),
                    scope_id: *scope_id,
                    name: name.clone(),
                };
                if type_args.is_empty() {
                    bare
                } else {
                    KType::ConstructorApply {
                        ctor: Box::new(bare),
                        args: type_args.as_ref().clone(),
                    }
                }
            }
            KObject::Struct { name, scope_id, .. } => KType::UserType {
                kind: UserTypeKind::struct_sentinel(),
                scope_id: *scope_id,
                name: name.clone(),
            },
            // O(1): read the memoized per-field type record rather than re-walking the
            // fields, mirroring `List` / `Dict`.
            KObject::Record(_, field_types) => KType::Record(field_types.clone()),
            // Module/signature values carry the identity directly — report it rather
            // than the meta-type marker, so dispatch sees the same shape as a
            // type-position carrier. Other `KTypeValue` carriers fill the
            // `TypeExprRef` dispatch-position marker.
            KObject::KTypeValue(kt) => match kt {
                KType::Module { .. } | KType::Signature { .. } => kt.clone(),
                _ => KType::TypeExprRef,
            },
            // Dispatch-equivalent to `KTypeValue` — both fill a `TypeExprRef`-typed slot.
            KObject::TypeNameRef(_) => KType::TypeExprRef,
            KObject::Wrapped { type_id, .. } => (*type_id).clone(),
        }
    }

    /// Independent-but-cheap clone: composite payloads `Rc::clone` under the
    /// immutable-value contract; `KFunction`/`KFuture` preserve their `Rc<CallArena>`
    /// anchor.
    pub fn deep_clone(&self) -> KObject<'a> {
        match self {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            KObject::List(items, elem) => KObject::List(Rc::clone(items), elem.clone()),
            KObject::Dict(entries, k, v) => KObject::Dict(Rc::clone(entries), k.clone(), v.clone()),
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFuture(t, frame) => KObject::KFuture(t.deep_clone(), frame.clone()),
            KObject::KFunction(f, frame) => KObject::KFunction(f, frame.clone()),
            KObject::Tagged {
                tag,
                value,
                scope_id,
                name,
                type_args,
            } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
                scope_id: *scope_id,
                name: name.clone(),
                type_args: Rc::clone(type_args),
            },
            KObject::Struct {
                name,
                scope_id,
                fields,
            } => KObject::Struct {
                name: name.clone(),
                scope_id: *scope_id,
                fields: Rc::clone(fields),
            },
            KObject::Record(fields, field_types) => {
                KObject::Record(Rc::clone(fields), field_types.clone())
            }
            KObject::KTypeValue(t) => KObject::KTypeValue(t.clone()),
            KObject::TypeNameRef(t) => KObject::TypeNameRef(t.clone()),
            KObject::Wrapped { inner, type_id } => KObject::Wrapped {
                inner: *inner,
                type_id,
            },
        }
    }

    pub fn as_kexpression(&self) -> Option<&KExpression<'a>> {
        match self {
            KObject::KExpression(e) => Some(e),
            _ => None,
        }
    }

    /// Projects through the `KTypeValue(KType::Module { .. })` carrier.
    pub fn as_module(&self) -> Option<&'a super::module::Module<'a>> {
        match self {
            KObject::KTypeValue(KType::Module { module, .. }) => Some(*module),
            _ => None,
        }
    }

    /// Projects through the `KTypeValue(KType::Signature { .. })` carrier.
    pub fn as_signature(&self) -> Option<&'a super::module::Signature<'a>> {
        match self {
            KObject::KTypeValue(KType::Signature { sig, .. }) => Some(*sig),
            _ => None,
        }
    }

    pub fn as_ktype(&self) -> Option<&KType<'a>> {
        match self {
            KObject::KTypeValue(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<&'a KFunction<'a>> {
        match self {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        }
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
    // [ktype.md § Record fields](../../../../design/typing/ktype.md#record-fields-and-ktype-hashing).
    let ret = match &f.signature.return_type {
        ReturnType::Resolved(kt) => Box::new(kt.clone()),
        ReturnType::Deferred(d) => Box::new(KType::DeferredReturn(
            DeferredReturnSurface::from_deferred(d),
        )),
    };
    // `is_functor` projects into the disjoint `KFunctor` family; cross-arm
    // admissibility is refused in `function_compat` — see
    // [design/typing/functors.md](../../../../design/typing/functors.md). The
    // projected functor type carries `body: Some(f)` — the callable handle that a
    // type-bound functor name (`LET F = (FUNCTOR …)`) is applied through — while
    // staying identity-inert under equality/hashing.
    if f.is_functor {
        KType::KFunctor {
            params,
            ret,
            body: Some(f),
        }
    } else {
        KType::KFunction { params, ret }
    }
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
            KObject::KFuture(t, _) => t.parsed.summarize(),
            KObject::KFunction(f, _) => f.summarize(),
            KObject::Tagged { tag, value, .. } => format!("{}({})", tag, value.summarize()),
            KObject::Struct { name, fields, .. } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{}: {}", field, value.summarize()))
                    .collect();
                format!("{}({})", name, parts.join(", "))
            }
            // Round-trips the `{x = 1, y = "a"}` value surface (`=` pairs).
            KObject::Record(fields, _) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{} = {}", field, value.summarize()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            // Module / signature carriers render as `module <path>` / `sig <path>`.
            KObject::KTypeValue(KType::Module { module, .. }) => format!("module {}", module.path),
            KObject::KTypeValue(KType::Signature { sig, .. }) => format!("sig {}", sig.path),
            KObject::KTypeValue(t) => t.render(),
            // Preserve the surface form the user wrote (`Point`, `Foo<Bar>`) — going
            // through the scope-resolved `&KType` would route via `name()` and might
            // normalize, which the "surface form survives bind" invariant forbids.
            KObject::TypeNameRef(t) => t.render(),
            // Render as `Distance(<inner summary>)`; `type_id.name()` returns the bare
            // declared name (per `user_type_name_renders_bare_name`).
            KObject::Wrapped { inner, type_id } => {
                format!("{}({})", type_id.name(), Parseable::summarize(inner.get()),)
            }
        }
    }
}
