use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::parse::kexpression::{KExpression, TypeExpr};
use crate::dispatch::kfunction::KFunction;
use crate::dispatch::runtime::{CallArena, KFuture};
use crate::dispatch::types::{KType, Parseable, Serializable, SignatureElement};
use super::module::{Module, Signature};

/// Runtime value: scalars, collections, an unevaluated expression, a bound-but-unrun task, or a
/// reference to a function in some scope. The universal value type that `KFunction`s consume
/// and produce; implements `Parseable` so values can be compared and rendered uniformly.
///
/// `List(Rc<Vec<...>>)` / `Dict(Rc<HashMap<...>>)`: composite payloads are reference-counted
/// so deep_clone (and the scheduler's lift-on-return walk) can `Rc::clone` instead of
/// rebuilding. The contract is "Koan's collections are immutable values"; a future mutable-list
/// builtin would need `Rc::make_mut` at the mutation site to clone-on-write. Subsequent lifts
/// of the same value through a return chain are O(1) for these variants once any embedded
/// `KFunction` has had its `Rc<CallArena>` attached.
///
/// `KFunction(&fn, Option<Rc<CallArena>>)`: the second field keeps the function's underlying
/// per-call arena alive when the function escaped from a per-call body (via lift-on-return).
/// `None` for the common case (builtins and top-level FNs, which live in run-root and don't
/// need a refcount). `Some(rc)` is set by the scheduler's lift logic when it detects the
/// function lives in a dying frame's arena. The Rc keeps the arena alive past the slot's
/// frame drop, so the `&KFunction` reference stays valid.
///
/// `KFuture(KFuture, Option<Rc<CallArena>>)`: same lifecycle anchor as `KFunction`. The
/// inner `KFuture` carries a bare `&KFunction` plus a bundle and parsed `KExpression` whose
/// `Future` parts are also borrowed `&KObject`s — all of which can point into a per-call
/// arena. A single `Rc<CallArena>` keeps that arena alive, transitively keeping every
/// internal reference valid. Set by `lift_kobject` when a KFuture-as-value escapes.
///
/// `TaggedUnionType(Rc<HashMap<...>>)`: a first-class tagged-union schema, mapping tag
/// names to the `KType` each tag's payload must satisfy. Built by the `UNION` builtin and
/// consumed by `TAG` (and the surface-level type-token / call-by-name construction paths)
/// to validate tagged values at construction time. Reports `KType::Type` so it slots into
/// any "expects a type" position alongside `StructType`.
///
/// `StructType { name, fields }`: a first-class struct schema produced by the `STRUCT`
/// builtin. `fields` is an ordered `Vec<(String, KType)>` (not a HashMap) because struct
/// construction is positional — the declaration order is part of the contract. Reports
/// `KType::Type` like `TaggedUnionType`.
///
/// `Tagged { tag, value }`: a tagged value — one variant of a tagged union, carrying its
/// tag name and inner payload. The payload is `Rc`-shared like `List`/`Dict` to keep
/// `deep_clone` cheap and the lift-on-return walk able to skip allocation when no
/// descendant `KFunction` is in flight.
///
/// `Struct { type_name, fields }`: a runtime struct value — a record of named fields. The
/// `fields` map is `Rc`-shared with the same immutability contract as `Dict`/`List`. Stored
/// as an `IndexMap` so iteration order matches the schema's declaration order — PRINT and
/// `summarize` emit fields in the order the user wrote them, and `.get(name)` is still O(1).
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
    /// A first-class type expression — produced by `ExpressionPart::resolve_for` when the
    /// receiving slot is `KType::TypeExprRef`. The structured `TypeExpr` survives the parser
    /// → dispatch boundary intact so consumers like FN's return-type slot can recover the
    /// full parameterized form (`List<Number>`, `Function<(N) -> S>`, ...) rather than just
    /// the bare type name. Internal-only — no user-facing operation produces this variant.
    TypeExprValue(TypeExpr),
    /// First-class module value (module-system stage 1). Carries an arena-allocated
    /// [`Module`] reference whose `child_scope` points at the body scope `MODULE` populated
    /// during construction. Reports `KType::Module`. ATTR routes through this variant for
    /// member access (`Foo.bar`).
    KModule(&'a Module<'a>),
    /// First-class signature value (module-system stage 1). Holds the declaring scope so the
    /// ascription operators `:|` / `:!` can iterate declared abstract types and operation
    /// signatures. Reports `KType::Signature`.
    KSignature(&'a Signature<'a>),
    Null,
}

impl<'a> KObject<'a> {
    /// Runtime type tag for this value. Used by the scheduler's post-call return-type check
    /// (`KType::matches_value`) and any future static-pass tooling. `KFuture` reports as
    /// `KFunction` since a bound-but-unrun call is functionally a thunk and KFutures don't
    /// escape as user-visible values today.
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
            KObject::KModule(_) => KType::Module,
            KObject::KSignature(_) => KType::Signature,
        }
    }

    /// Independent-but-cheap clone. Composite payloads are `Rc`-shared (the immutability
    /// contract on the `List`/`Dict` variants makes structural sharing safe), so cloning
    /// those is `Rc::clone` rather than a recursive walk. `KFuture` and `KExpression` carry
    /// their own clone semantics; `KFunction` preserves its `Rc<CallArena>` (if any) so a
    /// kept-alive per-call arena stays alive in the clone. `Dict` keys are cloned through
    /// `Serializable::clone_box` only when the surrounding `Rc` is being rebuilt — that
    /// happens in `lift_kobject`, not here.
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
            KObject::KModule(m) => KObject::KModule(m),
            KObject::KSignature(s) => KObject::KSignature(s),
        }
    }

    // Accessor helpers ---------------------------------------------------------------------
    //
    // Each `as_*` returns `Option<&T>` (or `Option<T>` for the trivially-`Copy` scalars). The
    // pattern collapses the per-call-site `match` block — `match obj { KObject::Number(n) =>
    // Some(*n), _ => None }` — to one method call. `Option<&T>::is_some()` covers any need
    // for a boolean predicate, so no `is_*` helpers are introduced; reach for the accessor
    // and call `.is_some()` if you only need shape detection. None of the helpers `clone()` —
    // returns are by reference where the variant carries a non-`Copy` payload.

    /// `KObject::Number(n)` → `Some(n)`. Numbers are `Copy`, so the value is returned by-value.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            KObject::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// `KObject::KString(s)` → `Some(&s)`. Borrowed; clone at the call site if ownership is
    /// needed.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            KObject::KString(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `KObject::Bool(b)` → `Some(b)`. Booleans are `Copy`, so the value is returned by-value.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            KObject::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// `KObject::List(items)` → `Some(&items)`. Returns the `Rc` reference so callers can
    /// `Rc::clone` for cheap shared ownership without rebuilding.
    pub fn as_list(&self) -> Option<&Rc<Vec<KObject<'a>>>> {
        match self {
            KObject::List(items) => Some(items),
            _ => None,
        }
    }

    /// `KObject::KExpression(e)` → `Some(&e)`. Borrowed; clone at the call site if the slot
    /// needs to be threaded through a tail-emit or similar.
    pub fn as_kexpression(&self) -> Option<&KExpression<'a>> {
        match self {
            KObject::KExpression(e) => Some(e),
            _ => None,
        }
    }

    /// `KObject::KFunction(f, _)` → `Some(f)`. Drops the optional `Rc<CallArena>` (the
    /// arena anchor is only relevant to lift / clone paths; consumers reading the function
    /// don't need it). Returns the `&'a KFunction<'a>` directly so call sites keep arena
    /// lifetime threading.
    pub fn as_kfunction(&self) -> Option<&'a KFunction<'a>> {
        match self {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        }
    }

    /// `KObject::StructType { name, fields }` → `Some((&name, &fields))`. Returns the `Rc`
    /// reference to the field list so callers can `Rc::clone` rather than walk it. The tuple
    /// type is non-trivial because it mirrors the struct variant's payload exactly — clippy's
    /// `type_complexity` lint fires here, but introducing a named alias just for a single
    /// return type would obscure rather than clarify; suppress targeted.
    #[allow(clippy::type_complexity)]
    pub fn as_struct_type(&self) -> Option<(&str, &Rc<Vec<(String, KType)>>)> {
        match self {
            KObject::StructType { name, fields } => Some((name.as_str(), fields)),
            _ => None,
        }
    }

    /// `KObject::TaggedUnionType(schema)` → `Some(&schema)`. Returns the `Rc` reference for
    /// the same reason as `as_struct_type`.
    pub fn as_tagged_union_type(&self) -> Option<&Rc<HashMap<String, KType>>> {
        match self {
            KObject::TaggedUnionType(schema) => Some(schema),
            _ => None,
        }
    }

    /// `KObject::KModule(m)` → `Some(m)`. The `&'a Module<'a>` reference is returned
    /// directly so call sites don't have to redo the dereference dance.
    pub fn as_module(&self) -> Option<&'a Module<'a>> {
        match self {
            KObject::KModule(m) => Some(*m),
            _ => None,
        }
    }

    /// `KObject::KSignature(s)` → `Some(s)`. Mirrors [`as_module`](Self::as_module).
    pub fn as_signature(&self) -> Option<&'a Signature<'a>> {
        match self {
            KObject::KSignature(s) => Some(*s),
            _ => None,
        }
    }

    /// `KObject::TypeExprValue(t)` → `Some(&t)`. Borrowed; clone if the call site needs to
    /// own the structured `TypeExpr`.
    pub fn as_type_expr(&self) -> Option<&TypeExpr> {
        match self {
            KObject::TypeExprValue(t) => Some(t),
            _ => None,
        }
    }
}

/// Project a parameterized `KType::KFunction { args, ret }` from a function value's signature.
/// Used by `KObject::ktype` for both `KFunction` and `KFuture` — both report the same
/// structural type since a `KFuture` is a bound-but-unrun thunk over the same `KFunction`.
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
            KObject::KModule(m) => format!("module {}", m.path),
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
