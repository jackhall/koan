//! AST node types shared across the parse module.

use std::collections::HashMap;

use crate::machine::core::source::{FileId, Span, Spanned};
use crate::machine::model::{
    KKey, KObject, Parseable, Record, Serializable, UntypedElement, UntypedKey,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

/// Surface representation of a bare type-name leaf (`Number`, `Point`, `T`, `Mo.Ty`).
///
/// A thin newtype over the source name: `Deref`s to `str`, derives eq/hash by string.
/// Compound shapes (`:(LIST OF X)`, `:(FN … -> …)`) are dispatch expressions, not
/// `TypeName` structure, so this carries no information the name string wouldn't. The
/// position tag rides on the carrier variant (`ExpressionPart::Type`,
/// `KObject::TypeNameRef`), not on this struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeName(String);

impl std::ops::Deref for TypeName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl TypeName {
    pub fn leaf(name: String) -> TypeName {
        TypeName(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Render in surface syntax so the output round-trips through the parser unchanged.
    pub fn render(&self) -> String {
        self.0.clone()
    }
}

/// One element of a parsed expression. `Future` is introduced by the scheduler when it
/// splices a completed dep's resolved value into its dependent's parts list.
pub enum ExpressionPart<'a> {
    Keyword(String),
    Identifier(String),
    Type(TypeName),
    Expression(Box<KExpression<'a>>),
    /// Parse-context marker for a `:(...)` group: the wrapped `KExpression` must dispatch
    /// in type-context, returning a type-side carrier. Shape recognition is the
    /// dispatcher's responsibility — the parser does no folding here. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    SigiledTypeExpr(Box<KExpression<'a>>),
    ListLiteral(Vec<ExpressionPart<'a>>),
    DictLiteral(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    /// Anonymous record literal (`{x = 1, y = "a"}`) — identifier-keyed `=` pairs. The
    /// brace frame routes here when the first pair separator is `=`; `:` pairs stay a
    /// `DictLiteral`. Field names are syntactic identifiers (never name-resolved).
    RecordLiteral(Vec<(String, ExpressionPart<'a>)>),
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
            ExpressionPart::SigiledTypeExpr(e) => {
                f.debug_tuple("SigiledTypeExpr").field(e).finish()
            }
            ExpressionPart::ListLiteral(items) => {
                f.debug_tuple("ListLiteral").field(items).finish()
            }
            ExpressionPart::DictLiteral(pairs) => {
                f.debug_tuple("DictLiteral").field(pairs).finish()
            }
            ExpressionPart::RecordLiteral(pairs) => {
                f.debug_tuple("RecordLiteral").field(pairs).finish()
            }
            ExpressionPart::Literal(l) => f.debug_tuple("Literal").field(l).finish(),
            ExpressionPart::Future(obj) => write!(f, "Future({})", obj.summarize()),
        }
    }
}

impl<'a> ExpressionPart<'a> {
    pub fn expression(parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
        ExpressionPart::Expression(Box::new(KExpression::new(
            parts.into_iter().map(Spanned::bare).collect(),
        )))
    }

    /// Per-part subset of `KExpression::summarize`.
    pub fn summarize(&self) -> String {
        match self {
            ExpressionPart::Keyword(s) => s.clone(),
            ExpressionPart::Identifier(s) => s.clone(),
            ExpressionPart::Type(t) => t.render(),
            ExpressionPart::Expression(e) => e.summarize(),
            ExpressionPart::SigiledTypeExpr(e) => format!(":({})", e.summarize()),
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
            ExpressionPart::RecordLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{} = {}", k, v.summarize()))
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

    /// Slot-aware resolve. Identical to `resolve` except for `Type` into a `TypeExprRef`
    /// slot: builtin shapes (`Number`, `List<Number>`, `Function<...>`) lower to
    /// `KObject::KTypeValue`; bare user names lower to `KObject::TypeNameRef`, deferring
    /// scope-aware elaboration to consumers via `Scope::resolve_type_expr`. Runs at
    /// `KFunction::bind` time, which has no `Scope` in hand.
    pub fn resolve_for(&self, slot: &crate::machine::model::KType<'a>) -> KObject<'a> {
        use crate::machine::model::types::KType;
        if let (ExpressionPart::Type(t), KType::TypeExprRef) = (self, slot) {
            // Builtin shapes lower directly at the caller's lifetime; bare user names
            // defer to `Scope::resolve_type_expr` via the `TypeNameRef` carrier.
            return match KType::<'a>::from_type_expr(t) {
                Ok(kt) => KObject::KTypeValue(kt),
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
            // Every SigiledTypeExpr must reach a value either through the dispatcher's
            // fast lane or via sub-Dispatch — both unwrap it preserving the type-context
            // marker. Reaching `resolve()` means a builtin grabbed the raw part and lost
            // that marker.
            ExpressionPart::SigiledTypeExpr(_) => {
                unreachable!("SigiledTypeExpr only valid in type-context dispatch")
            }
            ExpressionPart::ListLiteral(items) => {
                KObject::list(items.iter().map(|p| p.resolve()).collect())
            }
            // Non-scalar keys reaching here are a scheduler bug — it must surface them as
            // a structured `ShapeError` before resolve.
            ExpressionPart::DictLiteral(pairs) => {
                let mut map: HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>> = HashMap::new();
                for (k, v) in pairs {
                    let key_obj = k.resolve();
                    let kkey = KKey::try_from_kobject(&key_obj).unwrap_or_else(|e| {
                        panic!("DictLiteral::resolve = non-scalar key reached resolve(): {e}")
                    });
                    map.insert(Box::new(kkey), v.resolve());
                }
                KObject::dict(map)
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let fields: Record<KObject<'a>> = pairs
                    .iter()
                    .map(|(name, v)| (name.clone(), v.resolve()))
                    .collect();
                KObject::record(fields)
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
            ExpressionPart::SigiledTypeExpr(e) => ExpressionPart::SigiledTypeExpr(e.clone()),
            ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(items.clone()),
            ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(pairs.clone()),
            ExpressionPart::RecordLiteral(pairs) => ExpressionPart::RecordLiteral(pairs.clone()),
            ExpressionPart::Literal(l) => ExpressionPart::Literal(l.clone()),
            ExpressionPart::Future(o) => ExpressionPart::Future(o),
        }
    }
}

impl<'a> Clone for KExpression<'a> {
    fn clone(&self) -> Self {
        KExpression {
            parts: self.parts.clone(),
            span: self.span,
            file: self.file,
        }
    }
}

/// A parsed Koan expression: an ordered sequence of `ExpressionPart`s.
///
/// `span` and `file` are `None` for hand-built ASTs.
pub struct KExpression<'a> {
    pub parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub span: Option<Span>,
    pub file: Option<FileId>,
}

impl<'a> KExpression<'a> {
    /// Spanless constructor; `span`/`file` populated by later phases.
    pub fn new(parts: Vec<Spanned<ExpressionPart<'a>>>) -> Self {
        KExpression {
            parts,
            span: None,
            file: None,
        }
    }

    /// Bucket key: `Keyword` parts contribute `Keyword(s)`; every other variant contributes
    /// `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature that
    /// should match.
    pub fn untyped_key(&self) -> UntypedKey {
        self.parts
            .iter()
            .map(|part| match &part.value {
                ExpressionPart::Keyword(s) => UntypedElement::Keyword(s.clone()),
                _ => UntypedElement::Slot,
            })
            .collect()
    }

    /// Dispatch-time placeholder extractor for typed-binder builtins (`STRUCT <Name> = …`):
    /// if `parts[1]` is a single `Type(t)`, returns its bare name; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<String> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        }
    }

    /// If every part is `Expression(_)`, return refs to the inner expressions; otherwise
    /// `None`. The returned `Vec` encodes the all-`Expression` shape — callers iterate
    /// `&KExpression` directly without re-matching the variant.
    pub fn borrow_inner_expressions(&self) -> Option<Vec<&KExpression<'a>>> {
        let mut out = Vec::with_capacity(self.parts.len());
        for p in &self.parts {
            match &p.value {
                ExpressionPart::Expression(b) => out.push(b.as_ref()),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Consuming right-fold counterpart of [`Self::borrow_inner_expressions`]: returns
    /// `(preceding, last)` with both unwrapped from `ExpressionPart::Expression`. On any
    /// shape mismatch returns `self` back so the caller can pass through.
    pub fn try_take_inner_expressions_split(
        self,
    ) -> Result<(Vec<KExpression<'a>>, KExpression<'a>), Self> {
        let mut iter = self.parts.into_iter();
        let Some(first) = iter.next() else {
            return Err(KExpression::new(Vec::new()));
        };
        let mut last: KExpression<'a> = match first.value {
            ExpressionPart::Expression(b) => *b,
            other => {
                let mut parts = vec![Spanned {
                    value: other,
                    span: first.span,
                }];
                parts.extend(iter);
                return Err(KExpression::new(parts));
            }
        };
        let mut preceding: Vec<KExpression<'a>> = Vec::new();
        for p in iter.by_ref() {
            match p.value {
                ExpressionPart::Expression(b) => {
                    preceding.push(std::mem::replace(&mut last, *b));
                }
                other => {
                    let mut parts: Vec<Spanned<ExpressionPart<'a>>> = preceding
                        .into_iter()
                        .map(|e| Spanned::bare(ExpressionPart::Expression(Box::new(e))))
                        .collect();
                    parts.push(Spanned::bare(ExpressionPart::Expression(Box::new(last))));
                    parts.push(Spanned {
                        value: other,
                        span: p.span,
                    });
                    parts.extend(iter);
                    return Err(KExpression::new(parts));
                }
            }
        }
        Ok((preceding, last))
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression")
            .field("parts", &self.parts)
            .finish()
    }
}

impl<'a> Parseable<'a> for KExpression<'a> {
    fn equal(&self, other: &dyn Parseable<'a>) -> bool {
        self.summarize() == other.summarize()
    }
    fn ktype(&self) -> crate::machine::model::KType<'a> {
        crate::machine::model::KType::KExpression
    }
    fn summarize(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.value.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
