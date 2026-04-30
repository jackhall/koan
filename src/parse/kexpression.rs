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
/// sub-`Expression`, a fully-typed `Literal`, or a `Future` slot carrying the runtime result of a
/// sub-expression that has already been scheduled and run. The parser only emits the first three
/// variants; `Future` is introduced by the scheduler when it splices a dep's result into its
/// dependent's parts list before late dispatch.
pub enum ExpressionPart<'a> {
    Token(String),
    Expression(Box<KExpression<'a>>),
    Literal(KLiteral),
    Future(&'a KObject<'a>),
}

impl<'a> std::fmt::Debug for ExpressionPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressionPart::Token(s) => f.debug_tuple("Token").field(s).finish(),
            ExpressionPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
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
            ExpressionPart::Future(obj) => match obj {
                KObject::Number(n) => KObject::Number(*n),
                KObject::KString(s) => KObject::KString(s.clone()),
                KObject::Bool(b) => KObject::Bool(*b),
                KObject::Null => KObject::Null,
                other => KObject::KString(other.summarize()),
            },
        }
    }
}

impl<'a> Clone for ExpressionPart<'a> {
    fn clone(&self) -> Self {
        match self {
            ExpressionPart::Token(s) => ExpressionPart::Token(s.clone()),
            ExpressionPart::Expression(e) => ExpressionPart::Expression(e.clone()),
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

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression").field("parts", &self.parts).finish()
    }
}

impl<'a> Parseable for KExpression<'a> {
    fn equal(&self, other: &dyn Parseable) -> bool { self.summarize() == other.summarize() }
    fn summarize(&self) -> String {
        self.parts.iter()
            .map(|p| match p {
                ExpressionPart::Token(s) => s.clone(),
                ExpressionPart::Expression(e) => e.summarize(),
                ExpressionPart::Literal(lit) => match lit {
                    KLiteral::Number(n) => n.to_string(),
                    KLiteral::String(s) => s.clone(),
                    KLiteral::Boolean(b) => b.to_string(),
                    KLiteral::Null => "null".to_string(),
                },
                ExpressionPart::Future(obj) => obj.summarize(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl<'a> Executable for KExpression<'a> {
    fn execute(&self, _args: &[&dyn Parseable]) -> Box<dyn Parseable> {
        Box::new(KObject::KString(self.summarize()))
    }
}
