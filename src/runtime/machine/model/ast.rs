//! AST node types shared across the parse module. `KExpression` is a function-call node;
//! `ExpressionPart` is one element inside such a call — atoms (literals, identifiers, types,
//! keywords), collection literals (lists, dicts), and nested expressions. `KLiteral`
//! enumerates the concrete literal kinds the lexer can produce.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::runtime::machine::model::types::KType;
use crate::runtime::machine::model::{KKey, KObject, Parseable, Serializable, UntypedElement, UntypedKey};

#[derive(Debug, Clone)]
pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

/// Surface representation of a type token. Leaf types like `Number` carry `TypeParams::None`;
/// container types like `List<Number>` and `Function<A, B -> R>` carry their inner types in
/// the structured `TypeParams` variant.
///
/// `builtin_cache` is the Layer-1 resolution cache: when
/// [`crate::runtime::machine::model::types::KType::from_type_expr`] succeeds against
/// the builtin table, the resulting `KType` is stored here so subsequent
/// `resolve_for` calls skip the recursive walk. Scope-independent — the result depends
/// only on the surface form. `from_type_expr` failures (user-bound names) are not
/// cached here; those route through the scope-owned Layer-2 memo on `Scope`.
#[derive(Debug)]
pub struct TypeExpr {
    pub name: String,
    pub params: TypeParams,
    pub builtin_cache: OnceCell<KType>,
}

impl Clone for TypeExpr {
    fn clone(&self) -> Self {
        let cache = OnceCell::new();
        if let Some(kt) = self.builtin_cache.get() {
            let _ = cache.set(kt.clone());
        }
        TypeExpr {
            name: self.name.clone(),
            params: self.params.clone(),
            builtin_cache: cache,
        }
    }
}

impl PartialEq for TypeExpr {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.params == other.params
    }
}

impl Eq for TypeExpr {}

impl std::hash::Hash for TypeExpr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.params.hash(state);
    }
}

/// Inner-type carrier on a `TypeExpr`. The variant split bakes the `Function` arrow rule
/// into the *shape* — downstream consumers don't have to know that `Function` is special.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeParams {
    /// Leaf type (`Number`, `Str`, `Any`) or an unparameterized container (`List`).
    None,
    /// `List<X>`, `Dict<K, V>`. Arity validation lives at the KType-construction layer.
    List(Vec<TypeExpr>),
    /// `Function<A, B, ... -> R>` — the `->` arrow distinguishes args from return type.
    Function { args: Vec<TypeExpr>, ret: Box<TypeExpr> },
}

impl TypeExpr {
    pub fn leaf(name: String) -> TypeExpr {
        TypeExpr {
            name,
            params: TypeParams::None,
            builtin_cache: OnceCell::new(),
        }
    }

    /// Render in surface syntax — Design-B sigil form. Leaves render bare (`Number`);
    /// parameterized types render with the `:(...)` sigil so the output round-trips
    /// through the parser unchanged: `:(List Number)`, `:(Function (A) -> R)`.
    pub fn render(&self) -> String {
        match &self.params {
            TypeParams::None => self.name.clone(),
            TypeParams::List(items) => {
                let inner: Vec<String> = items.iter().map(|t| t.render()).collect();
                format!(":({} {})", self.name, inner.join(" "))
            }
            TypeParams::Function { args, ret } => {
                let inner: Vec<String> = args.iter().map(|t| t.render()).collect();
                format!(":({} ({}) -> {})", self.name, inner.join(" "), ret.render())
            }
        }
    }
}

/// One element of a parsed expression. Parser outputs are `Keyword`, `Identifier`, `Type`,
/// `Expression`, `ListLiteral`, `DictLiteral`, and `Literal`; the scheduler introduces
/// `Future` later, splicing a completed dep's resolved value into its dependent's parts
/// list before late dispatch.
pub enum ExpressionPart<'a> {
    Keyword(String),
    Identifier(String),
    /// A type-name reference like `Number`, `KFunction`, or `List<Number>`. The `TypeExpr`
    /// carries any nested parameters; leaf types use `TypeParams::None`.
    Type(TypeExpr),
    Expression(Box<KExpression<'a>>),
    ListLiteral(Vec<ExpressionPart<'a>>),
    DictLiteral(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    Literal(KLiteral),
    Future(&'a KObject<'a>),
}

impl<'a> std::fmt::Debug for ExpressionPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressionPart::Keyword(s) => f.debug_tuple("Keyword").field(s).finish(),
            ExpressionPart::Identifier(s) => f.debug_tuple("Identifier").field(s).finish(),
            ExpressionPart::Type(t) => f.debug_tuple("Type").field(t).finish(),
            ExpressionPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
            ExpressionPart::ListLiteral(items) => f.debug_tuple("ListLiteral").field(items).finish(),
            ExpressionPart::DictLiteral(pairs) => f.debug_tuple("DictLiteral").field(pairs).finish(),
            ExpressionPart::Literal(l) => f.debug_tuple("Literal").field(l).finish(),
            ExpressionPart::Future(obj) => write!(f, "Future({})", obj.summarize()),
        }
    }
}

impl<'a> ExpressionPart<'a> {
    pub fn expression(parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
        ExpressionPart::Expression(Box::new(KExpression { parts }))
    }

    /// Short textual rendering of this part, matching the per-part subset of
    /// `KExpression::summarize`.
    pub fn summarize(&self) -> String {
        match self {
            ExpressionPart::Keyword(s) => s.clone(),
            ExpressionPart::Identifier(s) => s.clone(),
            ExpressionPart::Type(t) => t.render(),
            ExpressionPart::Expression(e) => e.summarize(),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| p.summarize()).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(lit) => match lit {
                KLiteral::Number(n) => n.to_string(),
                KLiteral::String(s) => s.clone(),
                KLiteral::Boolean(b) => b.to_string(),
                KLiteral::Null => "null".to_string(),
            },
            ExpressionPart::Future(obj) => obj.summarize(),
        }
    }

    /// Slot-aware resolve. Identical to `resolve` for every variant except `Type`: when the
    /// receiving slot is `KType::TypeExprRef`, the surface `TypeExpr` is lowered into a
    /// runtime carrier so consumers downstream operate on a unified value representation
    /// rather than parser surface syntax.
    ///
    /// Two routes:
    /// - When [`crate::runtime::machine::model::types::KType::from_type_expr`] succeeds (builtin
    ///   leaf names like `Number`, or structural shapes like `List<Number>` /
    ///   `Function<(...)-> R>`), the result is packaged as `KObject::KTypeValue(kt)`.
    /// - When `from_type_expr` returns `Err` — i.e. a bare-leaf name that isn't a
    ///   builtin (`Point`, `IntOrd`, `MyList`) — the carrier becomes a
    ///   `KObject::TypeNameRef(t)` preserving the parser-side `TypeExpr`. The
    ///   consuming body (`extract_bare_type_name`, ATTR's TypeExprRef lhs, FN's
    ///   deferred return-type elaboration, `LET <Type-class> = …`) reads the surface
    ///   name directly off the carrier or — when scope-aware elaboration is needed —
    ///   calls [`crate::runtime::machine::core::Scope::resolve_type_expr`] which
    ///   memoizes the resolution per-scope.
    ///
    /// The carrier shape is required because `resolve_for` runs at `KFunction::bind`
    /// time — before any body sees the slot — and the bind-time pass has no `Scope`
    /// reference in hand. The earlier transitional `KType::Unresolved` carrier
    /// (deleted in stage 2) routed the surface name through the elaborated-type
    /// language; `TypeNameRef` keeps it on the value side where the rest of the
    /// bind-time surface-name plumbing lives.
    pub fn resolve_for(&self, slot: &crate::runtime::machine::model::KType) -> KObject<'a> {
        use crate::runtime::machine::model::types::KType;
        if let (ExpressionPart::Type(t), KType::TypeExprRef) = (self, slot) {
            // Layer-1 cache: builtin-only resolution is surface-form-only, so the
            // result is invariant across dispatches against this same `TypeExpr`.
            if let Some(kt) = t.builtin_cache.get() {
                return KObject::KTypeValue(kt.clone());
            }
            return match KType::from_type_expr(t) {
                Ok(kt) => {
                    let _ = t.builtin_cache.set(kt.clone());
                    KObject::KTypeValue(kt)
                }
                Err(_) => KObject::TypeNameRef(t.clone()),
            };
        }
        self.resolve()
    }

    pub fn resolve(&self) -> KObject<'a> {
        match self {
            ExpressionPart::Keyword(s) => KObject::KString(s.clone()),
            ExpressionPart::Identifier(s) => KObject::KString(s.clone()),
            ExpressionPart::Type(t) => KObject::KString(t.render()),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression((**e).clone()),
            // Sub-expression elements should already have been replaced with `Future`s by
            // the scheduler; a raw `Expression` here round-trips as a `KExpression` value
            // rather than its computed result.
            ExpressionPart::ListLiteral(items) => {
                KObject::List(Rc::new(items.iter().map(|p| p.resolve()).collect()))
            }
            // Sub-expression and bare-identifier dict entries should already have been
            // resolved by the scheduler. Non-scalar keys reaching here are a scheduler bug
            // — it's responsible for surfacing them as a structured `ShapeError` earlier.
            ExpressionPart::DictLiteral(pairs) => {
                let mut map: HashMap<Box<dyn Serializable + 'a>, KObject<'a>> = HashMap::new();
                for (k, v) in pairs {
                    let key_obj = k.resolve();
                    let kkey = KKey::try_from_kobject(&key_obj).unwrap_or_else(|e| {
                        panic!("DictLiteral::resolve = non-scalar key reached resolve(): {e}")
                    });
                    map.insert(Box::new(kkey), v.resolve());
                }
                KObject::Dict(Rc::new(map))
            }
            // Deep-clone, don't stringify: a Future-borne List or KExpression must
            // materialize back to its structured form.
            ExpressionPart::Future(obj) => obj.deep_clone(),
        }
    }
}

impl<'a> Clone for ExpressionPart<'a> {
    fn clone(&self) -> Self {
        match self {
            ExpressionPart::Keyword(s) => ExpressionPart::Keyword(s.clone()),
            ExpressionPart::Identifier(s) => ExpressionPart::Identifier(s.clone()),
            ExpressionPart::Type(t) => ExpressionPart::Type(t.clone()),
            ExpressionPart::Expression(e) => ExpressionPart::Expression(e.clone()),
            ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(items.clone()),
            ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(pairs.clone()),
            ExpressionPart::Literal(l) => ExpressionPart::Literal(l.clone()),
            ExpressionPart::Future(o) => ExpressionPart::Future(o),
        }
    }
}

impl<'a> Clone for KExpression<'a> {
    fn clone(&self) -> Self {
        KExpression { parts: self.parts.clone() }
    }
}

/// A parsed Koan expression: an ordered sequence of `ExpressionPart`s.
pub struct KExpression<'a> {
    pub parts: Vec<ExpressionPart<'a>>,
}

impl<'a> KExpression<'a> {
    /// Bucket key: `Keyword` parts contribute `Keyword(s)`; every other variant contributes
    /// `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature that
    /// should match.
    pub fn untyped_key(&self) -> UntypedKey {
        self.parts
            .iter()
            .map(|part| match part {
                ExpressionPart::Keyword(s) => UntypedElement::Keyword(s.clone()),
                _ => UntypedElement::Slot,
            })
            .collect()
    }

    /// If `parts[1]` is a single `Type(t)` token, return its bare name. Used as the
    /// dispatch-time placeholder extractor for typed-binder builtins (STRUCT, UNION,
    /// MODULE, SIG) whose surface form is `<KEYWORD> <Name> = (<body>)`. Returns `None`
    /// on shape mismatch; the builtin body is responsible for surfacing the structured
    /// error (see [`crate::runtime::machine::core::kfunction::PreRunFn`]).
    pub fn binder_name_from_type_part(&self) -> Option<String> {
        match self.parts.get(1)? {
            ExpressionPart::Type(t) => Some(t.name.clone()),
            _ => None,
        }
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression").field("parts", &self.parts).finish()
    }
}

impl<'a> Parseable for KExpression<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool { self.summarize() == other.summarize() }
    fn ktype(&self) -> crate::runtime::machine::model::KType { crate::runtime::machine::model::KType::KExpression }
    fn summarize(&self) -> String {
        self.parts.iter()
            .map(|p| p.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::runtime::machine::model::types::KType;

    /// Layer-1 cache: a builtin `TypeExpr` populates `builtin_cache` on first
    /// `resolve_for` and re-uses the cached `KType` on subsequent calls.
    #[test]
    fn resolve_for_populates_builtin_cache() {
        let part: ExpressionPart<'static> = ExpressionPart::Type(TypeExpr::leaf("Number".into()));
        let slot = KType::TypeExprRef;
        let _ = part.resolve_for(&slot);
        if let ExpressionPart::Type(t) = &part {
            assert_eq!(t.builtin_cache.get(), Some(&KType::Number));
        } else {
            panic!("expected Type part");
        }
        // Second call returns the cached value without re-walking.
        let r2 = part.resolve_for(&slot);
        match r2 {
            KObject::KTypeValue(kt) => assert_eq!(kt, KType::Number),
            _ => panic!("expected KTypeValue"),
        }
    }

    /// Layer-1 cache does NOT cache user-bound names: a leaf not in the builtin
    /// table produces a `TypeNameRef` carrier and `builtin_cache` remains empty.
    #[test]
    fn resolve_for_skips_cache_for_user_bound_leaf() {
        let part: ExpressionPart<'static> = ExpressionPart::Type(TypeExpr::leaf("MyType".into()));
        let slot = KType::TypeExprRef;
        let r = part.resolve_for(&slot);
        assert!(matches!(r, KObject::TypeNameRef(_)));
        if let ExpressionPart::Type(t) = &part {
            assert!(t.builtin_cache.get().is_none());
        } else {
            panic!("expected Type part");
        }
    }
}

