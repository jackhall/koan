//! AST node types shared across the parse module. `KExpression` is a function-call node
//! (head plus ordered and named arguments). `ExpressionPart` is one element inside such
//! a call — atoms (literals, identifiers, types, keywords), collection literals (lists,
//! dicts), and nested expressions. `KLiteral` enumerates the concrete literal kinds the
//! lexer can produce. Produced by `tokens` and assembled into trees by `expression_tree`.

use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::kfunction::{UntypedElement, UntypedKey};
use crate::dispatch::kkey::KKey;
use crate::dispatch::kobject::KObject;
use crate::dispatch::ktraits::{Parseable, Executable, Serializable};

/// Concrete literal kinds the parser recognizes; produced by `tokens::try_literal` and consumed
/// when resolving an `ExpressionPart` into a runtime `KObject`.
#[derive(Debug, Clone)]
pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

/// Surface representation of a type token. Leaf types like `Number` carry `TypeParams::None`;
/// container types like `List<Number>` and `Function<A, B -> R>` carry their inner types in
/// the structured `TypeParams` variant. Built by `expression_tree::build_tree`'s `Frame::Type`
/// when a `<...>` group closes, and consumed by the KType-construction layer
/// (`KType::from_type_expr`) at signature-parse time.
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
    /// Comma/whitespace-separated list of params: `List<X>`, `Dict<K, V>`. Arity validation
    /// (e.g. `List` needs exactly one) lives at the KType-construction layer, not here.
    List(Vec<TypeExpr>),
    /// `Function<A, B, ... -> R>` — the `->` arrow distinguishes args from return type.
    Function { args: Vec<TypeExpr>, ret: Box<TypeExpr> },
}

impl TypeExpr {
    /// Bare leaf — no parameters. Used by `tokens::classify_atom` for plain type tokens.
    pub fn leaf(name: String) -> TypeExpr {
        TypeExpr { name, params: TypeParams::None }
    }

    /// Render this type expression in the surface syntax (`List<Number>`, `Function<-> R>`).
    /// Used by `ExpressionPart::summarize` and Display so error messages and debug output
    /// read naturally.
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

/// One element of a parsed expression. The parser classifies each source token into one of:
/// `Keyword` (all-caps fixed tokens like `LET`/`=`/`THEN`; see `is_keyword_token`), `Type`
/// (capitalized type names like `Number`/`MyType`/`KFunction` — first char uppercase plus at
/// least one lowercase), or `Identifier` (everything else: lowercase/snake names). `Expression`,
/// `ListLiteral`, and `Literal` are the other parser outputs; the scheduler introduces `Future`
/// later, splicing a completed dep's result into its dependent's parts list before late dispatch.
pub enum ExpressionPart<'a> {
    /// Fixed token consumed by a `SignatureElement::Token` slot at dispatch time. Contributes
    /// `UntypedElement::Keyword(s)` to the bucket key.
    Keyword(String),
    /// Name slot bound to an `Argument` whose `KType` is `Identifier` or `Any`. Contributes
    /// `UntypedElement::Slot` to the bucket key — same shape as a literal or expression slot.
    Identifier(String),
    /// A type-name reference like `Number`, `KFunction`, or `List<Number>`. Used in surface
    /// positions that name a type (e.g. the return-type slot of `FN (sig) -> Type = (body)`).
    /// Contributes `UntypedElement::Slot` to the bucket key; an `Argument` whose `ktype` is
    /// `KType::TypeRef` matches this part. The `TypeExpr` carries any nested parameters
    /// (`List<Number>` etc.); leaf types use `TypeParams::None`.
    Type(TypeExpr),
    Expression(Box<KExpression<'a>>),
    /// A `[a b c]` source-level list. Each element is itself an `ExpressionPart`; sub-expression
    /// elements (`ExpressionPart::Expression`) are scheduled as deps and replaced with `Future`s
    /// before the parent is dispatched. The whole literal resolves to `KObject::List` at
    /// `resolve()` time.
    ListLiteral(Vec<ExpressionPart<'a>>),
    /// A `{k: v, ...}` source-level dict. Each pair holds two `ExpressionPart`s; sub-expression
    /// or bare-identifier sides are scheduled by the scheduler (mirroring `ListLiteral`'s path)
    /// and the result materializes to `KObject::Dict`. Bare-identifier keys/values are wrapped
    /// in a sub-`Dispatch` so they resolve via `value_lookup` (Python-like name resolution for
    /// keys, a small extra wrapping cost for values that pays for itself in consistency).
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
    /// `KExpression::summarize`. Used by error reporting (`KError::TypeMismatch.got` and
    /// `Frame::expression`) to name an offending part without dragging in `Parseable`.
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
            // The scheduler ordinarily replaces sub-expression elements with `Future`s before
            // this runs (see `schedule_list_literal`); a raw `Expression` element here would
            // round-trip through `KExpression` rather than its computed value.
            ExpressionPart::ListLiteral(items) => {
                KObject::List(Rc::new(items.iter().map(|p| p.resolve()).collect()))
            }
            // The scheduler ordinarily replaces sub-expression and bare-identifier dict
            // entries with resolved values via `schedule_dict_literal` before this runs (see
            // `Scheduler::schedule_dict_literal`); a raw non-scalar reaching here would
            // fail the scalar-key conversion. Materialize what we can: each key part
            // resolves to a `KObject` and is converted to a `KKey`. Panics if a key isn't a
            // scalar — the scheduler is responsible for catching that earlier with a
            // structured `ShapeError`.
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
            // Preserve compound shapes (List, KExpression) by deep-cloning rather than
            // stringifying — a Future-borne List or KExpression must materialize back to its
            // structured form.
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
            ExpressionPart::Future(o) => ExpressionPart::Future(*o),
        }
    }
}

impl<'a> Clone for KExpression<'a> {
    fn clone(&self) -> Self {
        KExpression { parts: self.parts.clone() }
    }
}

/// A parsed Koan expression: an ordered sequence of `ExpressionPart`s. The output of the parse
/// pipeline and the input to `Scope::dispatch`, which matches it against function signatures.
pub struct KExpression<'a> {
    pub parts: Vec<ExpressionPart<'a>>,
}

impl<'a> KExpression<'a> {
    /// Bucket key for this expression: `Keyword` parts contribute `Keyword(s)`; everything else
    /// (identifiers, literals, sub-expressions, list literals, futures) contributes `Slot`.
    /// Must agree with `ExpressionSignature::untyped_key` for any signature that should match —
    /// the parser classifies tokens via `is_keyword_token` up front so this is a direct lookup.
    pub fn untyped_key(&self) -> UntypedKey {
        self.parts
            .iter()
            .map(|part| match part {
                ExpressionPart::Keyword(s) => UntypedElement::Keyword(s.clone()),
                _ => UntypedElement::Slot,
            })
            .collect()
    }

}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression").field("parts", &self.parts).finish()
    }
}

impl<'a> Parseable for KExpression<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool { self.summarize() == other.summarize() }
    fn summarize(&self) -> String {
        self.parts.iter()
            .map(|p| p.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl<'a> Executable for KExpression<'a> {
    fn execute(&self, _args: &[&dyn Parseable]) -> Box<dyn Parseable> {
        Box::new(KObject::KString(self.summarize()))
    }
}
