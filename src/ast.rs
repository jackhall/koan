//! AST node types shared across the parse module. `KExpression` is a function-call node;
//! `ExpressionPart` is one element inside such a call — atoms (literals, identifiers, types,
//! keywords), collection literals (lists, dicts), and nested expressions. `KLiteral`
//! enumerates the concrete literal kinds the lexer can produce.

use std::collections::HashMap;
use std::rc::Rc;

use crate::runtime::model::{KKey, KObject, Parseable, Serializable, UntypedElement, UntypedKey};

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
#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub name: String,
    pub params: TypeParams,
}

/// Inner-type carrier on a `TypeExpr`. The variant split bakes the `Function` arrow rule
/// into the *shape* — downstream consumers don't have to know that `Function` is special.
#[derive(Debug, Clone)]
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
        TypeExpr { name, params: TypeParams::None }
    }

    /// Render in surface syntax (`List<Number>`, `Function<(A) -> R>`).
    pub fn render(&self) -> String {
        match &self.params {
            TypeParams::None => self.name.clone(),
            TypeParams::List(items) => {
                let inner: Vec<String> = items.iter().map(|t| t.render()).collect();
                format!("{}<{}>", self.name, inner.join(", "))
            }
            TypeParams::Function { args, ret } => {
                let inner: Vec<String> = args.iter().map(|t| t.render()).collect();
                format!("{}<({}) -> {}>", self.name, inner.join(", "), ret.render())
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
    /// receiving slot is `KType::TypeExprRef`, the structured `TypeExpr` is preserved as a
    /// `KObject::TypeExprValue` rather than flattened to a name string, so parameterized
    /// types like `List<Number>` survive into the binding.
    pub fn resolve_for(&self, slot: &crate::runtime::model::KType) -> KObject<'a> {
        if let (ExpressionPart::Type(t), crate::runtime::model::KType::TypeExprRef) =
            (self, slot)
        {
            return KObject::TypeExprValue(t.clone());
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
                        panic!("DictLiteral::resolve: non-scalar key reached resolve(): {e}")
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
    /// error (see [`crate::runtime::machine::kfunction::PreRunFn`]).
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
    fn ktype(&self) -> crate::runtime::model::KType { crate::runtime::model::KType::KExpression }
    fn summarize(&self) -> String {
        self.parts.iter()
            .map(|p| p.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

