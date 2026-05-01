use crate::dispatch::kfunction::{is_fixed_token, UntypedElement, UntypedKey};
use crate::dispatch::kobject::KObject;
use crate::dispatch::ktraits::{Parseable, Executable};

/// Concrete literal kinds the parser recognizes; produced by `tokens::try_literal` and consumed
/// when resolving an `ExpressionPart` into a runtime `KObject`.
#[derive(Debug, Clone)]
pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

/// One element inside a parsed expression: a raw identifier-like `Token`, a nested
/// sub-`Expression`, a `ListLiteral` from `[...]` source syntax, a fully-typed `Literal`, or a
/// `Future` slot carrying the runtime result of a sub-expression that has already been
/// scheduled and run. The parser emits everything except `Future`; the scheduler introduces
/// `Future` when it splices a dep's result into its dependent's parts list before late
/// dispatch.
pub enum ExpressionPart<'a> {
    Token(String),
    Expression(Box<KExpression<'a>>),
    /// A `[a b c]` source-level list. Each element is itself an `ExpressionPart`; sub-expression
    /// elements (`ExpressionPart::Expression`) are scheduled as deps and replaced with `Future`s
    /// before the parent is dispatched. The whole literal resolves to `KObject::List` at
    /// `resolve()` time.
    ListLiteral(Vec<ExpressionPart<'a>>),
    Literal(KLiteral),
    Future(&'a KObject<'a>),
}

impl<'a> std::fmt::Debug for ExpressionPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressionPart::Token(s) => f.debug_tuple("Token").field(s).finish(),
            ExpressionPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
            ExpressionPart::ListLiteral(items) => f.debug_tuple("ListLiteral").field(items).finish(),
            ExpressionPart::Literal(l) => f.debug_tuple("Literal").field(l).finish(),
            ExpressionPart::Future(obj) => write!(f, "Future({})", obj.summarize()),
        }
    }
}

impl<'a> ExpressionPart<'a> {
    pub fn expression(parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
        ExpressionPart::Expression(Box::new(KExpression { parts }))
    }

    pub fn resolve(&self) -> KObject<'a> {
        match self {
            ExpressionPart::Token(s) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression((**e).clone()),
            // A list literal materializes into `KObject::List` by resolving each element. Any
            // sub-expression elements should already have been replaced by `Future`s by the
            // scheduler before this runs (see `schedule_expr`'s ListLiteral handling); a raw
            // `Expression` element here would round-trip through `KExpression` rather than its
            // computed value, which is the same fate any other `Expression` part suffers
            // outside the eager pipeline.
            ExpressionPart::ListLiteral(items) => {
                KObject::List(items.iter().map(|p| p.resolve()).collect())
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
            ExpressionPart::Token(s) => ExpressionPart::Token(s.clone()),
            ExpressionPart::Expression(e) => ExpressionPart::Expression(e.clone()),
            ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(items.clone()),
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
    /// Bucket key for this expression: tokens that look fixed (no lowercase letters) become
    /// `Fixed(s)`; lowercase identifier-like tokens and all literal/expression/future parts
    /// become `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature
    /// that should match — `is_fixed_token` is the shared rule.
    pub fn untyped_key(&self) -> UntypedKey {
        self.parts
            .iter()
            .map(|part| match part {
                ExpressionPart::Token(s) if is_fixed_token(s) => UntypedElement::Fixed(s.clone()),
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
        fn part_summary(p: &ExpressionPart<'_>) -> String {
            match p {
                ExpressionPart::Token(s) => s.clone(),
                ExpressionPart::Expression(e) => e.summarize(),
                ExpressionPart::ListLiteral(items) => {
                    let inner: Vec<String> = items.iter().map(part_summary).collect();
                    format!("[{}]", inner.join(" "))
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
        self.parts.iter()
            .map(part_summary)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl<'a> Executable for KExpression<'a> {
    fn execute(&self, _args: &[&dyn Parseable]) -> Box<dyn Parseable> {
        Box::new(KObject::KString(self.summarize()))
    }
}
