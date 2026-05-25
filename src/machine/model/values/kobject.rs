use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::machine::model::ast::{KExpression, TypeExpr};
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{CallArena, KFuture, ScopeId};
use crate::machine::model::types::{KType, Parseable, Serializable, SignatureElement, UserTypeKind};
use super::module::{Module, Signature};

#[cfg(test)]
mod tests;

/// Reference to a [`KObject`] that is statically guaranteed not to be a
/// [`KObject::Wrapped`]. The only constructor is [`Self::peel`], which collapses any
/// `Wrapped` layer at construction time; by induction (every prior construction went
/// through the same peel), peeling one level is enough. Used as the field type of
/// `KObject::Wrapped.inner` so the newtype-over-newtype collapse invariant is encoded
/// in the type rather than enforced by caller discipline.
#[derive(Copy, Clone)]
pub struct NonWrappedRef<'a>(&'a KObject<'a>);

impl<'a> NonWrappedRef<'a> {
    /// Sole constructor. Peels any `Wrapped` layer so the wrapped reference is
    /// invariantly not to a `Wrapped`.
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

/// Runtime value: scalars, collections, an unevaluated expression, a bound-but-unrun task, or a
/// reference to a function in some scope. The universal value type that `KFunction`s consume
/// and produce; implements `Parseable` so values can be compared and rendered uniformly.
///
/// Composite payloads (`List`, `Dict`, `Tagged`, `Struct`, `TaggedUnionType`) are
/// `Rc`-shared under an immutable-value contract: a future mutable-list builtin would need
/// `Rc::make_mut` at the mutation site. `Struct.fields` uses `IndexMap` so iteration order
/// matches schema declaration order.
///
/// `KFunction` and `KFuture` carry an `Option<Rc<CallArena>>` lifecycle anchor; see
/// [memory-model.md § Closure escape](../../../../design/memory-model.md#closure-escape-per-call-arenas--rc).
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    /// List value. The second field is the memoized/ascribed element type: at fresh
    /// construction (`KObject::list`) it is the join (LUB) of the contents, computed once
    /// under the immutable-`Rc` contract; at an annotated boundary it is re-stamped to the
    /// declared element type (coarsening included). `ktype()` reads this field directly
    /// rather than re-walking the contents. Construct via [`KObject::list`] /
    /// [`KObject::list_with_type`]; never the tuple directly outside this module.
    List(Rc<Vec<KObject<'a>>>, Box<KType>),
    /// Dict value. The second/third fields are the memoized/ascribed key + value types,
    /// computed once at construction (`KObject::dict`) as the join of the keys / values, or
    /// re-stamped at an annotated boundary. `ktype()` reads them directly.
    Dict(
        Rc<HashMap<Box<dyn Serializable + 'a>, KObject<'a>>>,
        Box<KType>,
        Box<KType>,
    ),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>, Option<Rc<CallArena>>),
    KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>),
    /// Tagged-union schema. `(name, scope_id)` is the declared type's identity —
    /// `name` is the declared type name (`Maybe`), `scope_id` is the declaring scope's
    /// `ScopeId` and uses the same scheme `Module::scope_id()` does. `ktype()` reports
    /// `KType::Type` (the schema is a value *of* the meta-type); `Tagged` *values*
    /// synthesize `KType::UserType { kind: Tagged, .. }` from these identity fields,
    /// which `crate::builtins::tagged_union::construct` copies onto each
    /// produced value.
    TaggedUnionType {
        schema: Rc<HashMap<String, KType>>,
        name: String,
        scope_id: ScopeId,
    },
    /// Struct schema. `(scope_id, name)` is the declared type's identity — same scheme
    /// as `TaggedUnionType`. `ktype()` reports `KType::Type`; produced `Struct` values
    /// synthesize `KType::UserType { kind: Struct, .. }` from these identity fields,
    /// copied onto each value by `crate::builtins::struct_value::construct`.
    StructType {
        name: String,
        scope_id: ScopeId,
        fields: Rc<Vec<(String, KType)>>,
    },
    /// Tagged-union value. `(name, scope_id)` carries the declaring schema's identity
    /// through to the value, populated by `crate::builtins::tagged_union::construct` from the schema
    /// in the bundle. `ktype()` synthesizes `KType::UserType { kind: Tagged, .. }`
    /// from these fields so dispatch on type identity sees the declared union.
    ///
    /// `type_args` carries the value's runtime type arguments for a parameterized union
    /// (`Result<T, E>`): empty (`Rc::new(vec![])`) means erased — `ktype()` reports the
    /// bare `UserType` as before — and when populated `ktype()` synthesizes
    /// `KType::ConstructorApply { ctor, args: type_args }` so dispatch and slot admission
    /// see the full instantiation. Populated by ascription stamping at annotated boundaries.
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
        scope_id: ScopeId,
        name: String,
        type_args: Rc<Vec<KType>>,
    },
    /// Struct value. `(name, scope_id)` carries the declaring schema's identity through
    /// to the value, populated by `crate::builtins::struct_value::construct`. `ktype()` synthesizes
    /// `KType::UserType { kind: Struct, .. }` from these fields.
    Struct {
        name: String,
        scope_id: ScopeId,
        fields: Rc<IndexMap<String, KObject<'a>>>,
    },
    /// First-class type value carrying the elaborated `KType` directly. The parser's
    /// surface `TypeExpr` is lowered at the seam (`ExpressionPart::resolve_for` for bare
    /// `Type(_)` tokens, the type-builtins for parameterized sub-dispatches) so consumers
    /// downstream never see surface syntax again. Slot kind is still `KType::TypeExprRef`;
    /// the slot is the dispatch-position marker, the variant is the runtime value.
    KTypeValue(KType),
    /// Bind-time carrier for a `TypeExprRef`-slot value whose surface `TypeExpr` couldn't
    /// be lowered to a concrete `KType` at `ExpressionPart::resolve_for` time — i.e. a
    /// bare-leaf name not in [`KType::from_name`]'s builtin table (`Point`, `IntOrd`,
    /// `MyList`). Preserves the parser-side `TypeExpr` for consumers that want the surface
    /// name (`extract_bare_type_name`, ATTR's TypeExprRef-lhs lookup, FN's deferred return-
    /// type elaboration, `LET <Type-class> = …`); scope-aware resolution + memoization
    /// now lives on [`crate::machine::core::Scope::resolve_type_expr`].
    TypeNameRef(TypeExpr),
    /// `Option<Rc<CallArena>>` mirrors `KFunction`'s lifecycle anchor: a `Module` whose
    /// child scope was alloc'd inside a per-call frame (a functor body's freshly-built
    /// `MODULE Result = (...)`) carries the frame's `Rc` so the captured scope outlives
    /// the dying frame. `None` for modules built outside a per-call frame (top-level
    /// `MODULE Foo = (...)` and the ascription paths). See [memory-model.md § Closure
    /// escape](../../../../design/memory-model.md#closure-escape-per-call-arenas--rc).
    KModule(&'a Module<'a>, Option<Rc<CallArena>>),
    KSignature(&'a Signature<'a>),
    /// Stage-4 NEWTYPE carrier. Tags a representation value with a NEWTYPE type identity.
    /// `inner` is the underlying representation value, invariantly *not* a `Wrapped` —
    /// the [`NonWrappedRef`] field type enforces newtype-over-newtype collapse at the
    /// only construction path ([`crate::builtins::newtype_def::newtype_construct`]'s
    /// Combine finish, which builds the carrier through [`NonWrappedRef::peel`]).
    /// `type_id` is the `&'a KType::UserType { kind: Newtype, .. }` minted at NEWTYPE
    /// declaration time (the same arena reference `bindings.types[name]` holds).
    ///
    /// `ktype()` reports `(*type_id).clone()` — the per-declaration nominal identity.
    /// Dispatch on a slot typed by `Distance` admits a `Wrapped` whose `type_id`
    /// resolves to the same `(scope_id, name)`. ATTR over a `Wrapped` falls through to
    /// `inner` (`access_field`'s `Wrapped` arm), so wrapping a struct in a NEWTYPE
    /// doesn't force every field accessor to redo.
    Wrapped {
        inner: NonWrappedRef<'a>,
        type_id: &'a KType,
    },
    Null,
}

impl<'a> KObject<'a> {
    /// Fresh `List` carrier: computes the element type once as the join (LUB) of the
    /// contents under the immutable-`Rc` contract. Empty list memoizes `Any` (the join's
    /// identity); the empty-container *error* rule lives at the untyped-resolution boundary,
    /// not here.
    pub fn list(items: Vec<KObject<'a>>) -> KObject<'a> {
        let elem = KType::join_iter(items.iter().map(|i| i.ktype()));
        KObject::List(Rc::new(items), Box::new(elem))
    }

    /// `List` carrier with an explicitly supplied element type. Used by lift (preserve the
    /// already-memoized type across an arena-anchor rebuild) and by ascription stamping
    /// (re-tag to the declared element type, coarsening included).
    pub fn list_with_type(items: Rc<Vec<KObject<'a>>>, elem: KType) -> KObject<'a> {
        KObject::List(items, Box::new(elem))
    }

    /// Fresh `Dict` carrier: computes key + value types once as the join of the keys /
    /// values.
    pub fn dict(map: HashMap<Box<dyn Serializable + 'a>, KObject<'a>>) -> KObject<'a> {
        let k = KType::join_iter(map.keys().map(|k| k.ktype()));
        let v = KType::join_iter(map.values().map(|v| v.ktype()));
        KObject::Dict(Rc::new(map), Box::new(k), Box::new(v))
    }

    /// `Dict` carrier with explicitly supplied key + value types. See [`Self::list_with_type`].
    pub fn dict_with_type(
        map: Rc<HashMap<Box<dyn Serializable + 'a>, KObject<'a>>>,
        key: KType,
        value: KType,
    ) -> KObject<'a> {
        KObject::Dict(map, Box::new(key), Box::new(value))
    }

    /// Ascription stamping at an annotated boundary (FN return type, argument slot, LET
    /// ascription). The declared type is the contract: callers have already checked the
    /// value satisfies `declared` via `matches_value`; this re-tags the carrier to
    /// *exactly* the declared parameter types, coarsening included — a `List<Number>` value
    /// returned through `:(List Any)` re-tags to `List<Any>`, so downstream dispatch sees
    /// the contract rather than the implementation's incidental precision.
    ///
    /// Only the three parameterized carriers are re-tagged; every other shape passes
    /// through unchanged (its `ktype()` is already its nominal identity). For a `Tagged`
    /// stamped against a `ConstructorApply`, the constructor identity must already match
    /// (the caller's `matches_value` guaranteed it); the `type_args` are replaced with the
    /// declared args. Empty containers stamp vacuously (the declared element type wins).
    pub fn stamp_type(self, declared: &KType) -> KObject<'a> {
        match (self, declared) {
            (KObject::List(items, _), KType::List(elem)) => {
                KObject::List(items, elem.clone())
            }
            (KObject::Dict(map, _, _), KType::Dict(k, v)) => {
                KObject::Dict(map, k.clone(), v.clone())
            }
            (
                KObject::Tagged { tag, value, scope_id, name, .. },
                KType::ConstructorApply { args, .. },
            ) => KObject::Tagged {
                tag,
                value,
                scope_id,
                name,
                type_args: Rc::new(args.clone()),
            },
            (other, _) => other,
        }
    }

    /// True iff this is an empty container carrying no usable element-type information —
    /// an empty `List` whose memoized element type is `Any`, or an empty `Dict` whose key
    /// and value types are both `Any`. Such a value has no join to infer from and was never
    /// stamped by an annotation; reaching an *untyped* resolution boundary (an untyped `LET`
    /// binding, a bare top-level expression result) with this shape is an error
    /// (see [runtime-type-parameter-carriers](../../../../roadmap/type_language/runtime-type-parameter-carriers.md)).
    ///
    /// A stamped empty container (e.g. `FN -> :(List Number) = ([])` re-tags to element
    /// `Number`) is *not* flagged: its carrier carries a non-`Any` element type. A
    /// non-empty heterogeneous literal (`[2, "hello"]` → `List<Any>`) is *not* flagged: it
    /// carries information and is legal where `:(List Any)` is declared.
    pub fn is_unstamped_empty_container(&self) -> bool {
        match self {
            KObject::List(items, elem) => {
                items.is_empty() && matches!(elem.as_ref(), KType::Any)
            }
            KObject::Dict(map, k, v) => {
                map.is_empty()
                    && matches!(k.as_ref(), KType::Any)
                    && matches!(v.as_ref(), KType::Any)
            }
            _ => false,
        }
    }

    /// Runtime type tag. `KFuture` reports as `KFunction` since a bound-but-unrun call is
    /// functionally a thunk and KFutures don't escape as user-visible values today.
    pub fn ktype(&self) -> KType {
        match self {
            KObject::Number(_) => KType::Number,
            KObject::KString(_) => KType::Str,
            KObject::Bool(_) => KType::Bool,
            KObject::Null => KType::Null,
            // O(1) field read of the memoized/ascribed element type — no contents re-walk.
            KObject::List(_, elem) => KType::List(elem.clone()),
            KObject::Dict(_, k, v) => KType::Dict(k.clone(), v.clone()),
            KObject::KFunction(f, _) => function_value_ktype(f),
            KObject::KFuture(t, _) => function_value_ktype(t.function),
            KObject::KExpression(_) => KType::KExpression,
            // Schema carriers report the meta-type (`KType::Type`): they are values *of*
            // the meta-type, not user-typed values. Per-declaration value carriers
            // (`Struct`, `Tagged`, `KModule`) synthesize `KType::UserType` from their
            // `(scope_id, name)` identity fields so dispatch on type identity sees
            // distinct types per declaration.
            KObject::TaggedUnionType { .. } => KType::Type,
            KObject::StructType { .. } => KType::Type,
            // Erased `type_args` reports the bare `UserType` identity (today's behavior);
            // a populated carrier synthesizes the applied form so dispatch / slot admission
            // see the full instantiation (`Result<Number, MyErr>`).
            KObject::Tagged { name, scope_id, type_args, .. } => {
                let bare = KType::UserType {
                    kind: UserTypeKind::Tagged,
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
                kind: UserTypeKind::Struct,
                scope_id: *scope_id,
                name: name.clone(),
            },
            KObject::KTypeValue(_) => KType::TypeExprRef,
            // `TypeNameRef` is dispatch-equivalent to `KTypeValue` — both fill a
            // `TypeExprRef`-typed slot. The slot's role is the dispatch-position marker;
            // whether the carrier resolved at `resolve_for` time or stays surface-form
            // until a scope-aware consumer asks is an internal detail.
            KObject::TypeNameRef(_) => KType::TypeExprRef,
            KObject::KModule(m, _) => KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: m.scope_id(),
                name: m.path.clone(),
            },
            KObject::KSignature(_) => KType::MetaSignature,
            // Stage 4: a `Wrapped` reports its cached NEWTYPE identity directly. The cell
            // is the arena ref the declaration site minted; cloning preserves the
            // `(kind, scope_id, name)` triple the dispatcher reads.
            KObject::Wrapped { type_id, .. } => (*type_id).clone(),
        }
    }

    /// Independent-but-cheap clone: composite payloads are `Rc::clone`d under the
    /// immutable-value contract; `KFunction`/`KFuture` preserve their `Rc<CallArena>` anchor.
    pub fn deep_clone(&self) -> KObject<'a> {
        match self {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            KObject::List(items, elem) => KObject::List(Rc::clone(items), elem.clone()),
            KObject::Dict(entries, k, v) => {
                KObject::Dict(Rc::clone(entries), k.clone(), v.clone())
            }
            KObject::KExpression(e) => KObject::KExpression(e.clone()),
            KObject::KFuture(t, frame) => KObject::KFuture(t.deep_clone(), frame.clone()),
            KObject::KFunction(f, frame) => KObject::KFunction(f, frame.clone()),
            KObject::TaggedUnionType { schema, name, scope_id } => KObject::TaggedUnionType {
                schema: Rc::clone(schema),
                name: name.clone(),
                scope_id: *scope_id,
            },
            KObject::StructType { name, scope_id, fields } => KObject::StructType {
                name: name.clone(),
                scope_id: *scope_id,
                fields: Rc::clone(fields),
            },
            KObject::Tagged { tag, value, scope_id, name, type_args } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
                scope_id: *scope_id,
                name: name.clone(),
                type_args: Rc::clone(type_args),
            },
            KObject::Struct { name, scope_id, fields } => KObject::Struct {
                name: name.clone(),
                scope_id: *scope_id,
                fields: Rc::clone(fields),
            },
            KObject::KTypeValue(t) => KObject::KTypeValue(t.clone()),
            KObject::TypeNameRef(t) => KObject::TypeNameRef(t.clone()),
            KObject::KModule(m, frame) => KObject::KModule(m, frame.clone()),
            KObject::KSignature(s) => KObject::KSignature(s),
            // Stage 4: both fields are arena references; copying them preserves the
            // immutable-carrier contract. `inner` already lives in the arena, so no
            // deep allocation is needed here.
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

    /// Returns the `Rc` directly so callers can `Rc::clone` the field list.
    #[allow(clippy::type_complexity)]
    pub fn as_struct_type(&self) -> Option<(&str, &Rc<Vec<(String, KType)>>)> {
        match self {
            KObject::StructType { name, fields, .. } => Some((name.as_str(), fields)),
            _ => None,
        }
    }

    pub fn as_tagged_union_type(&self) -> Option<&Rc<HashMap<String, KType>>> {
        match self {
            KObject::TaggedUnionType { schema, .. } => Some(schema),
            _ => None,
        }
    }

    pub fn as_module(&self) -> Option<&'a Module<'a>> {
        match self {
            KObject::KModule(m, _) => Some(*m),
            _ => None,
        }
    }

    pub fn as_signature(&self) -> Option<&'a Signature<'a>> {
        match self {
            KObject::KSignature(s) => Some(*s),
            _ => None,
        }
    }

    pub fn as_ktype(&self) -> Option<&KType> {
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

fn function_value_ktype<'a>(f: &KFunction<'a>) -> KType {
    use crate::machine::model::types::ReturnType;
    let args: Vec<KType> = f
        .signature
        .elements
        .iter()
        .filter_map(|el| match el {
            SignatureElement::Argument(a) => Some(a.ktype.clone()),
            _ => None,
        })
        .collect();
    // Module-system functor-params Stage B coarsening: structural `KType::KFunction`
    // can't carry a `Deferred(_)` return-type carrier (the structural type language has
    // no surface for "per-call elaboration of this expression"). Collapse to `KType::Any`
    // so the structural type stays well-formed; the precise per-call return type is
    // observed at the dispatch boundary, not from a structural-type comparison.
    let ret = match &f.signature.return_type {
        ReturnType::Resolved(kt) => Box::new(kt.clone()),
        ReturnType::Deferred(_) => Box::new(KType::Any),
    };
    KType::KFunction { args, ret }
}

impl<'a> Parseable for KObject<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool {
        self.summarize() == other.summarize()
    }
    fn ktype(&self) -> KType {
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
            KObject::TaggedUnionType { schema, .. } => {
                let parts: Vec<String> = schema
                    .iter()
                    .map(|(tag, ktype)| format!("{}: {}", tag, ktype.name()))
                    .collect();
                format!("Union{{{}}}", parts.join(", "))
            }
            KObject::StructType { name, fields, .. } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, ktype)| format!("{}: {}", field, ktype.name()))
                    .collect();
                format!("{}{{{}}}", name, parts.join(", "))
            }
            KObject::Tagged { tag, value, .. } => format!("{}({})", tag, value.summarize()),
            KObject::Struct { name, fields, .. } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{}: {}", field, value.summarize()))
                    .collect();
                format!("{}({})", name, parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            KObject::KTypeValue(t) => t.render(),
            // Preserve the surface form the user wrote (`Point`, `Foo<Bar>`) for
            // diagnostics — rendering through the scope-resolved `&KType` would route
            // via `name()` and might normalize, which the "surface form survives bind"
            // invariant forbids.
            KObject::TypeNameRef(t) => t.render(),
            KObject::KModule(m, _) => format!("module {}", m.path),
            KObject::KSignature(s) => format!("sig {}", s.path),
            // Stage 4: render as `Distance(<inner summary>)`. `type_id.name()` returns
            // the bare declared name (per `user_type_name_renders_bare_name`); the
            // inner summary recurses via the `Parseable` impl, mirroring the
            // surface-form invariant Struct / Tagged carriers honor.
            KObject::Wrapped { inner, type_id } => format!(
                "{}({})",
                type_id.name(),
                Parseable::summarize(inner.get()),
            ),
        }
    }
}
