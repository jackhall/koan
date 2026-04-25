use crate::dispatch::kobject::KObject;
use crate::dispatch::ktraits::{Parseable, Executable};

pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

pub enum ExpressionPart {
    Token(String),
    Expression(Box<KExpression>),
    Literal(KLiteral),
}

impl ExpressionPart {
    pub fn expression(parts: Vec<ExpressionPart>) -> ExpressionPart {
        ExpressionPart::Expression(Box::new(KExpression { parts }))
    }

    pub fn resolve<'a>(&self) -> KObject<'a> {
        match self {
            ExpressionPart::Token(s) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KString(e.summarize()),
        }
    }
}

pub struct KExpression {
    pub parts: Vec<ExpressionPart>,
}

impl Parseable for KExpression {
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
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Executable for KExpression {
    fn execute(&self, _args: &[&dyn Parseable]) -> Box<dyn Parseable> {
        Box::new(KObject::KString(self.summarize()))
    }
}
