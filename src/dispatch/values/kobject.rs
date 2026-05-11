use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::parse::{KExpression, TypeExpr};
use crate::dispatch::kfunction::KFunction;
use crate::dispatch::runtime::{CallArena, KFuture};
use crate::dispatch::types::{KType, Parseable, Serializable, SignatureElement};
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
/// [memory-model.md § Closure escape](../../../design/memory-model.md#closure-escape-per-call-arenas--rc).
pub enum KObject<'a> {
    Number(f64),
    KString(String),
    Bool(bool),
    List(Rc<Vec<KObject<'a>>>),
    Dict(Rc<HashMap<Box<dyn Serializable + 'a>, KObject<'a>>>),
    KExpression(KExpression<'a>),
    KFuture(KFuture<'a>, Option<Rc<CallArena>>),
    KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>),
    TaggedUnionType(Rc<HashMap<String, KType>>),
    StructType {
        name: String,
        fields: Rc<Vec<(String, KType)>>,
    },
    Tagged {
        tag: String,
        value: Rc<KObject<'a>>,
    },
    Struct {
        type_name: String,
        fields: Rc<IndexMap<String, KObject<'a>>>,
    },
    /// First-class type expression: preserves the structured `TypeExpr` across the parser→
    /// dispatch boundary so consumers like FN's return-type slot recover the full
    /// parameterized form rather than just a bare type name. Internal-only.
    TypeExprValue(TypeExpr),
    /// `Option<Rc<CallArena>>` mirrors `KFunction`'s lifecycle anchor: a `Module` whose
    /// child scope was alloc'd inside a per-call frame (a functor body's freshly-built
    /// `MODULE Result = (...)`) carries the frame's `Rc` so the captured scope outlives
    /// the dying frame. `None` for modules built outside a per-call frame (top-level
    /// `MODULE Foo = (...)` and the ascription paths). See [memory-model.md § Closure
    /// escape](../../../design/memory-model.md#closure-escape-per-call-arenas--rc).
    KModule(&'a Module<'a>, Option<Rc<CallArena>>),
    KSignature(&'a Signature<'a>),
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
            KObject::TaggedUnionType(_) => KType::Type,
            KObject::StructType { .. } => KType::Type,
            KObject::Tagged { .. } => KType::Tagged,
            KObject::Struct { .. } => KType::Struct,
            KObject::TypeExprValue(_) => KType::TypeExprRef,
            KObject::KModule(_, _) => KType::Module,
            KObject::KSignature(_) => KType::Signature,
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
            KObject::TaggedUnionType(schema) => KObject::TaggedUnionType(Rc::clone(schema)),
            KObject::StructType { name, fields } => KObject::StructType {
                name: name.clone(),
                fields: Rc::clone(fields),
            },
            KObject::Tagged { tag, value } => KObject::Tagged {
                tag: tag.clone(),
                value: Rc::clone(value),
            },
            KObject::Struct { type_name, fields } => KObject::Struct {
                type_name: type_name.clone(),
                fields: Rc::clone(fields),
            },
            KObject::TypeExprValue(t) => KObject::TypeExprValue(t.clone()),
            KObject::KModule(m, frame) => KObject::KModule(m, frame.clone()),
            KObject::KSignature(s) => KObject::KSignature(s),
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
            KObject::StructType { name, fields } => Some((name.as_str(), fields)),
            _ => None,
        }
    }

    pub fn as_tagged_union_type(&self) -> Option<&Rc<HashMap<String, KType>>> {
        match self {
            KObject::TaggedUnionType(schema) => Some(schema),
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

    pub fn as_type_expr(&self) -> Option<&TypeExpr> {
        match self {
            KObject::TypeExprValue(t) => Some(t),
            _ => None,
        }
    }
}

fn function_value_ktype<'a>(f: &KFunction<'a>) -> KType {
    let args: Vec<KType> = f
        .signature
        .elements
        .iter()
        .filter_map(|el| match el {
            SignatureElement::Argument(a) => Some(a.ktype.clone()),
            _ => None,
        })
        .collect();
    let ret = Box::new(f.signature.return_type.clone());
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
            KObject::TaggedUnionType(schema) => {
                let parts: Vec<String> = schema
                    .iter()
                    .map(|(tag, ktype)| format!("{}: {}", tag, ktype.name()))
                    .collect();
                format!("Union{{{}}}", parts.join(", "))
            }
            KObject::StructType { name, fields } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, ktype)| format!("{}: {}", field, ktype.name()))
                    .collect();
                format!("{}{{{}}}", name, parts.join(", "))
            }
            KObject::Tagged { tag, value } => format!("{}({})", tag, value.summarize()),
            KObject::Struct { type_name, fields } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{}: {}", field, value.summarize()))
                    .collect();
                format!("{}({})", type_name, parts.join(", "))
            }
            KObject::Null => "null".to_string(),
            KObject::TypeExprValue(t) => t.render(),
            KObject::KModule(m, _) => format!("module {}", m.path),
            KObject::KSignature(s) => format!("sig {}", s.path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::values::KKey;
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
}
