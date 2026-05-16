use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::runtime::machine::model::ast::{KExpression, TypeExpr};
use crate::runtime::machine::core::kfunction::KFunction;
use crate::runtime::machine::core::{CallArena, KFuture, ScopeId};
use crate::runtime::machine::model::types::{KType, Parseable, Serializable, SignatureElement, UserTypeKind};
use super::module::{Module, Signature};

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
/// [memory-model.md § Closure escape](../../../../../design/memory-model.md#closure-escape-per-call-arenas--rc).
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    List(Rc<Vec<KObject<'a>>>),
    Dict(Rc<HashMap<Box<dyn Serializable + 'a>, KObject<'a>>>),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>, Option<Rc<CallArena>>),
    KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>),
    /// Tagged-union schema. `(name, scope_id)` is the declared type's identity —
    /// `name` is the declared type name (`Maybe`), `scope_id` is the declaring scope's
    /// `ScopeId` and uses the same scheme `Module::scope_id()` does. `ktype()` reports
    /// `KType::Type` (the schema is a value *of* the meta-type); `Tagged` *values*
    /// synthesize `KType::UserType { kind: Tagged, .. }` from these identity fields,
    /// which `crate::runtime::builtins::tagged_union::construct` copies onto each
    /// produced value.
    TaggedUnionType {
        schema: Rc<HashMap<String, KType>>,
        name: String,
        scope_id: ScopeId,
    },
    /// Struct schema. `(scope_id, name)` is the declared type's identity — same scheme
    /// as `TaggedUnionType`. `ktype()` reports `KType::Type`; produced `Struct` values
    /// synthesize `KType::UserType { kind: Struct, .. }` from these identity fields,
    /// copied onto each value by `crate::runtime::builtins::struct_value::construct`.
    StructType {
        name: String,
        scope_id: ScopeId,
        fields: Rc<Vec<(String, KType)>>,
    },
    /// Tagged-union value. `(name, scope_id)` carries the declaring schema's identity
    /// through to the value, populated by `crate::runtime::builtins::tagged_union::construct` from the schema
    /// in the bundle. `ktype()` synthesizes `KType::UserType { kind: Tagged, .. }`
    /// from these fields so dispatch on type identity sees the declared union.
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
        scope_id: ScopeId,
        name: String,
    },
    /// Struct value. `(name, scope_id)` carries the declaring schema's identity through
    /// to the value, populated by `crate::runtime::builtins::struct_value::construct`. `ktype()` synthesizes
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
    /// now lives on [`crate::runtime::machine::core::Scope::resolve_type_expr`].
    TypeNameRef(TypeExpr),
    /// `Option<Rc<CallArena>>` mirrors `KFunction`'s lifecycle anchor: a `Module` whose
    /// child scope was alloc'd inside a per-call frame (a functor body's freshly-built
    /// `MODULE Result = (...)`) carries the frame's `Rc` so the captured scope outlives
    /// the dying frame. `None` for modules built outside a per-call frame (top-level
    /// `MODULE Foo = (...)` and the ascription paths). See [memory-model.md § Closure
    /// escape](../../../design/memory-model.md#closure-escape-per-call-arenas--rc).
    KModule(&'a Module<'a>, Option<Rc<CallArena>>),
    KSignature(&'a Signature<'a>),
    /// Stage-4 NEWTYPE carrier. Tags a representation value with a NEWTYPE type identity.
    /// `inner` is the underlying representation value (arena-allocated, invariantly *not*
    /// a `Wrapped` — newtype-over-newtype is collapsed to a single layer at construction
    /// time in `newtype_def::newtype_construct`'s Combine finish). `type_id` is the
    /// `&'a KType::UserType { kind: Newtype, .. }` minted at NEWTYPE declaration time
    /// (the same arena reference `bindings.types[name]` holds).
    ///
    /// `ktype()` reports `(*type_id).clone()` — the per-declaration nominal identity.
    /// Dispatch on a slot typed by `Distance` admits a `Wrapped` whose `type_id`
    /// resolves to the same `(scope_id, name)`. ATTR over a `Wrapped` falls through to
    /// `inner` (`access_field`'s `Wrapped` arm), so wrapping a struct in a NEWTYPE
    /// doesn't force every field accessor to redo.
    Wrapped {
        inner: &'a KObject<'a>,
        type_id: &'a KType,
    },
    Null,
}

impl<'a> KObject<'a> {
    /// Runtime type tag. `KFuture` reports as `KFunction` since a bound-but-unrun call is
    /// functionally a thunk and KFutures don't escape as user-visible values today.
    pub fn ktype(&self) -> KType {
        match self {
            KObject::Number(_) => KType::Number,
            KObject::KString(_) => KType::Str,
            KObject::Bool(_) => KType::Bool,
            KObject::Null => KType::Null,
            KObject::List(items) => {
                let elem = KType::join_iter(items.iter().map(|i| i.ktype()));
                KType::List(Box::new(elem))
            }
            KObject::Dict(map) => {
                let k = KType::join_iter(map.keys().map(|k| k.ktype()));
                let v = KType::join_iter(map.values().map(|v| v.ktype()));
                KType::Dict(Box::new(k), Box::new(v))
            }
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
            KObject::Tagged { name, scope_id, .. } => KType::UserType {
                kind: UserTypeKind::Tagged,
                scope_id: *scope_id,
                name: name.clone(),
            },
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
            KObject::KSignature(_) => KType::Signature,
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
            KObject::List(items) => KObject::List(Rc::clone(items)),
            KObject::Dict(entries) => KObject::Dict(Rc::clone(entries)),
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
            KObject::Tagged { tag, value, scope_id, name } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
                scope_id: *scope_id,
                name: name.clone(),
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
                inner,
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
}

fn function_value_ktype<'a>(f: &KFunction<'a>) -> KType {
    use crate::runtime::machine::model::types::ReturnType;
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
            KObject::List(items) => {
                let parts: Vec<String> = items.iter().map(|i| i.summarize()).collect();
                format!("[{}]", parts.join(", "))
            }
            KObject::Dict(entries) => {
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
                Parseable::summarize(*inner),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::machine::model::values::KKey;
    use std::collections::HashMap;

    #[test]
    fn ktype_of_homogeneous_number_list() {
        let l: KObject<'_> =
            KObject::List(Rc::new(vec![KObject::Number(1.0), KObject::Number(2.0)]));
        assert_eq!(l.ktype(), KType::List(Box::new(KType::Number)));
    }

    #[test]
    fn ktype_of_mixed_list_is_list_any() {
        let l: KObject<'_> = KObject::List(Rc::new(vec![
            KObject::Number(1.0),
            KObject::KString("x".into()),
        ]));
        assert_eq!(l.ktype(), KType::List(Box::new(KType::Any)));
    }

    #[test]
    fn ktype_of_empty_list_is_list_any() {
        let l: KObject<'_> = KObject::List(Rc::new(vec![]));
        assert_eq!(l.ktype(), KType::List(Box::new(KType::Any)));
    }

    #[test]
    fn ktype_of_nested_list() {
        let inner: KObject<'_> = KObject::List(Rc::new(vec![KObject::Number(1.0)]));
        let outer: KObject<'_> = KObject::List(Rc::new(vec![inner]));
        assert_eq!(
            outer.ktype(),
            KType::List(Box::new(KType::List(Box::new(KType::Number))))
        );
    }

    #[test]
    fn ktype_of_dict_string_number() {
        let mut map: HashMap<Box<dyn Serializable + 'static>, KObject<'static>> = HashMap::new();
        map.insert(Box::new(KKey::String("a".into())), KObject::Number(1.0));
        map.insert(Box::new(KKey::String("b".into())), KObject::Number(2.0));
        let d: KObject<'_> = KObject::Dict(Rc::new(map));
        assert_eq!(
            d.ktype(),
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    #[test]
    fn ktype_of_empty_dict_is_dict_any_any() {
        let map: HashMap<Box<dyn Serializable + 'static>, KObject<'static>> = HashMap::new();
        let d: KObject<'_> = KObject::Dict(Rc::new(map));
        assert_eq!(
            d.ktype(),
            KType::Dict(Box::new(KType::Any), Box::new(KType::Any))
        );
    }

    #[test]
    fn matches_value_list_number_rejects_string_element() {
        let t = KType::List(Box::new(KType::Number));
        let bad: KObject<'_> = KObject::List(Rc::new(vec![
            KObject::Number(1.0),
            KObject::KString("x".into()),
        ]));
        assert!(!t.matches_value(&bad));
    }

    #[test]
    fn matches_value_list_number_accepts_all_numbers() {
        let t = KType::List(Box::new(KType::Number));
        let good: KObject<'_> = KObject::List(Rc::new(vec![
            KObject::Number(1.0),
            KObject::Number(2.0),
        ]));
        assert!(t.matches_value(&good));
    }

    #[test]
    fn matches_value_list_any_accepts_any_list() {
        let t = KType::List(Box::new(KType::Any));
        let mixed: KObject<'_> = KObject::List(Rc::new(vec![
            KObject::Number(1.0),
            KObject::KString("x".into()),
        ]));
        assert!(t.matches_value(&mixed));
    }

    /// `TypeNameRef` summarizes through `TypeExpr::render`, preserving the surface form
    /// (`MyT`, `Point<Foo>`) for diagnostics. The surface form must survive bind
    /// regardless of whether downstream scope-aware consumers have resolved the
    /// carrier.
    #[test]
    fn type_name_ref_summarize_renders_surface_form() {
        use crate::runtime::machine::model::ast::TypeExpr;
        let v = KObject::TypeNameRef(TypeExpr::leaf("MyT".into()));
        use crate::runtime::machine::model::types::Parseable;
        assert_eq!(v.summarize(), "MyT");
    }

    /// `TypeNameRef::ktype()` reports `TypeExprRef` so it fills the same dispatch slot as
    /// the fully-elaborated `KTypeValue` carrier. Pins the slot-routing invariant.
    #[test]
    fn type_name_ref_ktype_is_type_expr_ref() {
        use crate::runtime::machine::model::ast::TypeExpr;
        let v = KObject::TypeNameRef(TypeExpr::leaf("MyT".into()));
        assert_eq!(v.ktype(), KType::TypeExprRef);
    }

    #[test]
    fn ktype_value_round_trips_through_summarize() {
        // `KObject::KTypeValue` summarizes through `KType::render`, mirroring the surface
        // form a user would write. Pins the post-refactor diagnostic shape.
        let v = KObject::KTypeValue(KType::List(Box::new(KType::Number)));
        use crate::runtime::machine::model::types::Parseable;
        assert_eq!(v.summarize(), ":(List Number)");
    }

    /// Stage 4: `Wrapped::ktype()` reports a clone of `*type_id`, preserving the full
    /// `(kind, scope_id, name)` triple the dispatcher reads for per-declaration identity
    /// comparisons.
    #[test]
    fn wrapped_ktype_reports_clone_of_type_id() {
        use crate::runtime::machine::RuntimeArena;
        let arena = RuntimeArena::new();
        let inner = arena.alloc_object(KObject::Number(3.0));
        let type_id: &KType = arena.alloc_ktype(KType::UserType {
            kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
            scope_id: ScopeId::from_raw(0, 0xAA),
            name: "Distance".into(),
        });
        let w = KObject::Wrapped { inner, type_id };
        match w.ktype() {
            KType::UserType { kind: UserTypeKind::Newtype { .. }, name, scope_id } => {
                assert_eq!(name, "Distance");
                assert_eq!(scope_id, ScopeId::from_raw(0, 0xAA));
            }
            other => panic!("expected Newtype identity, got {other:?}"),
        }
    }

    /// Stage 4: `Wrapped::summarize()` renders `Distance(<inner>)`, mirroring the
    /// surface-form invariant Struct / Tagged carriers honor.
    #[test]
    fn wrapped_summarize_renders_surface_form() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::model::types::Parseable;
        let arena = RuntimeArena::new();
        let inner = arena.alloc_object(KObject::Number(3.0));
        let type_id = arena.alloc_ktype(KType::UserType {
            kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
            scope_id: ScopeId::from_raw(0, 0xAA),
            name: "Distance".into(),
        });
        let w = KObject::Wrapped { inner, type_id };
        assert_eq!(w.summarize(), "Distance(3)");
    }

    /// Stage 4: `Wrapped::deep_clone()` copies both arena references without
    /// re-allocating. The cloned `inner` and `type_id` point at the same arena slots.
    #[test]
    fn wrapped_deep_clone_preserves_arena_references() {
        use crate::runtime::machine::RuntimeArena;
        let arena = RuntimeArena::new();
        let inner = arena.alloc_object(KObject::Number(3.0));
        let type_id = arena.alloc_ktype(KType::UserType {
            kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
            scope_id: ScopeId::from_raw(0, 0xAA),
            name: "Distance".into(),
        });
        let original = KObject::Wrapped { inner, type_id };
        let cloned = original.deep_clone();
        match cloned {
            KObject::Wrapped { inner: ci, type_id: ct } => {
                assert!(std::ptr::eq(ci, inner));
                assert!(std::ptr::eq(ct, type_id));
            }
            _ => panic!("expected Wrapped after deep_clone"),
        }
    }
}
